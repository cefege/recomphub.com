//! Submissions: creation with trust and daily limits, moderation, apply and revert.

use crate::games::IgdbGame;
use crate::ports::{PortFull, normalize_repo_url};
use crate::{DbError, DbResult, activity, games, ports, settings, users_sessions};
use chrono::{DateTime, Utc};
use recomphub_shared::{
    ModerationQueueCounts, ModerationQueueQuery, PortKind, QueueLane, Role, SubmissionKind,
    SubmissionStatus, TriageStateFilter,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::{PgConnection, PgPool};

/// Allowed `submissions.source` values.
pub const SOURCES: &[&str] = &[
    "user",
    "discovery:github",
    "discovery:reddit",
    "import:recompendium",
    "import:portsdr",
    "import:curated",
];

/// One submission row.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SubmissionRow {
    pub id: i64,
    pub kind: String,
    pub status: String,
    pub submitter_id: Option<i64>,
    pub source: String,
    pub payload: Value,
    pub before: Option<Value>,
    pub target_port_id: Option<i64>,
    pub target_game_id: Option<i64>,
    pub reviewer_id: Option<i64>,
    pub review_note: Option<String>,
    pub created_at: DateTime<Utc>,
    pub reviewed_at: Option<DateTime<Utc>>,
    pub triage: Option<Value>,
    pub triage_revision: i64,
    pub triage_attempted_at: Option<DateTime<Utc>>,
    pub triage_error: Option<String>,
}

impl SubmissionRow {
    /// The parsed kind.
    pub fn kind(&self) -> DbResult<SubmissionKind> {
        Ok(self.kind.parse::<SubmissionKind>()?)
    }

    /// The parsed status.
    pub fn status(&self) -> DbResult<SubmissionStatus> {
        Ok(self.status.parse::<SubmissionStatus>()?)
    }
}

/// A pending submission with its submitter's login and trust count.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PendingRow {
    #[sqlx(flatten)]
    pub sub: SubmissionRow,
    pub submitter_name: Option<String>,
    pub submitter_approved: Option<i64>,
}

/// Server-filtered page of pending moderation rows.
#[derive(Debug, Clone)]
pub struct PendingPage {
    pub items: Vec<PendingRow>,
    pub page: i64,
    pub pages: i64,
    pub total: i64,
    pub counts: ModerationQueueCounts,
}

/// One eligible row selected for evidence preparation.
#[derive(Debug, Clone)]
pub struct TriageDueRow {
    pub submission: SubmissionRow,
    pub reason: String,
}

/// Select pending rows in the required priority order. Explicit IDs bypass due timing.
pub async fn due_for_triage(
    pool: &PgPool,
    limit: i64,
    explicit_ids: Option<&[i64]>,
) -> DbResult<Vec<TriageDueRow>> {
    let ids = explicit_ids.map(<[i64]>::to_vec);
    let rows = sqlx::query_as::<_, SubmissionRow>(
        "SELECT * FROM submissions s
         WHERE s.status = 'pending'
           AND (s.kind = 'candidate' OR (s.source = 'user' AND s.kind = 'new_port'))
           AND ($2::bigint[] IS NULL OR s.id = ANY($2))
           AND (
             $2::bigint[] IS NOT NULL
             OR (
               s.triage_error IS NULL
               AND (
                 s.triage IS NULL
                 OR s.triage->>'assessment' IS NULL
                 OR COALESCE(s.triage->>'version', '') <> $3::text
                 OR s.triage_attempted_at < now() - interval '7 days'
               )
             )
             OR (s.triage_error IS NOT NULL AND s.triage_attempted_at < now() - interval '3 days')
           )
         ORDER BY
           CASE
             WHEN s.triage_error IS NULL AND s.triage IS NULL AND s.source = 'user' THEN 0
             WHEN s.triage_error IS NULL AND (s.triage IS NULL OR s.triage->>'assessment' IS NULL) AND s.kind = 'candidate' THEN 1
             WHEN s.triage_error IS NOT NULL THEN 2
             ELSE 3
           END,
           s.created_at, s.id
         LIMIT $1",
    )
    .bind(limit)
    .bind(ids)
    .bind(recomphub_shared::TRIAGE_VERSION.to_string())
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|submission| {
            let reason = if explicit_ids.is_some() {
                "explicit"
            } else if submission.triage_error.is_some() {
                "retry_failed"
            } else if submission.triage.is_none()
                || submission
                    .triage
                    .as_ref()
                    .is_some_and(|v| v.get("assessment").is_none_or(Value::is_null))
            {
                if submission.source == "user" {
                    "unclassified_member"
                } else {
                    "unclassified_candidate"
                }
            } else {
                "refresh"
            };
            TriageDueRow {
                submission,
                reason: reason.to_string(),
            }
        })
        .collect())
}

/// Persist only triage context, guarded against a concurrent moderator or worker update.
pub async fn save_triage(
    pool: &PgPool,
    id: i64,
    original_payload: &Value,
    expected_revision: i64,
    triage: Option<&Value>,
    error: Option<&str>,
) -> DbResult<bool> {
    let changed = sqlx::query(
        "UPDATE submissions
         SET triage = $4,
             triage_error = $5,
             triage_attempted_at = now(),
             triage_revision = triage_revision + CASE
               WHEN triage IS DISTINCT FROM $4 OR triage_error IS DISTINCT FROM $5 THEN 1 ELSE 0 END
         WHERE id = $1 AND status = 'pending' AND payload = $2 AND triage_revision = $3",
    )
    .bind(id)
    .bind(original_payload)
    .bind(expected_revision)
    .bind(triage)
    .bind(error)
    .execute(pool)
    .await?
    .rows_affected()
        == 1;
    Ok(changed)
}

/// A new submission before trust is applied.
#[derive(Debug, Clone)]
pub struct NewSubmission {
    pub kind: SubmissionKind,
    pub source: String,
    /// `None` means a bot, which is always trusted.
    pub submitter_id: Option<i64>,
    pub payload: Value,
    pub target_game_id: Option<i64>,
    pub target_port_id: Option<i64>,
}

/// Moderator choices made when approving a submission.
#[derive(Debug, Clone, Default)]
pub struct ApproveOverrides {
    /// Existing game to attach a port to.
    pub game_id: Option<i64>,
    /// IGDB game to create or refresh and attach a port to.
    pub new_game: Option<IgdbGame>,
    /// Port kind for candidates, which carry none.
    pub kind: Option<PortKind>,
    pub playability: Option<recomphub_shared::Playability>,
    pub declared_platforms: Option<Vec<recomphub_shared::TargetPlatform>>,
}

/// Payload of a `new_port` or `candidate` submission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortSubmission {
    /// Existing game the port belongs to.
    pub game_id: Option<i64>,
    /// IGDB game to create when `game_id` is absent.
    pub new_game: Option<IgdbGame>,
    #[serde(flatten)]
    pub port: PortFull,
}

/// True when a submitter's submissions apply immediately: moderators, or accounts with enough
/// approvals. Only covers *new* content (`new_port`, `new_game`); see [`create`] for why edits
/// to an existing port need more than account trust.
pub fn is_trusted(role: Role, approved_count: i64, threshold: i64) -> bool {
    role.is_moderator() || approved_count >= threshold
}

/// Applies `changes` to `current` and records the old values of the changed keys.
/// Unknown keys are rejected. Unchanged keys are not recorded.
pub fn merge_changes(
    current: &PortFull,
    changes: &Map<String, Value>,
) -> DbResult<(PortFull, Map<String, Value>)> {
    let mut merged = serde_json::to_value(current)?;
    let obj = merged
        .as_object_mut()
        .ok_or_else(|| DbError::InvalidState("port did not serialize to an object".to_string()))?;
    let mut before = Map::new();
    for (key, value) in changes {
        let cur = obj
            .get(key)
            .ok_or_else(|| DbError::InvalidPayload(format!("unknown port field {key}")))?;
        if cur != value {
            before.insert(key.clone(), cur.clone());
            obj.insert(key.clone(), value.clone());
        }
    }
    let full: PortFull = serde_json::from_value(merged)?;
    Ok((full, before))
}

/// Creates a submission. Trusted submitters' `new_port` and `new_game` apply at once and are
/// marked `auto_approved`: creating something new only adds to the catalog, so a few good
/// submissions earning that is a reasonable bar. An `edit_port` changes a listing that may be
/// someone else's work, and RecompHub cannot yet tell who that is (sign-in no longer carries a
/// verified GitHub login), so edits always go to the moderation queue unless the submitter is a
/// moderator. Enforces the banned check and the daily limit.
pub async fn create(pool: &PgPool, new: &NewSubmission) -> DbResult<SubmissionRow> {
    if !SOURCES.contains(&new.source.as_str()) {
        return Err(DbError::InvalidPayload(format!(
            "unknown source {}",
            new.source
        )));
    }
    let mut tx = pool.begin().await?;
    let trusted = match new.submitter_id {
        None => true,
        Some(uid) => {
            let user = users_sessions::lock_active(&mut *tx, uid).await?;
            let limit = settings::get_i64(&mut *tx, "submission_daily_limit", 30).await?;
            let used = sqlx::query_scalar::<_, i64>(
                "SELECT count(*)::bigint FROM submissions
                 WHERE submitter_id = $1 AND created_at > now() - interval '1 day'",
            )
            .bind(uid)
            .fetch_one(&mut *tx)
            .await?;
            if used >= limit {
                return Err(DbError::RateLimited(limit));
            }
            let role = user.role()?;
            if new.kind == SubmissionKind::EditPort {
                role.is_moderator()
            } else {
                let threshold = settings::get_i64(&mut *tx, "trust_threshold", 3).await?;
                is_trusted(role, user.approved_count as i64, threshold)
            }
        }
    };

    let mut row = sqlx::query_as::<_, SubmissionRow>(
        "INSERT INTO submissions (kind, status, submitter_id, source, payload, target_game_id, target_port_id)
         VALUES ($1, 'pending', $2, $3, $4, $5, $6)
         RETURNING *",
    )
    .bind(new.kind.as_str())
    .bind(new.submitter_id)
    .bind(new.source.clone())
    .bind(new.payload.clone())
    .bind(new.target_game_id)
    .bind(new.target_port_id)
    .fetch_one(&mut *tx)
    .await?;

    let auto = matches!(
        new.kind,
        SubmissionKind::NewPort | SubmissionKind::EditPort | SubmissionKind::NewGame
    );
    if trusted && auto {
        apply(&mut tx, row.id, &ApproveOverrides::default()).await?;
        row = sqlx::query_as::<_, SubmissionRow>(
            "UPDATE submissions SET status = 'auto_approved', reviewed_at = now()
             WHERE id = $1 RETURNING *",
        )
        .bind(row.id)
        .fetch_one(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(row)
}

/// Applies a submission's data change. Does not change the status.
/// Sets the target ids (and `before` for edits) on the submission row.
pub async fn apply(conn: &mut PgConnection, id: i64, ov: &ApproveOverrides) -> DbResult<()> {
    let sub = get(&mut *conn, id).await?;
    match sub.kind()? {
        SubmissionKind::NewPort => {
            let ps: PortSubmission = serde_json::from_value(sub.payload.clone())?;
            apply_new_port(conn, &sub, ov, ps).await?;
        }
        SubmissionKind::Candidate => {
            let ps: PortSubmission = serde_json::from_value(sub.payload.clone())?;
            apply_new_port(conn, &sub, ov, ps).await?;
        }
        SubmissionKind::EditPort => {
            let port_id = sub
                .target_port_id
                .ok_or_else(|| DbError::InvalidState("edit has no target port".to_string()))?;
            let changes = object(&sub.payload)?;
            let current = ports::load_full(&mut *conn, port_id).await?;
            let (merged, before) = merge_changes(&current, &changes)?;
            ports::update_port(&mut *conn, port_id, &merged).await?;
            let fields: Vec<String> = before.keys().cloned().collect();
            ports::lock_fields(&mut *conn, port_id, &fields).await?;
            sqlx::query("UPDATE submissions SET before = $2 WHERE id = $1")
                .bind(sub.id)
                .bind(Value::Object(before))
                .execute(&mut *conn)
                .await?;
            set_targets(&mut *conn, sub.id, Some(port_id), None).await?;
        }
        SubmissionKind::NewGame => {
            let g: IgdbGame = serde_json::from_value(sub.payload.clone())?;
            let game_id = games::sync_igdb(&mut *conn, &g).await?;
            activity::insert(
                &mut *conn,
                "game_added",
                game_id,
                None,
                &serde_json::json!({}),
                None,
            )
            .await?;
            set_targets(&mut *conn, sub.id, None, Some(game_id)).await?;
        }
        SubmissionKind::Report => {}
    }
    Ok(())
}

fn triage_repository_identity(sub: &SubmissionRow) -> Option<(String, i64)> {
    if sub.triage_error.is_some() {
        return None;
    }
    let triage = sub.triage.as_ref()?;
    if triage.get("version")?.as_u64()? != recomphub_shared::TRIAGE_VERSION as u64 {
        return None;
    }
    let snapshot = triage.get("snapshot")?;
    let repo_url = snapshot.get("repo_url")?.as_str()?.to_owned();
    let github_repo_id = snapshot.get("github_repo_id")?.as_i64()?;
    (github_repo_id > 0).then_some((repo_url, github_repo_id))
}

fn map_port_identity_conflict(error: DbError) -> DbError {
    match &error {
        DbError::Sql(sqlx::Error::Database(database))
            if database.code().as_deref() == Some("23505")
                && matches!(
                    database.constraint(),
                    Some("ports_repo_url_key" | "ports_github_repo_id_key")
                ) =>
        {
            DbError::Conflict("This repository is already listed.".to_string())
        }
        _ => error,
    }
}

async fn apply_new_port(
    conn: &mut PgConnection,
    sub: &SubmissionRow,
    ov: &ApproveOverrides,
    mut ps: PortSubmission,
) -> DbResult<()> {
    if let Some(kind) = ov.kind {
        ps.port.input.kind = kind.as_str().to_string();
    }
    if let Some(playability) = ov.playability {
        ps.port.input.playability = playability.as_str().to_string();
    }
    if let Some(targets) = &ov.declared_platforms {
        ps.port.input.declared_platforms = targets
            .iter()
            .map(|target| target.as_str().to_string())
            .collect();
    }
    let original_repo = ports::normalize_repo_url(&ps.port.input.repo_url)?;
    if let Some(existing) = ports::find_by_repo_url(&mut *conn, &original_repo).await? {
        return Err(DbError::Conflict(format!(
            "Repository is already listed as port {}.",
            existing.id
        )));
    }
    let triage_identity = triage_repository_identity(sub);
    if let Some((canonical_repo, github_repo_id)) = &triage_identity {
        if let Some(existing) = ports::find_by_repo_url(&mut *conn, canonical_repo).await? {
            return Err(DbError::Conflict(format!(
                "Canonical repository is already listed as port {}.",
                existing.id
            )));
        }
        if let Some(existing) =
            ports::other_port_with_repo(&mut *conn, 0, *github_repo_id, canonical_repo).await?
        {
            return Err(DbError::Conflict(format!(
                "GitHub repository is already listed as port {existing}."
            )));
        }
    }
    let game_id = resolve_game(&mut *conn, ov, &ps).await?;
    let port_id = ports::insert_port(
        &mut *conn,
        game_id,
        &ps.port,
        std::slice::from_ref(&sub.source),
    )
    .await
    .map_err(map_port_identity_conflict)?;
    if let Some((_, github_repo_id)) = triage_identity {
        sqlx::query("UPDATE ports SET github_repo_id = $2 WHERE id = $1")
            .bind(port_id)
            .bind(github_repo_id)
            .execute(&mut *conn)
            .await
            .map_err(|error| map_port_identity_conflict(DbError::Sql(error)))?;
    }
    activity::insert(
        &mut *conn,
        "port_added",
        game_id,
        Some(port_id),
        &serde_json::json!({}),
        None,
    )
    .await?;
    set_targets(&mut *conn, sub.id, Some(port_id), Some(game_id)).await?;
    Ok(())
}

async fn resolve_game(
    conn: &mut PgConnection,
    ov: &ApproveOverrides,
    ps: &PortSubmission,
) -> DbResult<i64> {
    if let Some(id) = ov.game_id.or(ps.game_id) {
        games::get(&mut *conn, id).await?;
        return Ok(id);
    }
    if let Some(g) = ov.new_game.as_ref().or(ps.new_game.as_ref()) {
        return games::sync_igdb(&mut *conn, g).await;
    }
    Err(DbError::InvalidState(
        "submission names no game".to_string(),
    ))
}

async fn set_targets(
    conn: &mut PgConnection,
    id: i64,
    port_id: Option<i64>,
    game_id: Option<i64>,
) -> DbResult<()> {
    sqlx::query(
        "UPDATE submissions
         SET target_port_id = COALESCE($2::bigint, target_port_id),
             target_game_id = COALESCE($3::bigint, target_game_id)
         WHERE id = $1",
    )
    .bind(id)
    .bind(port_id)
    .bind(game_id)
    .execute(conn)
    .await?;
    Ok(())
}

fn object(v: &Value) -> DbResult<Map<String, Value>> {
    v.as_object()
        .cloned()
        .ok_or_else(|| DbError::InvalidPayload("payload must be a JSON object".to_string()))
}

fn require_status(sub: &SubmissionRow, allowed: &[SubmissionStatus]) -> DbResult<()> {
    let current = sub.status()?;
    if allowed.contains(&current) {
        Ok(())
    } else {
        Err(DbError::InvalidState(format!(
            "submission {} is {}",
            sub.id, sub.status
        )))
    }
}

/// Approves a pending submission, applies it, and counts the approval toward the submitter's trust.
pub async fn approve(
    pool: &PgPool,
    id: i64,
    reviewer_id: i64,
    ov: &ApproveOverrides,
    expected_triage_revision: Option<i64>,
) -> DbResult<SubmissionRow> {
    let mut tx = pool.begin().await?;
    let sub = lock(&mut tx, id).await?;
    require_status(&sub, &[SubmissionStatus::Pending])?;
    if let Some(expected) = expected_triage_revision
        && sub.triage_revision != expected
    {
        return Err(DbError::Conflict(
            "Submission evidence changed; reload before reviewing.".to_string(),
        ));
    }
    apply(&mut tx, id, ov).await?;
    if let Some(uid) = sub.submitter_id {
        users_sessions::increment_approved(&mut *tx, uid).await?;
    }
    let row = sqlx::query_as::<_, SubmissionRow>(
        "UPDATE submissions SET status = 'approved', reviewer_id = $2, reviewed_at = now()
         WHERE id = $1 RETURNING *",
    )
    .bind(id)
    .bind(reviewer_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(row)
}

/// Rejects a pending submission with an optional note.
pub async fn reject(
    pool: &PgPool,
    id: i64,
    reviewer_id: i64,
    note: Option<&str>,
    expected_triage_revision: Option<i64>,
) -> DbResult<SubmissionRow> {
    let mut tx = pool.begin().await?;
    let sub = lock(&mut tx, id).await?;
    require_status(&sub, &[SubmissionStatus::Pending])?;
    if let Some(expected) = expected_triage_revision
        && sub.triage_revision != expected
    {
        return Err(DbError::Conflict(
            "Submission evidence changed; reload before reviewing.".to_string(),
        ));
    }
    let row = sqlx::query_as::<_, SubmissionRow>(
        "UPDATE submissions SET status = 'rejected', reviewer_id = $2, review_note = $3, reviewed_at = now()
         WHERE id = $1 RETURNING *",
    )
    .bind(id)
    .bind(reviewer_id)
    .bind(note.map(str::to_string))
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(row)
}

/// Reverts an applied submission. New ports and candidates are hidden, edits restore their
/// `before` values, and new games and reports only change status.
pub async fn revert(pool: &PgPool, id: i64, reviewer_id: i64) -> DbResult<SubmissionRow> {
    let mut tx = pool.begin().await?;
    let sub = lock(&mut tx, id).await?;
    require_status(
        &sub,
        &[SubmissionStatus::Approved, SubmissionStatus::AutoApproved],
    )?;
    match sub.kind()? {
        SubmissionKind::NewPort | SubmissionKind::Candidate => {
            let port_id = sub
                .target_port_id
                .ok_or_else(|| DbError::InvalidState("submission has no port".to_string()))?;
            ports::set_status(&mut tx, port_id, "hidden").await?;
        }
        SubmissionKind::EditPort => {
            let port_id = sub
                .target_port_id
                .ok_or_else(|| DbError::InvalidState("edit has no target port".to_string()))?;
            let before =
                object(sub.before.as_ref().ok_or_else(|| {
                    DbError::InvalidState("edit has no saved values".to_string())
                })?)?;
            let current = ports::load_full(&mut tx, port_id).await?;
            let (restored, _) = merge_changes(&current, &before)?;
            ports::update_port(&mut tx, port_id, &restored).await?;
        }
        SubmissionKind::NewGame | SubmissionKind::Report => {}
    }
    let row = sqlx::query_as::<_, SubmissionRow>(
        "UPDATE submissions SET status = 'reverted', reviewer_id = $2, reviewed_at = now()
         WHERE id = $1 RETURNING *",
    )
    .bind(id)
    .bind(reviewer_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(row)
}

/// Queues a discovered repo as a `candidate`. Skips repos already in `ports` or already
/// submitted as a candidate in any status. Returns whether a row was inserted.
pub async fn add_candidate(pool: &PgPool, source: &str, payload: &Value) -> DbResult<bool> {
    if !SOURCES.contains(&source) {
        return Err(DbError::InvalidPayload(format!("unknown source {source}")));
    }
    let raw = payload
        .get("repo_url")
        .and_then(Value::as_str)
        .ok_or_else(|| DbError::InvalidPayload("candidate needs repo_url".to_string()))?;
    let repo = normalize_repo_url(raw)?;
    if ports::find_by_repo_url(pool, &repo).await?.is_some() {
        return Ok(false);
    }
    let mut stored = payload.clone();
    if let Some(obj) = stored.as_object_mut() {
        obj.insert("repo_url".to_string(), Value::String(repo.clone()));
    }
    let listed = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM submissions WHERE kind = 'candidate' AND payload->>'repo_url' = $1)",
    )
    .bind(repo.clone())
    .fetch_one(pool)
    .await?;
    if listed {
        return Ok(false);
    }
    let res = sqlx::query(
        "INSERT INTO submissions (kind, status, submitter_id, source, payload)
         VALUES ('candidate', 'pending', NULL, $1, $2)
         ON CONFLICT DO NOTHING",
    )
    .bind(source)
    .bind(stored)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() == 1)
}

async fn lock(conn: &mut PgConnection, id: i64) -> DbResult<SubmissionRow> {
    sqlx::query_as::<_, SubmissionRow>("SELECT * FROM submissions WHERE id = $1 FOR UPDATE")
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or_else(|| DbError::NotFound(format!("submission {id}")))
}

/// A submission by id. Returns `NotFound` when missing.
pub async fn get(conn: &mut PgConnection, id: i64) -> DbResult<SubmissionRow> {
    sqlx::query_as::<_, SubmissionRow>("SELECT * FROM submissions WHERE id = $1")
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or_else(|| DbError::NotFound(format!("submission {id}")))
}

/// Loads one fixed-size pending queue lane plus global moderator counts.
pub async fn list_pending_page(
    pool: &PgPool,
    query: &ModerationQueueQuery,
) -> DbResult<PendingPage> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *tx)
        .await?;
    const PAGE_SIZE: i64 = 50;
    let category = query.category.map(enum_wire);
    let confidence = query.confidence.map(enum_wire);
    let target = query.target.map(enum_wire);
    let playability = query.playability.map(enum_wire);
    let state = match query.triage_state {
        TriageStateFilter::All => "all",
        TriageStateFilter::Untriaged => "untriaged",
        TriageStateFilter::Classified => "classified",
        TriageStateFilter::Failed => "failed",
    };
    let member_lane = query.lane == QueueLane::Members;
    let version = recomphub_shared::TRIAGE_VERSION.to_string();
    let predicate = "s.status = 'pending' AND (s.source = 'user') = $1
        AND ($2::text IS NULL OR s.triage_error IS NULL AND s.triage->>'version' = $3 AND s.triage->'assessment'->>'category' = $2)
        AND ($4::text IS NULL OR s.triage_error IS NULL AND s.triage->>'version' = $3 AND s.triage->'assessment'->>'confidence' = $4)
        AND ($5::text IS NULL OR s.triage_error IS NULL AND s.triage->>'version' = $3 AND s.triage->'assessment'->>'playability' = $5)
        AND ($6::text IS NULL OR s.triage_error IS NULL AND s.triage->>'version' = $3 AND s.triage->'assessment'->'targets' @> jsonb_build_array(to_jsonb($6::text)))
        AND (NOT $7 OR s.triage_error IS NULL AND s.triage->>'version' = $3 AND jsonb_array_length(COALESCE(s.triage->'assessment'->'warnings', '[]'::jsonb)) > 0)
        AND ($8 = 'all' OR ($8 = 'failed' AND s.triage_error IS NOT NULL) OR ($8 = 'classified' AND s.triage_error IS NULL AND s.triage->>'version' = $3 AND s.triage->>'assessment' IS NOT NULL) OR ($8 = 'untriaged' AND s.triage_error IS NULL AND (s.triage IS NULL OR s.triage->>'version' <> $3 OR s.triage->>'assessment' IS NULL)))";
    let total: i64 = sqlx::query_scalar(&format!(
        "SELECT count(*)::bigint FROM submissions s WHERE {predicate}"
    ))
    .bind(member_lane)
    .bind(category.as_deref())
    .bind(&version)
    .bind(confidence.as_deref())
    .bind(playability.as_deref())
    .bind(target.as_deref())
    .bind(query.flagged_only)
    .bind(state)
    .fetch_one(&mut *tx)
    .await?;
    let pages = ((total + PAGE_SIZE - 1) / PAGE_SIZE).max(1);
    let page = query.page.clamp(1, pages);
    let items = sqlx::query_as::<_, PendingRow>(&format!("SELECT s.*, u.display_name AS submitter_name, u.approved_count::bigint AS submitter_approved FROM submissions s LEFT JOIN users u ON u.id=s.submitter_id WHERE {predicate} ORDER BY s.created_at, s.id LIMIT $9 OFFSET $10"))
        .bind(member_lane).bind(category.as_deref()).bind(&version).bind(confidence.as_deref()).bind(playability.as_deref()).bind(target.as_deref()).bind(query.flagged_only).bind(state).bind(PAGE_SIZE).bind((page - 1) * PAGE_SIZE).fetch_all(&mut *tx).await?;
    let counts = sqlx::query_as::<_, (i64,i64,i64,i64,i64,i64,i64,i64,i64)>(
        "SELECT count(*) FILTER (WHERE source='user')::bigint, count(*) FILTER (WHERE source<>'user')::bigint,
          count(*) FILTER (WHERE source<>'user' AND triage_error IS NULL AND triage->>'version'=$1 AND triage->'assessment'->>'category'='game_project')::bigint,
          count(*) FILTER (WHERE source<>'user' AND triage_error IS NULL AND triage->>'version'=$1 AND triage->'assessment'->>'category'='tooling')::bigint,
          count(*) FILTER (WHERE source<>'user' AND triage_error IS NULL AND triage->>'version'=$1 AND triage->'assessment'->>'category'='reference')::bigint,
          count(*) FILTER (WHERE source<>'user' AND triage_error IS NULL AND triage->>'version'=$1 AND triage->'assessment'->>'category'='unrelated')::bigint,
          count(*) FILTER (WHERE source<>'user' AND triage_error IS NULL AND triage->>'version'=$1 AND triage->'assessment'->>'category'='uncertain')::bigint,
          count(*) FILTER (WHERE source<>'user' AND triage_error IS NULL AND (triage IS NULL OR triage->>'version'<>$1 OR triage->>'assessment' IS NULL))::bigint,
          count(*) FILTER (WHERE source<>'user' AND triage_error IS NOT NULL)::bigint FROM submissions WHERE status='pending'",
    ).bind(&version).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    Ok(PendingPage {
        items,
        page,
        pages,
        total,
        counts: ModerationQueueCounts {
            members: counts.0,
            found: counts.1,
            game_project: counts.2,
            tooling: counts.3,
            reference: counts.4,
            unrelated: counts.5,
            uncertain: counts.6,
            untriaged: counts.7,
            failed: counts.8,
        },
    })
}

fn enum_wire<T: Serialize>(value: T) -> String {
    serde_json::to_value(value)
        .expect("triage enum serialization")
        .as_str()
        .expect("triage enum string")
        .to_string()
}

/// Count of pending submissions from real members (not discovery/import), for the admin nav badge.
pub async fn pending_member_count(pool: &PgPool) -> DbResult<i64> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT count(*)::bigint FROM submissions WHERE status = 'pending' AND source = 'user'",
    )
    .fetch_one(pool)
    .await?)
}

/// Applied and reverted submissions, newest first.
pub async fn list_log(pool: &PgPool, limit: i64, offset: i64) -> DbResult<Vec<SubmissionRow>> {
    let rows = sqlx::query_as::<_, SubmissionRow>(
        "SELECT * FROM submissions
         WHERE status IN ('approved', 'auto_approved', 'rejected', 'reverted')
         ORDER BY COALESCE(reviewed_at, created_at) DESC, id DESC
         LIMIT $1 OFFSET $2",
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// A submitter's own submissions, newest first, for the "My submissions" account section.
pub async fn list_by_submitter(
    pool: &PgPool,
    submitter_id: i64,
    limit: i64,
    offset: i64,
) -> DbResult<Vec<SubmissionRow>> {
    let rows = sqlx::query_as::<_, SubmissionRow>(
        "SELECT * FROM submissions WHERE submitter_id = $1
         ORDER BY created_at DESC, id DESC
         LIMIT $2 OFFSET $3",
    )
    .bind(submitter_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn port(v: Value) -> PortFull {
        serde_json::from_value(v).expect("valid port json")
    }

    #[test]
    fn trust_needs_moderator_role_or_threshold() {
        assert!(is_trusted(Role::Moderator, 0, 3));
        assert!(is_trusted(Role::Admin, 0, 3));
        assert!(is_trusted(Role::User, 3, 3));
        assert!(!is_trusted(Role::User, 2, 3));
    }

    #[test]
    fn merge_records_only_changed_keys() {
        let current = port(json!({
            "name": "Port A",
            "kind": "static_recomp",
            "repo_url": "https://github.com/a/b",
            "description": "old"
        }));
        let changes = json!({"description": "new", "name": "Port A"});
        let (merged, before) = merge_changes(&current, changes.as_object().unwrap()).unwrap();
        assert_eq!(merged.input.description.as_deref(), Some("new"));
        assert_eq!(merged.input.name, "Port A");
        assert_eq!(before.len(), 1);
        assert_eq!(before.get("description"), Some(&json!("old")));
    }

    #[test]
    fn merge_rejects_unknown_fields() {
        let current = port(
            json!({"name": "A", "kind": "static_recomp", "repo_url": "https://github.com/a/b"}),
        );
        let changes = json!({"no_such_field": 1});
        assert!(merge_changes(&current, changes.as_object().unwrap()).is_err());
    }
}
