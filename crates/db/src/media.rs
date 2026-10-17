//! The media table: queued image downloads and their AVIF/JPEG outputs.

use crate::DbResult;
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::PgExecutor;

/// A row of the `media` table.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MediaRow {
    pub id: i64,
    pub kind: String,
    pub source_url: String,
    pub hash: String,
    pub widths: Vec<i32>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub status: String,
    pub attempts: i32,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Stable 16-hex-character name for a media item: the first 8 bytes of sha256(url + kind).
pub fn media_hash(source_url: &str, kind: &str) -> String {
    let digest = Sha256::digest(format!("{source_url}{kind}").as_bytes());
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// Queues a download for `(source_url, kind)` and returns its id. Idempotent.
pub async fn enqueue(ex: impl PgExecutor<'_>, kind: &str, source_url: &str) -> DbResult<i64> {
    let hash = media_hash(source_url, kind);
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO media (kind, source_url, hash, status)
         VALUES ($1, $2, $3, 'pending')
         ON CONFLICT (source_url, kind) DO UPDATE SET source_url = EXCLUDED.source_url
         RETURNING id",
    )
    .bind(kind)
    .bind(source_url)
    .bind(hash)
    .fetch_one(ex)
    .await?;
    Ok(id)
}

/// Up to `limit` rows still to process: pending, or failed with fewer than 3 attempts.
/// A member's uploaded screenshot goes first: someone is waiting on it, and it should never
/// sit behind a large catalog backlog. Covers, avatars and video thumbnails come next since
/// every listing shows them, then screenshots, then og images.
pub async fn next_pending(ex: impl PgExecutor<'_>, limit: i64) -> DbResult<Vec<MediaRow>> {
    let rows = sqlx::query_as::<_, MediaRow>(
        "SELECT m.* FROM media m
         LEFT JOIN (
             SELECT cover_media_id, bool_or(port_count > 0) AS ported,
                    max(importance) AS importance
             FROM games WHERE cover_media_id IS NOT NULL GROUP BY cover_media_id
         ) g ON g.cover_media_id = m.id AND m.kind = 'cover'
         WHERE m.status = 'pending' OR (m.status = 'failed' AND m.attempts < 3)
         ORDER BY CASE m.kind
                      WHEN 'port_screenshot' THEN 0
                      WHEN 'cover' THEN 1
                      WHEN 'avatar' THEN 2
                      WHEN 'video_thumb' THEN 3
                      WHEN 'screenshot' THEN 4
                      ELSE 5
                  END,
                  g.ported DESC NULLS LAST,
                  g.importance DESC NULLS LAST,
                  m.id
         LIMIT $1",
    )
    .bind(limit)
    .fetch_all(ex)
    .await?;
    Ok(rows)
}

/// Marks a row ready with its output widths and source dimensions.
pub async fn mark_ready(
    ex: impl PgExecutor<'_>,
    id: i64,
    widths: Vec<i32>,
    width: i32,
    height: i32,
) -> DbResult<()> {
    sqlx::query(
        "UPDATE media SET status = 'ready', widths = $2, width = $3, height = $4, error = NULL
         WHERE id = $1",
    )
    .bind(id)
    .bind(widths)
    .bind(width)
    .bind(height)
    .execute(ex)
    .await?;
    Ok(())
}

/// Marks a row failed, increments its attempts and stores the error.
pub async fn mark_failed(ex: impl PgExecutor<'_>, id: i64, error: &str) -> DbResult<()> {
    sqlx::query(
        "UPDATE media SET status = 'failed', attempts = attempts + 1, error = $2 WHERE id = $1",
    )
    .bind(id)
    .bind(error)
    .execute(ex)
    .await?;
    Ok(())
}

/// One media row by id, if it exists.
pub async fn get(ex: impl PgExecutor<'_>, id: i64) -> DbResult<Option<MediaRow>> {
    let row = sqlx::query_as::<_, MediaRow>("SELECT * FROM media WHERE id = $1")
        .bind(id)
        .fetch_optional(ex)
        .await?;
    Ok(row)
}

/// Media rows for a list of ids.
pub async fn get_many(ex: impl PgExecutor<'_>, ids: Vec<i64>) -> DbResult<Vec<MediaRow>> {
    let rows = sqlx::query_as::<_, MediaRow>("SELECT * FROM media WHERE id = ANY($1::bigint[])")
        .bind(ids)
        .fetch_all(ex)
        .await?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_16_hex_and_depends_on_kind() {
        let a = media_hash("https://images.igdb.com/x.jpg", "cover");
        let b = media_hash("https://images.igdb.com/x.jpg", "og");
        assert_eq!(a.len(), 16);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
        assert_eq!(a, media_hash("https://images.igdb.com/x.jpg", "cover"));
    }
}
