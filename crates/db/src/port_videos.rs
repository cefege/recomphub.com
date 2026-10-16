//! Port-attributed video discovery: README video staging and the per-port
//! yt-dlp search schedule. Video rows themselves live in `videos`.

use crate::DbResult;
use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use std::collections::HashSet;

/// A README-linked video waiting to be resolved with yt-dlp.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ReadmeRef {
    pub port_id: i64,
    pub youtube_id: String,
    pub game_id: i64,
    pub port_name: String,
    pub platforms: Vec<String>,
}

/// A published port due for a yt-dlp search, with the names the search needs.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SearchPort {
    pub port_id: i64,
    pub game_id: i64,
    pub port_name: String,
    pub repo_url: String,
    pub game_name: String,
    pub game_alt_names: Vec<String>,
    pub platforms: Vec<String>,
}

/// Stores README video IDs for a port. Existing rows are left alone, so a
/// resolved video is not re-queued when the README is scanned again.
pub async fn record_readme_videos(
    ex: impl PgExecutor<'_>,
    port_id: i64,
    youtube_ids: &[String],
) -> DbResult<()> {
    if youtube_ids.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO port_readme_videos (port_id, youtube_id)
         SELECT $1, u FROM unnest($2::text[]) AS u
         ON CONFLICT (port_id, youtube_id) DO NOTHING",
    )
    .bind(port_id)
    .bind(youtube_ids)
    .execute(ex)
    .await?;
    Ok(())
}

/// Unresolved README videos of published ports, oldest first. Refs that have
/// already failed `max_attempts` times are skipped.
pub async fn due_readme_refs(
    ex: impl PgExecutor<'_>,
    max_attempts: i32,
    limit: i64,
) -> DbResult<Vec<ReadmeRef>> {
    Ok(sqlx::query_as::<_, ReadmeRef>(
        "SELECT r.port_id, r.youtube_id, p.game_id, p.name AS port_name,
                port_platforms(p) AS platforms
         FROM port_readme_videos r
         JOIN ports p ON p.id = r.port_id
         WHERE p.status = 'published'
           AND r.resolved_at IS NULL
           AND r.attempts < $1
         ORDER BY r.found_at, r.port_id, r.youtube_id
         LIMIT $2",
    )
    .bind(max_attempts)
    .bind(limit)
    .fetch_all(ex)
    .await?)
}

/// Marks a README video as resolved (stored, or confirmed unusable).
pub async fn mark_readme_resolved(
    ex: impl PgExecutor<'_>,
    port_id: i64,
    youtube_id: &str,
) -> DbResult<()> {
    sqlx::query(
        "UPDATE port_readme_videos
         SET resolved_at = now(), last_error = NULL
         WHERE port_id = $1 AND youtube_id = $2",
    )
    .bind(port_id)
    .bind(youtube_id)
    .execute(ex)
    .await?;
    Ok(())
}

/// Records a failed attempt at resolving a README video.
pub async fn mark_readme_failed(
    ex: impl PgExecutor<'_>,
    port_id: i64,
    youtube_id: &str,
    error: &str,
) -> DbResult<()> {
    sqlx::query(
        "UPDATE port_readme_videos
         SET attempts = attempts + 1, last_error = $3
         WHERE port_id = $1 AND youtube_id = $2",
    )
    .bind(port_id)
    .bind(youtube_id)
    .bind(error)
    .execute(ex)
    .await?;
    Ok(())
}

/// Published, non-decompilation ports, never searched first, then stalest first.
pub async fn ports_due_for_search(
    ex: impl PgExecutor<'_>,
    limit: i64,
) -> DbResult<Vec<SearchPort>> {
    Ok(sqlx::query_as::<_, SearchPort>(
        "SELECT p.id AS port_id, p.game_id, p.name AS port_name, p.repo_url,
                g.name AS game_name, g.alt_names AS game_alt_names,
                port_platforms(p) AS platforms
         FROM ports p
         JOIN games g ON g.id = p.game_id
         LEFT JOIN port_video_searches s ON s.port_id = p.id
         WHERE p.status = 'published' AND p.kind <> 'decompilation'
         ORDER BY s.last_run_at NULLS FIRST, p.id
         LIMIT $1",
    )
    .bind(limit)
    .fetch_all(ex)
    .await?)
}

/// Youtube IDs already stored for a game under any target, used to skip repeats.
pub async fn known_youtube_ids(ex: impl PgExecutor<'_>, game_id: i64) -> DbResult<HashSet<String>> {
    let ids = sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT youtube_id FROM videos WHERE game_id = $1",
    )
    .bind(game_id)
    .fetch_all(ex)
    .await?;
    Ok(ids.into_iter().collect())
}

/// Stamps a port's search as run. `error` is the last failure, if any. A
/// failed port still rotates to the back of the queue.
pub async fn mark_search_run(
    ex: impl PgExecutor<'_>,
    port_id: i64,
    error: Option<&str>,
    at: DateTime<Utc>,
) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO port_video_searches (port_id, last_run_at, last_error)
         VALUES ($1, $2, $3)
         ON CONFLICT (port_id) DO UPDATE
           SET last_run_at = EXCLUDED.last_run_at, last_error = EXCLUDED.last_error",
    )
    .bind(port_id)
    .bind(at)
    .bind(error)
    .execute(ex)
    .await?;
    Ok(())
}
