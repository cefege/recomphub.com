//! The media table: queued image downloads and their public AVIF outputs.

use crate::DbResult;
use chrono::{DateTime, Utc};
use recomphub_shared::slugify;
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
    pub render_version: i32,
    pub public_stem: String,
    pub created_at: DateTime<Utc>,
}

/// Stable 16-hex-character name for a media item: the first 8 bytes of sha256(url + kind).
pub fn media_hash(source_url: &str, kind: &str) -> String {
    let digest = Sha256::digest(format!("{source_url}{kind}").as_bytes());
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// Validates and normalizes an owning path stem. Sources are processing inputs and never affect
/// public filenames.
pub fn public_stem(stem: &str) -> String {
    let slug = slugify(stem);
    let cut = slug
        .char_indices()
        .take_while(|(index, _)| *index < 80)
        .map(|(_, character)| character)
        .collect::<String>();
    let stem = cut.trim_matches('-');
    if stem.is_empty() {
        "media".to_string()
    } else {
        stem.to_string()
    }
}

/// Queues a download for `(source_url, kind)` and returns its id. The first non-empty semantic
/// stem wins, keeping URLs stable when the same source is encountered again.
pub async fn enqueue(
    ex: impl PgExecutor<'_>,
    kind: &str,
    source_url: &str,
    stem: &str,
) -> DbResult<i64> {
    let hash = media_hash(source_url, kind);
    let stem = public_stem(stem);
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO media (kind, source_url, hash, public_stem, status)
         VALUES ($1, $2, $3, $4, 'pending')
         ON CONFLICT (source_url, kind) DO UPDATE
             SET public_stem = CASE
                 WHEN media.public_stem = 'media' AND EXCLUDED.public_stem <> 'media'
                     THEN EXCLUDED.public_stem
                 ELSE media.public_stem
             END
         RETURNING id",
    )
    .bind(kind)
    .bind(source_url)
    .bind(hash)
    .bind(stem)
    .fetch_one(ex)
    .await?;
    Ok(id)
}

/// Up to `limit` rows still to process: pending, retryable failures, or a ready rendition from
/// an older renderer. A refresh remains public until all of its replacement files are written.
pub async fn next_pending(
    ex: impl PgExecutor<'_>,
    limit: i64,
    render_version: i32,
) -> DbResult<Vec<MediaRow>> {
    let rows = sqlx::query_as::<_, MediaRow>(
        "SELECT m.* FROM media m
         LEFT JOIN (
             SELECT cover_media_id, bool_or(port_count > 0) AS ported,
                    max(importance) AS importance
             FROM games WHERE cover_media_id IS NOT NULL GROUP BY cover_media_id
         ) g ON g.cover_media_id = m.id AND m.kind = 'cover'
         WHERE m.status = 'pending'
            OR (m.status = 'failed' AND m.attempts < 3)
            OR (m.status = 'ready' AND m.render_version < $2)
         ORDER BY CASE
                      WHEN m.kind = 'port_screenshot' AND m.source_url LIKE 'upload:%' THEN 0
                      WHEN m.kind = 'cover' THEN 1
                      WHEN m.kind = 'avatar' THEN 2
                      WHEN m.kind = 'video_thumb' THEN 3
                      WHEN m.kind = 'screenshot' THEN 4
                      WHEN m.kind = 'port_screenshot' THEN 4
                      ELSE 5
                  END,
                  g.ported DESC NULLS LAST,
                  g.importance DESC NULLS LAST,
                  m.id
         LIMIT $1",
    )
    .bind(limit)
    .bind(render_version)
    .fetch_all(ex)
    .await?;
    Ok(rows)
}

/// Atomically records the complete set of freshly-rendered assets and their source dimensions.
pub async fn mark_ready(
    ex: impl PgExecutor<'_>,
    id: i64,
    widths: Vec<i32>,
    width: i32,
    height: i32,
    render_version: i32,
) -> DbResult<()> {
    sqlx::query(
        "UPDATE media
         SET status = 'ready', widths = $2, width = $3, height = $4,
             render_version = $5, error = NULL
         WHERE id = $1",
    )
    .bind(id)
    .bind(widths)
    .bind(width)
    .bind(height)
    .bind(render_version)
    .execute(ex)
    .await?;
    Ok(())
}

/// Records a bounded render error. A failed refresh keeps the current ready rendition public.
pub async fn mark_failed(ex: impl PgExecutor<'_>, id: i64, error: &str) -> DbResult<()> {
    let error = error.chars().take(500).collect::<String>();
    sqlx::query(
        "UPDATE media
         SET status = CASE WHEN status = 'ready' THEN 'ready' ELSE 'failed' END,
             attempts = attempts + 1, error = $2
         WHERE id = $1",
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
