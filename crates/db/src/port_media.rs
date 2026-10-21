//! Member-uploaded port screenshots, and screenshots the GitHub scan finds in a port's own
//! README. Member uploads always start `pending`, even from a signed-in account, and only show
//! on the port page once a moderator approves them. README screenshots are trusted the same way
//! README-linked gameplay videos already are (see `recomphub_db::port_videos`): they come from
//! the project's own repository, not a stranger's upload, so they are inserted `approved`.

use chrono::{DateTime, Utc};
use sqlx::{PgExecutor, PgPool};

use crate::media::{self, MediaRow};
use crate::{DbError, DbResult};

/// One `port_media` row.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PortMediaRow {
    pub id: i64,
    pub port_id: i64,
    pub media_id: i64,
    pub uploaded_by: Option<i64>,
    pub status: String,
    pub source: String,
    pub position: Option<i32>,
    pub source_alt: Option<String>,
    pub decision_reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// A pending screenshot with enough to review it without opening the port page.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PendingPortImage {
    #[sqlx(flatten)]
    pub item: PortMediaRow,
    pub uploader_name: Option<String>,
    pub port_name: String,
    pub port_slug: String,
    pub game_slug: String,
    pub media_width: Option<i32>,
    pub media_height: Option<i32>,
    pub media_status: String,
    pub media_error: Option<String>,
}

/// Queues an uploaded, already-rendered image for review. Returns the `port_media` id. If this
/// account (or another) already queued the exact same file for this port, `media::enqueue`
/// resolves to the same `media_id`; the no-op update (the same trick `media::enqueue` uses)
/// makes `RETURNING id` give back the existing row instead of erroring.
pub async fn insert_pending(
    ex: impl PgExecutor<'_>,
    port_id: i64,
    media_id: i64,
    uploaded_by: i64,
) -> DbResult<i64> {
    Ok(sqlx::query_scalar::<_, i64>(
        "INSERT INTO port_media (port_id, media_id, uploaded_by, status, source)
         VALUES ($1, $2, $3, 'pending', 'upload')
         ON CONFLICT (port_id, media_id) DO UPDATE SET port_id = EXCLUDED.port_id
         RETURNING id",
    )
    .bind(port_id)
    .bind(media_id)
    .bind(uploaded_by)
    .fetch_one(ex)
    .await?)
}

/// How many screenshots this account has uploaded in the last 24 hours, for the per-account
/// daily cap. An unbounded upload endpoint writes to disk before any moderator looks at it, so
/// without a cap one account could fill the media volume.
pub async fn uploads_today(ex: impl PgExecutor<'_>, uploaded_by: i64) -> DbResult<i64> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT count(*)::bigint FROM port_media
         WHERE uploaded_by = $1 AND created_at > now() - interval '1 day'",
    )
    .bind(uploaded_by)
    .fetch_one(ex)
    .await?)
}

/// Approved, rendered screenshots for a port, README-derived ones first in their README order,
/// then uploads oldest first. A row whose render has not finished yet, or failed, is left out:
/// otherwise an unrendered README image would open the port's hero on a broken image. A
/// README-derived render under 480px wide is also left out: a README's first image is sometimes
/// a small project logo or badge that slipped past the alt-text filter, and a logo must not
/// replace the game's real screenshots in the hero. A member's upload is exempt: a moderator
/// already judged it before approving it, so a deliberately small screenshot stays.
pub async fn approved_for_port(ex: impl PgExecutor<'_>, port_id: i64) -> DbResult<Vec<MediaRow>> {
    Ok(sqlx::query_as::<_, MediaRow>(
        "SELECT m.* FROM port_media pm
         JOIN media m ON m.id = pm.media_id
         WHERE pm.port_id = $1 AND pm.status = 'approved' AND m.status = 'ready'
           AND (pm.source = 'upload' OR m.width >= 480)
         ORDER BY pm.position NULLS LAST, pm.created_at, pm.id",
    )
    .bind(port_id)
    .fetch_all(ex)
    .await?)
}

/// Every pending screenshot across the site, oldest first, for the moderation queue.
pub async fn list_pending(ex: impl PgExecutor<'_>) -> DbResult<Vec<PendingPortImage>> {
    Ok(sqlx::query_as::<_, PendingPortImage>(
        "SELECT pm.id, pm.port_id, pm.media_id, pm.uploaded_by, pm.status, pm.source,
                pm.position, pm.source_alt, pm.decision_reason, pm.created_at,
                u.display_name AS uploader_name, p.name AS port_name, p.slug AS port_slug,
                g.slug AS game_slug, m.width AS media_width, m.height AS media_height,
                m.status AS media_status, m.error AS media_error
         FROM port_media pm
         JOIN media m ON m.id = pm.media_id
         JOIN ports p ON p.id = pm.port_id
         JOIN games g ON g.id = p.game_id
         LEFT JOIN users u ON u.id = pm.uploaded_by
         WHERE pm.status = 'pending'
         ORDER BY pm.created_at",
    )
    .fetch_all(ex)
    .await?)
}

/// Count of pending screenshots, for the admin nav badge.
pub async fn count_pending(ex: impl PgExecutor<'_>) -> DbResult<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*)::bigint FROM port_media WHERE status = 'pending'")
            .fetch_one(ex)
            .await?,
    )
}

/// Approves or rejects a pending screenshot. Returns `NotFound` when it is missing or already
/// reviewed, so a double-click from two moderators cannot double-apply.
pub async fn review(
    ex: impl PgExecutor<'_>,
    id: i64,
    reviewer_id: i64,
    approve: bool,
) -> DbResult<()> {
    let status = if approve { "approved" } else { "rejected" };
    let res = sqlx::query(
        "UPDATE port_media SET status = $2, reviewed_by = $3, reviewed_at = now()
         WHERE id = $1 AND status = 'pending'",
    )
    .bind(id)
    .bind(status)
    .bind(reviewer_id)
    .execute(ex)
    .await?;
    if res.rows_affected() == 0 {
        return Err(DbError::NotFound(format!("pending port image {id}")));
    }
    Ok(())
}

/// Applies the deterministic README-render gate. Only a ready image at least 480px wide becomes
/// public automatically; smaller images stay reviewable rather than silently disappearing.
pub async fn approve_readme_after_render(
    ex: impl PgExecutor<'_>,
    media_id: i64,
    width: i32,
) -> DbResult<()> {
    let (status, reason) = if width >= 480 {
        ("approved", None)
    } else {
        ("pending", Some("README image is under 480 px wide"))
    };
    sqlx::query(
        "UPDATE port_media
         SET status = $2, decision_reason = $3,
             reviewed_by = CASE WHEN $2 = 'approved' THEN reviewed_by ELSE NULL END,
             reviewed_at = CASE WHEN $2 = 'approved' THEN reviewed_at ELSE NULL END
         WHERE media_id = $1 AND source = 'readme' AND status = 'pending'",
    )
    .bind(media_id)
    .bind(status)
    .bind(reason)
    .execute(ex)
    .await?;
    Ok(())
}

/// Records a renderer failure on automated README screenshots without changing their review
/// state. The detailed bounded error remains on `media.error`.
pub async fn note_readme_render_failure(ex: impl PgExecutor<'_>, media_id: i64) -> DbResult<()> {
    sqlx::query(
        "UPDATE port_media
         SET decision_reason = 'README image failed to render; see media error'
         WHERE media_id = $1 AND source = 'readme' AND status = 'pending'",
    )
    .bind(media_id)
    .execute(ex)
    .await?;
    Ok(())
}
/// Caps README-derived screenshots kept per port, matching `recomphub_ingest::readme_images::MAX_IMAGES`.
pub const README_IMAGE_CAP: usize = 6;

/// Enqueues and stages README screenshots in source order. Automated rows never become public
/// until the renderer validates them at a usable width or a moderator explicitly reviews them.
pub async fn record_readme_images(
    pool: &PgPool,
    port_id: i64,
    images: &[(String, String)],
    port_stem: &str,
) -> DbResult<()> {
    let images: Vec<&(String, String)> = images.iter().take(README_IMAGE_CAP).collect();
    let mut tx = pool.begin().await?;
    let mut media_ids = Vec::with_capacity(images.len());
    for (position, (url, alt)) in images.iter().enumerate() {
        let media_id = media::enqueue(
            &mut *tx,
            "port_screenshot",
            url,
            &format!("{port_stem}-screenshot-{}", position + 1),
        )
        .await?;
        media_ids.push(media_id);
        sqlx::query(
            "INSERT INTO port_media
                (port_id, media_id, uploaded_by, status, source, position, source_alt, decision_reason)
             VALUES ($1, $2, NULL, 'pending', 'readme', $3, $4, 'README image awaits successful render')
             ON CONFLICT (port_id, media_id) DO UPDATE SET
                 position = EXCLUDED.position,
                 source_alt = EXCLUDED.source_alt
             WHERE port_media.status = 'pending'",
        )
        .bind(port_id)
        .bind(media_id)
        .bind(position as i32)
        .bind(alt)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "DELETE FROM port_media
         WHERE port_id = $1 AND source = 'readme' AND status = 'approved'
           AND NOT (media_id = ANY($2))",
    )
    .bind(port_id)
    .bind(&media_ids)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}
