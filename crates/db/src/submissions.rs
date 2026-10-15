//! Submissions: creation with trust and daily limits, moderation, apply and revert.

use crate::games::IgdbGame;
use crate::ports::{PortFull, normalize_repo_url};
use crate::{DbError, DbResult, activity, games, ports, settings, users_sessions};
use chrono::{DateTime, Utc};
use recomphub_shared::{PortKind, Role, SubmissionKind, SubmissionStatus};
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
    pub submitter_login: Option<String>,
    pub submitter_approved: Option<i64>,
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
/// approvals.
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

/// Creates a submission. Trusted submitters' `new_port`, `edit_port` and `new_game` apply at once
/// and are marked `auto_approved`. Enforces the banned check and the daily limit.
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
            let threshold = settings::get_i64(&mut *tx, "trust_threshold", 3).await?;
            is_trusted(user.role()?, user.approved_count as i64, threshold)
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

async fn apply_new_port(
    conn: &mut PgConnection,
    sub: &SubmissionRow,
    ov: &ApproveOverrides,
    mut ps: PortSubmission,
) -> DbResult<()> {
    if let Some(kind) = ov.kind {
        ps.port.input.kind = kind.as_str().to_string();
    }
    let game_id = resolve_game(&mut *conn, ov, &ps).await?;
    let port_id = ports::insert_port(
        &mut *conn,
        game_id,
        &ps.port,
        std::slice::from_ref(&sub.source),
    )
    .await?;
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
) -> DbResult<SubmissionRow> {
    let mut tx = pool.begin().await?;
    let sub = lock(&mut tx, id).await?;
    require_status(&sub, &[SubmissionStatus::Pending])?;
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
) -> DbResult<SubmissionRow> {
    let mut tx = pool.begin().await?;
    let sub = lock(&mut tx, id).await?;
    require_status(&sub, &[SubmissionStatus::Pending])?;
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

/// Pending submissions, oldest first.
pub async fn list_pending(pool: &PgPool, limit: i64, offset: i64) -> DbResult<Vec<PendingRow>> {
    let rows = sqlx::query_as::<_, PendingRow>(
        "SELECT s.*, u.login AS submitter_login, u.approved_count::bigint AS submitter_approved
         FROM submissions s
         LEFT JOIN users u ON u.id = s.submitter_id
         WHERE s.status = 'pending'
         ORDER BY s.created_at, s.id
         LIMIT $1 OFFSET $2",
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Applied and reverted submissions, newest first.
pub async fn list_log(pool: &PgPool, limit: i64, offset: i64) -> DbResult<Vec<SubmissionRow>> {
    let rows = sqlx::query_as::<_, SubmissionRow>(
        "SELECT * FROM submissions
         WHERE status IN ('approved', 'auto_approved', 'reverted')
         ORDER BY COALESCE(reviewed_at, created_at) DESC, id DESC
         LIMIT $1 OFFSET $2",
    )
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
