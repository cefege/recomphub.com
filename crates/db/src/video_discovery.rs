//! Durable, reviewable machine video discovery. Candidate rows are never public; only a
//! materialized row in `videos` is shown by catalog queries.

use chrono::{DateTime, Utc};
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

use crate::videos::{self, NewVideo};
use crate::{DbError, DbResult};

pub const CANDIDATE_SOURCE_GOOGLE: &str = "google";
pub const CANDIDATE_SOURCE_README: &str = "readme";
pub const DECISION_PUBLISHED: &str = "published";
pub const DECISION_PENDING: &str = "pending";
pub const DECISION_REJECTED: &str = "rejected";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DiscoveryPort {
    pub port_id: i64,
    pub game_id: i64,
    pub port_name: String,
    pub game_name: String,
    pub kind: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DiscoveryTask {
    pub id: Uuid,
    pub port_id: i64,
    pub game_id: i64,
    pub query: String,
    pub provider_task_id: Option<String>,
    pub state: String,
    pub submitted_at: DateTime<Utc>,
    pub collected_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DiscoveryCandidate {
    pub id: i64,
    pub port_id: i64,
    pub game_id: i64,
    pub youtube_id: String,
    pub url: String,
    pub title: String,
    pub description: String,
    pub rank: Option<i32>,
    pub thumbnail_media_id: Option<i64>,
    pub source: String,
    pub decision: String,
    pub decision_reason: Option<String>,
    pub reviewer_id: Option<i64>,
    pub reviewed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewDiscoveryCandidate {
    pub port_id: i64,
    pub game_id: i64,
    pub youtube_id: String,
    pub url: String,
    pub title: String,
    pub description: String,
    pub rank: Option<i32>,
    pub source: &'static str,
    pub decision: &'static str,
    pub decision_reason: Option<String>,
}

/// Marks stale provider work failed so a port can become due again after DataForSEO's three-day
/// Tasks Ready retention window expires.
pub async fn expire_uncollected(ex: impl PgExecutor<'_>) -> DbResult<u64> {
    Ok(sqlx::query(
        "UPDATE video_discovery_tasks
         SET state = 'failed', last_error = 'DataForSEO task expired before collection'
         WHERE state = 'submitted' AND submitted_at < now() - interval '3 days'",
    )
    .execute(ex)
    .await?
    .rows_affected())
}

/// Published ports with no outstanding provider task and no published or pending candidate from
/// the previous 30 days. Ordering deliberately matches the old port video search rotation.
pub async fn ports_due_for_discovery(
    ex: impl PgExecutor<'_>,
    limit: i64,
) -> DbResult<Vec<DiscoveryPort>> {
    Ok(sqlx::query_as::<_, DiscoveryPort>(
        "SELECT p.id AS port_id, p.game_id, p.name AS port_name, g.name AS game_name, p.kind
         FROM ports p
         JOIN games g ON g.id = p.game_id
         WHERE p.status = 'published'
           AND NOT EXISTS (
               SELECT 1 FROM video_discovery_tasks t
               WHERE t.port_id = p.id AND t.state = 'submitted'
           )
           AND NOT EXISTS (
               SELECT 1 FROM video_discovery_candidates c
               WHERE c.port_id = p.id
                 AND c.decision IN ('published', 'pending')
                 AND c.created_at >= now() - interval '30 days'
           )
         ORDER BY (
             SELECT min(t.submitted_at) FROM video_discovery_tasks t WHERE t.port_id = p.id
         ) NULLS FIRST, p.id
         LIMIT $1",
    )
    .bind(limit)
    .fetch_all(ex)
    .await?)
}

/// Records a successfully submitted provider task. The local UUID is supplied as the durable
/// request identity; the provider task id is used when matching Tasks Ready results.
pub async fn record_provider_task(ex: impl PgExecutor<'_>, task: &DiscoveryTask) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO video_discovery_tasks
             (id, port_id, game_id, query, provider_task_id, state, submitted_at, collected_at, last_error)
         VALUES ($1, $2, $3, $4, $5, 'submitted', now(), NULL, NULL)
         ON CONFLICT (port_id, query) DO UPDATE SET
             id = EXCLUDED.id,
             game_id = EXCLUDED.game_id,
             provider_task_id = EXCLUDED.provider_task_id,
             state = 'submitted',
             submitted_at = now(),
             collected_at = NULL,
             last_error = NULL",
    )
    .bind(task.id)
    .bind(task.port_id)
    .bind(task.game_id)
    .bind(&task.query)
    .bind(&task.provider_task_id)
    .execute(ex)
    .await?;
    Ok(())
}

/// Current discovery metadata for one published port. Used when collecting a task: open tasks
/// intentionally exclude their port from the due list, but result qualification still needs names.
pub async fn discovery_port(
    ex: impl PgExecutor<'_>,
    port_id: i64,
) -> DbResult<Option<DiscoveryPort>> {
    Ok(sqlx::query_as::<_, DiscoveryPort>(
        "SELECT p.id AS port_id, p.game_id, p.name AS port_name, g.name AS game_name, p.kind
         FROM ports p
         JOIN games g ON g.id = p.game_id
         WHERE p.id = $1 AND p.status = 'published'",
    )
    .bind(port_id)
    .fetch_optional(ex)
    .await?)
}

/// Submitted tasks that are eligible for a Tasks Ready collection attempt.
pub async fn submitted_tasks(ex: impl PgExecutor<'_>, limit: i64) -> DbResult<Vec<DiscoveryTask>> {
    Ok(sqlx::query_as::<_, DiscoveryTask>(
        "SELECT * FROM video_discovery_tasks
         WHERE state = 'submitted'
         ORDER BY submitted_at
         LIMIT $1",
    )
    .bind(limit)
    .fetch_all(ex)
    .await?)
}

pub async fn mark_task_collected(ex: impl PgExecutor<'_>, id: Uuid) -> DbResult<()> {
    sqlx::query(
        "UPDATE video_discovery_tasks
         SET state = 'collected', collected_at = now(), last_error = NULL
         WHERE id = $1 AND state = 'submitted'",
    )
    .bind(id)
    .execute(ex)
    .await?;
    Ok(())
}

pub async fn mark_task_error(ex: impl PgExecutor<'_>, id: Uuid, error: &str) -> DbResult<()> {
    let error = error.chars().take(500).collect::<String>();
    sqlx::query(
        "UPDATE video_discovery_tasks SET last_error = $2 WHERE id = $1 AND state = 'submitted'",
    )
    .bind(id)
    .bind(error)
    .execute(ex)
    .await?;
    Ok(())
}

/// Inserts a candidate once per port/video. Existing decisions, particularly permanent
/// rejections, are never reset by rediscovery.
pub async fn store_candidate(
    ex: impl PgExecutor<'_>,
    candidate: &NewDiscoveryCandidate,
) -> DbResult<DiscoveryCandidate> {
    Ok(sqlx::query_as::<_, DiscoveryCandidate>(
        "INSERT INTO video_discovery_candidates
             (port_id, game_id, youtube_id, url, title, description, rank, source, decision, decision_reason)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
         ON CONFLICT (port_id, youtube_id) DO UPDATE SET updated_at = now()
         RETURNING *",
    )
    .bind(candidate.port_id)
    .bind(candidate.game_id)
    .bind(&candidate.youtube_id)
    .bind(&candidate.url)
    .bind(&candidate.title)
    .bind(&candidate.description)
    .bind(candidate.rank)
    .bind(candidate.source)
    .bind(candidate.decision)
    .bind(&candidate.decision_reason)
    .fetch_one(ex)
    .await?)
}

pub async fn candidate(ex: impl PgExecutor<'_>, id: i64) -> DbResult<DiscoveryCandidate> {
    sqlx::query_as::<_, DiscoveryCandidate>(
        "SELECT * FROM video_discovery_candidates WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(ex)
    .await?
    .ok_or_else(|| DbError::NotFound(format!("video discovery candidate {id}")))
}

pub async fn is_rejected(
    ex: impl PgExecutor<'_>,
    port_id: i64,
    youtube_id: &str,
) -> DbResult<bool> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
             SELECT 1 FROM video_discovery_candidates
             WHERE port_id = $1 AND youtube_id = $2 AND decision = 'rejected'
         )",
    )
    .bind(port_id)
    .bind(youtube_id)
    .fetch_one(ex)
    .await?)
}

/// Makes a candidate public by materializing it through `videos`. It is safe to call for a
/// candidate already published, which lets the thumbnail attachment path retry independently.
pub async fn publish_candidate(
    pool: &PgPool,
    id: i64,
    reviewer_id: Option<i64>,
) -> DbResult<DiscoveryCandidate> {
    let mut tx = pool.begin().await?;
    let row = candidate(&mut *tx, id).await?;
    if row.decision == DECISION_REJECTED {
        return Err(DbError::InvalidState(format!(
            "rejected video discovery candidate {id}"
        )));
    }
    videos::upsert_video(
        &mut *tx,
        &NewVideo {
            game_id: row.game_id,
            port_id: Some(row.port_id),
            target: "any".to_string(),
            youtube_id: row.youtube_id.clone(),
            title: row.title.clone(),
            channel: None,
            published_at: None,
            thumb_media_id: row.thumbnail_media_id,
        },
    )
    .await?;
    let published = sqlx::query_as::<_, DiscoveryCandidate>(
        "UPDATE video_discovery_candidates
         SET decision = 'published', decision_reason = NULL,
             reviewer_id = COALESCE($2, reviewer_id),
             reviewed_at = CASE WHEN $2 IS NULL THEN reviewed_at ELSE now() END,
             updated_at = now()
         WHERE id = $1
         RETURNING *",
    )
    .bind(id)
    .bind(reviewer_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(published)
}

pub async fn reject_candidate(pool: &PgPool, id: i64, reviewer_id: i64) -> DbResult<()> {
    let result = sqlx::query(
        "UPDATE video_discovery_candidates
         SET decision = 'rejected', reviewer_id = $2, reviewed_at = now(), updated_at = now()
         WHERE id = $1 AND decision = 'pending'",
    )
    .bind(id)
    .bind(reviewer_id)
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound(format!(
            "pending video discovery candidate {id}"
        )));
    }
    Ok(())
}

pub async fn attach_thumbnail(pool: &PgPool, candidate_id: i64, media_id: i64) -> DbResult<()> {
    let mut tx = pool.begin().await?;
    let candidate = candidate(&mut *tx, candidate_id).await?;
    sqlx::query(
        "UPDATE video_discovery_candidates
         SET thumbnail_media_id = $2, updated_at = now()
         WHERE id = $1",
    )
    .bind(candidate_id)
    .bind(media_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE videos SET thumb_media_id = $2
         WHERE game_id = $1 AND port_id = $3 AND youtube_id = $4",
    )
    .bind(candidate.game_id)
    .bind(media_id)
    .bind(candidate.port_id)
    .bind(candidate.youtube_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn list_pending(ex: impl PgExecutor<'_>) -> DbResult<Vec<DiscoveryCandidate>> {
    Ok(sqlx::query_as::<_, DiscoveryCandidate>(
        "SELECT * FROM video_discovery_candidates WHERE decision = 'pending' ORDER BY created_at",
    )
    .fetch_all(ex)
    .await?)
}

pub fn new_task(port: &DiscoveryPort, query: String, provider_task_id: String) -> DiscoveryTask {
    DiscoveryTask {
        id: Uuid::new_v4(),
        port_id: port.port_id,
        game_id: port.game_id,
        query,
        provider_task_id: Some(provider_task_id),
        state: "submitted".to_string(),
        submitted_at: Utc::now(),
        collected_at: None,
        last_error: None,
    }
}
