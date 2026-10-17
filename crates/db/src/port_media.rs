//! Member-uploaded port screenshots. Always moderated, even from a signed-in account: a row
//! starts `pending` and only shows on the port page once a moderator approves it.

use chrono::{DateTime, Utc};
use sqlx::PgExecutor;

use crate::media::MediaRow;
use crate::{DbError, DbResult};

/// One `port_media` row.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PortMediaRow {
    pub id: i64,
    pub port_id: i64,
    pub media_id: i64,
    pub uploaded_by: Option<i64>,
    pub status: String,
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
}

/// Queues an uploaded, already-rendered image for review. Returns the `port_media` id.
pub async fn insert_pending(
    ex: impl PgExecutor<'_>,
    port_id: i64,
    media_id: i64,
    uploaded_by: i64,
) -> DbResult<i64> {
    Ok(sqlx::query_scalar::<_, i64>(
        "INSERT INTO port_media (port_id, media_id, uploaded_by, status)
         VALUES ($1, $2, $3, 'pending') RETURNING id",
    )
    .bind(port_id)
    .bind(media_id)
    .bind(uploaded_by)
    .fetch_one(ex)
    .await?)
}

/// Approved screenshots for a port, oldest first.
pub async fn approved_for_port(ex: impl PgExecutor<'_>, port_id: i64) -> DbResult<Vec<MediaRow>> {
    Ok(sqlx::query_as::<_, MediaRow>(
        "SELECT m.* FROM port_media pm
         JOIN media m ON m.id = pm.media_id
         WHERE pm.port_id = $1 AND pm.status = 'approved'
         ORDER BY pm.created_at",
    )
    .bind(port_id)
    .fetch_all(ex)
    .await?)
}

/// Every pending screenshot across the site, oldest first, for the moderation queue.
pub async fn list_pending(ex: impl PgExecutor<'_>) -> DbResult<Vec<PendingPortImage>> {
    Ok(sqlx::query_as::<_, PendingPortImage>(
        "SELECT pm.id, pm.port_id, pm.media_id, pm.uploaded_by, pm.status, pm.created_at,
                u.display_name AS uploader_name, p.name AS port_name, p.slug AS port_slug,
                g.slug AS game_slug
         FROM port_media pm
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
