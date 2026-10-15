//! Activity feed rows: starred, wanted, port_added, release and game_added.

use crate::DbResult;
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::PgExecutor;

/// An activity row joined with the names and cover needed to render it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ActivityRow {
    pub id: i64,
    pub created_at: DateTime<Utc>,
    pub kind: String,
    pub game_id: i64,
    pub port_id: Option<i64>,
    pub data: serde_json::Value,
    pub game_slug: String,
    pub game_name: String,
    pub game_cover_media_id: Option<i64>,
    pub port_slug: Option<String>,
    pub port_name: Option<String>,
}

const ACTIVITY_SQL: &str = "SELECT a.id, a.created_at, a.kind, a.game_id, a.port_id, a.data,
        g.slug AS game_slug, g.name AS game_name, g.cover_media_id AS game_cover_media_id,
        p.slug AS port_slug, p.name AS port_name
    FROM activity a
    JOIN games g ON g.id = a.game_id
    LEFT JOIN ports p ON p.id = a.port_id";

/// Stores an activity row. With `actor_hash`, a repeat of the same action is ignored.
/// Returns whether a row was inserted.
pub async fn insert(
    ex: impl PgExecutor<'_>,
    kind: &str,
    game_id: i64,
    port_id: Option<i64>,
    data: &serde_json::Value,
    actor_hash: Option<Vec<u8>>,
) -> DbResult<bool> {
    let res = sqlx::query(
        "INSERT INTO activity (kind, game_id, port_id, data, actor_hash)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT DO NOTHING",
    )
    .bind(kind)
    .bind(game_id)
    .bind(port_id)
    .bind(data.clone())
    .bind(actor_hash)
    .execute(ex)
    .await?;
    Ok(res.rows_affected() == 1)
}

/// Activity rows with `id > after_id`, oldest first, at most `limit`.
pub async fn since(
    ex: impl PgExecutor<'_>,
    after_id: i64,
    limit: i64,
) -> DbResult<Vec<ActivityRow>> {
    let sql = format!("{ACTIVITY_SQL} WHERE a.id > $1 ORDER BY a.id LIMIT $2");
    let rows = sqlx::query_as::<_, ActivityRow>(&sql)
        .bind(after_id)
        .bind(limit)
        .fetch_all(ex)
        .await?;
    Ok(rows)
}

/// The newest activity rows, newest first.
pub async fn recent(ex: impl PgExecutor<'_>, limit: i64) -> DbResult<Vec<ActivityRow>> {
    let sql = format!("{ACTIVITY_SQL} ORDER BY a.id DESC LIMIT $1");
    let rows = sqlx::query_as::<_, ActivityRow>(&sql)
        .bind(limit)
        .fetch_all(ex)
        .await?;
    Ok(rows)
}

/// The newest activity id, or 0 when the table is empty.
pub async fn latest_id(ex: impl PgExecutor<'_>) -> DbResult<i64> {
    let id = sqlx::query_scalar::<_, i64>("SELECT COALESCE(max(id), 0) FROM activity")
        .fetch_one(ex)
        .await?;
    Ok(id)
}

/// Deletes rows older than 30 days. Returns the number deleted.
pub async fn purge_old(ex: impl PgExecutor<'_>) -> DbResult<u64> {
    let res = sqlx::query("DELETE FROM activity WHERE created_at < now() - interval '30 days'")
        .execute(ex)
        .await?;
    Ok(res.rows_affected())
}

/// Pseudonymous actor hash for dedupe: sha256 of the decimal user id.
pub fn actor_hash(user_id: i64) -> Vec<u8> {
    Sha256::digest(user_id.to_string().as_bytes()).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actor_hash_is_stable_and_distinct() {
        assert_eq!(actor_hash(7), actor_hash(7));
        assert_ne!(actor_hash(7), actor_hash(8));
        assert_eq!(actor_hash(7).len(), 32);
    }
}
