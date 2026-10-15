//! RecompHub stars on ports and wants on games. Each toggle is one transaction.

use crate::{DbError, DbResult, activity, games, users_sessions};
use serde::Serialize;
use sqlx::PgPool;

/// Result of toggling a star.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct StarState {
    pub starred: bool,
    pub count: i64,
}

/// Result of toggling a want.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct WantState {
    pub wanted: bool,
    pub count: i64,
}

/// Toggles the user's star on a published port. Adds a `starred` activity row when a star is
/// added; the dedupe index keeps repeat stars from flooding the feed.
pub async fn toggle_star(pool: &PgPool, port_id: i64, user_id: i64) -> DbResult<StarState> {
    let mut tx = pool.begin().await?;
    users_sessions::ensure_active(&mut *tx, user_id).await?;
    let game_id: i64 = sqlx::query_scalar::<_, i64>(
        "SELECT game_id FROM ports WHERE id = $1 AND status = 'published'",
    )
    .bind(port_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| DbError::NotFound(format!("port {port_id}")))?;

    let removed = sqlx::query("DELETE FROM port_stars WHERE user_id = $1 AND port_id = $2")
        .bind(user_id)
        .bind(port_id)
        .execute(&mut *tx)
        .await?
        .rows_affected()
        == 1;
    let starred = !removed;
    if starred {
        sqlx::query("INSERT INTO port_stars (user_id, port_id) VALUES ($1, $2)")
            .bind(user_id)
            .bind(port_id)
            .execute(&mut *tx)
            .await?;
        activity::insert(
            &mut *tx,
            "starred",
            game_id,
            Some(port_id),
            &serde_json::json!({}),
            Some(activity::actor_hash(user_id)),
        )
        .await?;
    }
    let count =
        sqlx::query_scalar::<_, i64>("SELECT count(*)::bigint FROM port_stars WHERE port_id = $1")
            .bind(port_id)
            .fetch_one(&mut *tx)
            .await?;
    sqlx::query("UPDATE ports SET star_count = $2 WHERE id = $1")
        .bind(port_id)
        .bind(count as i32)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(StarState { starred, count })
}

/// Toggles the user's want on a game. Adds a `wanted` activity row when a want is added.
/// Only portless games can be wanted, so the feed is about missing ports.
pub async fn toggle_want(pool: &PgPool, game_id: i64, user_id: i64) -> DbResult<WantState> {
    let mut tx = pool.begin().await?;
    users_sessions::ensure_active(&mut *tx, user_id).await?;
    let exists = sqlx::query_scalar::<_, i64>("SELECT id FROM games WHERE id = $1")
        .bind(game_id)
        .fetch_optional(&mut *tx)
        .await?;
    if exists.is_none() {
        return Err(DbError::NotFound(format!("game {game_id}")));
    }

    let removed = sqlx::query("DELETE FROM game_wants WHERE user_id = $1 AND game_id = $2")
        .bind(user_id)
        .bind(game_id)
        .execute(&mut *tx)
        .await?
        .rows_affected()
        == 1;
    let wanted = !removed;
    if wanted {
        sqlx::query("INSERT INTO game_wants (user_id, game_id) VALUES ($1, $2)")
            .bind(user_id)
            .bind(game_id)
            .execute(&mut *tx)
            .await?;
        activity::insert(
            &mut *tx,
            "wanted",
            game_id,
            None,
            &serde_json::json!({}),
            Some(activity::actor_hash(user_id)),
        )
        .await?;
    }
    games::refresh_rollup(&mut *tx, game_id).await?;
    let count =
        sqlx::query_scalar::<_, i64>("SELECT count(*)::bigint FROM game_wants WHERE game_id = $1")
            .bind(game_id)
            .fetch_one(&mut *tx)
            .await?;
    tx.commit().await?;
    Ok(WantState { wanted, count })
}
