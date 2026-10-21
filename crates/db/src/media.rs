//! The media table: queued image downloads and their public AVIF outputs.

use crate::{DbError, DbResult, PgPool};
use chrono::{DateTime, Utc};
use recomphub_shared::slugify;
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, PgExecutor};

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
    pub public_filename: Option<String>,
    pub next_attempt_at: Option<DateTime<Utc>>,
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
            OR (m.status = 'failed'
                AND m.attempts < 3
                AND (m.next_attempt_at IS NULL OR m.next_attempt_at <= now()))
            OR (m.status = 'ready'
                AND (m.render_version < $2 OR m.public_filename IS NULL)
                AND m.attempts < 3
                AND (m.next_attempt_at IS NULL OR m.next_attempt_at <= now()))
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

/// Claims one eligible row with a connection-scoped advisory lock. The lock is released
/// automatically if the worker exits, so another worker can safely resume after a crash.
///
/// Call [`release_claim`] once processing completes and keep `connection` alive meanwhile.
pub async fn claim_next(
    connection: &mut PgConnection,
    render_version: i32,
) -> DbResult<Option<MediaRow>> {
    for row in next_pending(&mut *connection, 16, render_version).await? {
        if let Some(row) = claim_row(connection, row.id, render_version).await? {
            return Ok(Some(row));
        }
    }
    Ok(None)
}

/// Locks and then rereads one row. The candidate list is intentionally only a priority hint:
/// another worker may complete a row between selection and lock acquisition.
async fn claim_row(
    connection: &mut PgConnection,
    media_id: i64,
    render_version: i32,
) -> DbResult<Option<MediaRow>> {
    let claimed: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(media_id)
        .fetch_one(&mut *connection)
        .await?;
    if !claimed {
        return Ok(None);
    }
    let row = sqlx::query_as::<_, MediaRow>(
        "SELECT * FROM media
         WHERE id = $1
           AND (
               status = 'pending'
               OR (status = 'failed'
                   AND attempts < 3
                   AND (next_attempt_at IS NULL OR next_attempt_at <= now()))
               OR (status = 'ready'
                   AND (render_version < $2 OR public_filename IS NULL)
                   AND attempts < 3
                   AND (next_attempt_at IS NULL OR next_attempt_at <= now()))
           )",
    )
    .bind(media_id)
    .bind(render_version)
    .fetch_optional(&mut *connection)
    .await?;
    if row.is_none() {
        release_claim(connection, media_id).await?;
    }
    Ok(row)
}

/// Releases a lock acquired by [`claim_next`].
pub async fn release_claim(connection: &mut PgConnection, media_id: i64) -> DbResult<()> {
    sqlx::query_scalar::<_, bool>("SELECT pg_advisory_unlock($1)")
        .bind(media_id)
        .fetch_one(connection)
        .await?;
    Ok(())
}

/// Counts ready rows that still need current bytes or a public filename.
pub async fn stale_ready_count(ex: impl PgExecutor<'_>, render_version: i32) -> DbResult<i64> {
    let count = sqlx::query_scalar(
        "SELECT COUNT(*) FROM media
         WHERE status = 'ready' AND (render_version < $1 OR public_filename IS NULL)",
    )
    .bind(render_version)
    .fetch_one(ex)
    .await?;
    Ok(count)
}

/// Makes every incomplete public rendition eligible for one explicit renderer cutover.
pub async fn reset_stale_attempts(ex: impl PgExecutor<'_>, render_version: i32) -> DbResult<()> {
    sqlx::query(
        "UPDATE media SET attempts = 0, next_attempt_at = NULL
         WHERE status = 'ready' AND (render_version < $1 OR public_filename IS NULL)",
    )
    .bind(render_version)
    .execute(ex)
    .await?;
    Ok(())
}

/// The public kind is intentionally shorter than the internal storage kind.
pub fn public_kind(kind: &str) -> &str {
    match kind {
        "port_screenshot" => "screenshot",
        "og" => "preview",
        "video_thumb" => "video",
        _ => kind,
    }
}

/// Finds the real owning context and stable preferred sequence for a ready row.
///
/// A shared media row keeps the first relation by the documented stable ordering. The old stem
/// is used only when no public relation owns the row.
pub async fn public_filename_input(pool: &PgPool, row: &MediaRow) -> DbResult<(String, i32)> {
    let kind = public_kind(&row.kind);
    let owner_and_number: Option<(String, i32)> = match row.kind.as_str() {
        "cover" | "og" => {
            sqlx::query_as(
                "SELECT g.slug, 1
                 FROM games g
                 WHERE ($2 = 'cover' AND g.cover_media_id = $1)
                    OR ($2 = 'og' AND g.og_media_id = $1)
                 ORDER BY g.id
                 LIMIT 1",
            )
            .bind(row.id)
            .bind(&row.kind)
            .fetch_optional(pool)
            .await?
        }
        "screenshot" => {
            sqlx::query_as(
                "SELECT g.slug, gm.position + 1
                 FROM game_media gm
                 JOIN games g ON g.id = gm.game_id
                 WHERE gm.media_id = $1
                 ORDER BY g.id, gm.position
                 LIMIT 1",
            )
            .bind(row.id)
            .fetch_optional(pool)
            .await?
        }
        "port_screenshot" => {
            sqlx::query_as(
                "SELECT CASE WHEN g.slug = p.slug THEN g.slug
                             ELSE g.slug || '-' || p.slug END,
                        coalesce(pm.position, pm.id::integer) + 1
                 FROM port_media pm
                 JOIN ports p ON p.id = pm.port_id
                 JOIN games g ON g.id = p.game_id
                 WHERE pm.media_id = $1
                 ORDER BY p.id, pm.position NULLS LAST, pm.id
                 LIMIT 1",
            )
            .bind(row.id)
            .fetch_optional(pool)
            .await?
        }
        "video_thumb" => {
            sqlx::query_as(
                "SELECT owner, sequence FROM (
                    SELECT CASE WHEN p.id IS NULL OR g.slug = p.slug THEN g.slug
                                ELSE g.slug || '-' || p.slug END AS owner,
                           v.thumb_media_id,
                           row_number() OVER (
                               PARTITION BY v.game_id, v.port_id ORDER BY v.id
                           )::integer AS sequence
                    FROM videos v
                    JOIN games g ON g.id = v.game_id
                    LEFT JOIN ports p ON p.id = v.port_id
                 ) videos
                 WHERE thumb_media_id = $1
                 LIMIT 1",
            )
            .bind(row.id)
            .fetch_optional(pool)
            .await?
        }
        "avatar" => {
            sqlx::query_as(
                "SELECT owner, 1 FROM (
                    SELECT coalesce(nullif(pa.github_login, ''), pa.name) AS owner,
                           0 AS source, p.id AS port_id, pa.position
                    FROM port_authors pa
                    JOIN ports p ON p.id = pa.port_id
                    WHERE pa.avatar_media_id = $1 AND pa.github_login IS NOT NULL
                    UNION ALL
                    SELECT u.display_name AS owner, 1 AS source, u.id AS port_id, 0 AS position
                    FROM users u
                    WHERE u.avatar_media_id = $1
                 ) owners
                 ORDER BY source, port_id, position
                 LIMIT 1",
            )
            .bind(row.id)
            .fetch_optional(pool)
            .await?
        }
        _ => None,
    };
    let (owner, preferred_number) = owner_and_number
        .unwrap_or_else(|| (row.public_stem.clone(), 1));
    Ok((
        public_stem(&format!("{owner}-{kind}")),
        preferred_number.max(1),
    ))
}

/// Atomically reserves the stable public filename for one fully-written media row.
pub async fn assign_public_filename(
    pool: &PgPool,
    media_id: i64,
    base: &str,
    preferred_number: i32,
) -> DbResult<String> {
    let base = public_stem(base);
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(&base)
        .execute(&mut *tx)
        .await?;
    let existing: Option<Option<String>> =
        sqlx::query_scalar("SELECT public_filename FROM media WHERE id = $1 FOR UPDATE")
            .bind(media_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some(existing) = existing else {
        return Err(DbError::NotFound(format!("media {media_id}")));
    };
    if let Some(filename) = existing {
        tx.commit().await?;
        return Ok(filename);
    }
    for number in preferred_number.max(1).. {
        let filename = format!("{base}-{number}.avif");
        let claimed: bool = sqlx::query_scalar(
            "SELECT NOT EXISTS (
                 SELECT 1 FROM media WHERE public_filename = $1
             )",
        )
        .bind(&filename)
        .fetch_one(&mut *tx)
        .await?;
        if claimed {
            sqlx::query(
                "UPDATE media SET public_filename = $2
                 WHERE id = $1 AND public_filename IS NULL",
            )
            .bind(media_id)
            .bind(&filename)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            return Ok(filename);
        }
        if number == i32::MAX {
            return Err(DbError::Conflict("media filename sequence exhausted".to_string()));
        }
    }
    unreachable!("unbounded positive sequence has an i32 maximum guard")
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
             render_version = $5, error = NULL, attempts = 0, next_attempt_at = NULL
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
             attempts = attempts + 1,
             next_attempt_at = now() + make_interval(
                 secs => LEAST(3600, 60 * power(2, LEAST(attempts, 6))::integer)
             ),
             error = $2
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

    #[sqlx::test(migrations = "./migrations")]
    async fn stale_candidate_is_rechecked_after_lock_acquisition(pool: PgPool) -> DbResult<()> {
        let id = enqueue(&pool, "screenshot", "https://example.test/stale.png", "fixture").await?;
        let mut stale_reader = pool.acquire().await?;
        assert_eq!(next_pending(&mut *stale_reader, 1, 2).await?[0].id, id);

        let mut finisher = pool.acquire().await?;
        assert!(sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock($1)")
            .bind(id)
            .fetch_one(&mut *finisher)
            .await?);
        mark_ready(&pool, id, vec![960], 960, 540, 2).await?;
        assign_public_filename(&pool, id, "fixture-screenshot", 1).await?;
        release_claim(&mut finisher, id).await?;

        assert!(claim_row(&mut stale_reader, id, 2).await?.is_none());
        Ok(())
    }
}
