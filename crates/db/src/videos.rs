//! YouTube gameplay videos per game and target, plus the query schedule for the sync job.

use crate::{DbError, DbResult};
use chrono::{DateTime, Utc};
use recomphub_shared::TargetPlatform;
use sqlx::PgExecutor;

/// Targets the YouTube sync searches for. `any` is added separately.
pub const SEARCH_TARGETS: &[&str] = &["windows", "macos", "linux", "web", "android", "ios"];

/// One stored video.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct VideoRow {
    pub id: i64,
    pub game_id: i64,
    pub port_id: Option<i64>,
    pub target: String,
    pub youtube_id: String,
    pub title: String,
    pub channel: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
    pub thumb_media_id: Option<i64>,
    pub hidden: bool,
    pub pending: bool,
    pub fetched_at: DateTime<Utc>,
}

/// A video found by the sync job, ready to store.
#[derive(Debug, Clone)]
pub struct NewVideo {
    pub game_id: i64,
    pub port_id: Option<i64>,
    /// A `TargetPlatform` value or `any`.
    pub target: String,
    pub youtube_id: String,
    pub title: String,
    pub channel: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
    pub thumb_media_id: Option<i64>,
}

/// One `(game, target)` pair the YouTube sync should search for.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct YoutubeCandidate {
    pub game_id: i64,
    pub target: String,
    pub game_name: String,
    pub port_name: Option<String>,
}

fn validate_target(target: &str) -> DbResult<()> {
    if target == "any" {
        return Ok(());
    }
    target
        .parse::<TargetPlatform>()
        .map(|_| ())
        .map_err(DbError::from)
}

/// Inserts or refreshes a video found by the sync job. A row that moderators hid stays hidden;
/// a row a member submitted (`pending`) is left untouched here, so a coincidental rediscovery
/// never bypasses review.
pub async fn upsert_video(ex: impl PgExecutor<'_>, v: &NewVideo) -> DbResult<i64> {
    validate_target(&v.target)?;
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO videos (game_id, port_id, target, youtube_id, title, channel, published_at, thumb_media_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (game_id, target, youtube_id) DO UPDATE
           SET title = EXCLUDED.title,
               channel = EXCLUDED.channel,
               published_at = COALESCE(EXCLUDED.published_at, videos.published_at),
               thumb_media_id = COALESCE(EXCLUDED.thumb_media_id, videos.thumb_media_id),
               port_id = COALESCE(EXCLUDED.port_id, videos.port_id),
               fetched_at = now()
         RETURNING id",
    )
    .bind(v.game_id)
    .bind(v.port_id)
    .bind(v.target.clone())
    .bind(v.youtube_id.clone())
    .bind(v.title.clone())
    .bind(v.channel.clone())
    .bind(v.published_at)
    .bind(v.thumb_media_id)
    .fetch_one(ex)
    .await?;
    Ok(id)
}

/// Videos for a game, newest first. Hidden and member-submitted-but-unreviewed rows are
/// included only when `include_hidden` is set (the moderation views; no public page passes it).
pub async fn list_for_game(
    ex: impl PgExecutor<'_>,
    game_id: i64,
    include_hidden: bool,
) -> DbResult<Vec<VideoRow>> {
    let rows = sqlx::query_as::<_, VideoRow>(
        "SELECT * FROM videos
         WHERE game_id = $1 AND ($2 OR (NOT hidden AND NOT pending))
         ORDER BY published_at DESC NULLS LAST, id DESC",
    )
    .bind(game_id)
    .bind(include_hidden)
    .fetch_all(ex)
    .await?;
    Ok(rows)
}

/// A member-submitted video, pending moderator approval. Reuses the sync job's
/// `(game, target, youtube_id)` uniqueness: resubmitting a known video just touches it and
/// never resets an already-approved video back to pending.
pub async fn submit_user_video(ex: impl PgExecutor<'_>, v: &NewVideo) -> DbResult<i64> {
    validate_target(&v.target)?;
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO videos (game_id, port_id, target, youtube_id, title, channel, thumb_media_id, pending)
         VALUES ($1, $2, $3, $4, $5, $6, $7, true)
         ON CONFLICT (game_id, target, youtube_id) DO UPDATE
           SET port_id = COALESCE(videos.port_id, EXCLUDED.port_id),
               fetched_at = now()
         RETURNING id",
    )
    .bind(v.game_id)
    .bind(v.port_id)
    .bind(v.target.clone())
    .bind(v.youtube_id.clone())
    .bind(v.title.clone())
    .bind(v.channel.clone())
    .bind(v.thumb_media_id)
    .fetch_one(ex)
    .await?;
    Ok(id)
}

/// A pending, member-submitted video with enough to review it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PendingVideo {
    #[sqlx(flatten)]
    pub video: VideoRow,
    pub game_name: String,
    pub game_slug: String,
    pub port_name: Option<String>,
    pub port_slug: Option<String>,
}

/// Every pending, member-submitted video, oldest first, for the moderation queue.
pub async fn list_pending(ex: impl PgExecutor<'_>) -> DbResult<Vec<PendingVideo>> {
    Ok(sqlx::query_as::<_, PendingVideo>(
        "SELECT v.*, g.name AS game_name, g.slug AS game_slug, p.name AS port_name, p.slug AS port_slug
         FROM videos v
         JOIN games g ON g.id = v.game_id
         LEFT JOIN ports p ON p.id = v.port_id
         WHERE v.pending
         ORDER BY v.fetched_at",
    )
    .fetch_all(ex)
    .await?)
}

/// Count of pending, member-submitted videos, for the admin nav badge.
pub async fn count_pending(ex: impl PgExecutor<'_>) -> DbResult<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*)::bigint FROM videos WHERE pending")
            .fetch_one(ex)
            .await?,
    )
}

/// Approves (clears `pending`, so the video becomes public) or rejects (deletes the row, since
/// it was never public) a pending submission. Returns `NotFound` if it is missing or already
/// reviewed.
pub async fn review_pending(ex: impl PgExecutor<'_>, id: i64, approve: bool) -> DbResult<()> {
    let res = if approve {
        sqlx::query("UPDATE videos SET pending = false WHERE id = $1 AND pending")
            .bind(id)
            .execute(ex)
            .await?
    } else {
        sqlx::query("DELETE FROM videos WHERE id = $1 AND pending")
            .bind(id)
            .execute(ex)
            .await?
    };
    if res.rows_affected() == 0 {
        return Err(DbError::NotFound(format!("pending video {id}")));
    }
    Ok(())
}

/// Hides or shows a video. Returns `NotFound` when the id is unknown.
pub async fn set_hidden(ex: impl PgExecutor<'_>, id: i64, hidden: bool) -> DbResult<()> {
    let res = sqlx::query("UPDATE videos SET hidden = $2 WHERE id = $1")
        .bind(id)
        .bind(hidden)
        .execute(ex)
        .await?;
    if res.rows_affected() == 0 {
        return Err(DbError::NotFound(format!("video {id}")));
    }
    Ok(())
}

/// The next `(game, target)` pairs to search, at most `limit`.
///
/// Pairs come from games with a published port, for each effective target of their published
/// ports (among the search targets), plus `any`. A pair is due when it has never run or last ran
/// more than 30 days ago. Order: the game's 180-day hits, then popularity.
pub async fn youtube_candidates(
    ex: impl PgExecutor<'_>,
    limit: i64,
) -> DbResult<Vec<YoutubeCandidate>> {
    let rows = sqlx::query_as::<_, YoutubeCandidate>(
        "WITH pairs AS (
           SELECT g.id AS game_id, x.target
           FROM games g
           CROSS JOIN LATERAL (
             SELECT DISTINCT unnest(port_platforms(p)) AS target
             FROM ports p
             WHERE p.game_id = g.id AND p.status = 'published'
           ) x
           WHERE g.port_count > 0 AND x.target = ANY($2::text[])
           UNION
           SELECT id, 'any' FROM games WHERE port_count > 0
         )
         SELECT pr.game_id,
                pr.target,
                g.name AS game_name,
                (SELECT p.name FROM ports p
                 WHERE p.game_id = g.id AND p.status = 'published'
                 ORDER BY p.star_count DESC, p.id
                 LIMIT 1) AS port_name
         FROM pairs pr
         JOIN games g ON g.id = pr.game_id
         LEFT JOIN youtube_queries q ON q.game_id = pr.game_id AND q.target = pr.target
         WHERE q.last_run_at IS NULL OR q.last_run_at < now() - interval '30 days'
         ORDER BY (SELECT COALESCE(sum(h.hits), 0)::bigint FROM game_hits_daily h
                   WHERE h.game_id = g.id AND h.day > current_date - 180) DESC,
                  g.importance DESC,
                  pr.game_id,
                  pr.target
         LIMIT $1",
    )
    .bind(limit)
    .bind(
        SEARCH_TARGETS
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<String>>(),
    )
    .fetch_all(ex)
    .await?;
    Ok(rows)
}

/// Records that a `(game, target)` search ran now.
pub async fn mark_query_run(ex: impl PgExecutor<'_>, game_id: i64, target: &str) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO youtube_queries (game_id, target, last_run_at)
         VALUES ($1, $2, now())
         ON CONFLICT (game_id, target) DO UPDATE SET last_run_at = now()",
    )
    .bind(game_id)
    .bind(target.to_string())
    .execute(ex)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_must_be_any_or_a_platform() {
        assert!(validate_target("any").is_ok());
        assert!(validate_target("windows").is_ok());
        assert!(validate_target("macos").is_ok());
        assert!(validate_target("amiga").is_err());
    }
}
