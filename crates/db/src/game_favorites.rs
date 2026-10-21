//! Private, account-scoped catalog game favorites.

use std::collections::HashSet;

use sqlx::PgPool;

use crate::{DbError, DbResult, users_sessions};

/// The result of an additive account favorite import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeResult {
    pub accepted_game_ids: Vec<i64>,
    pub missing_game_ids: Vec<i64>,
}

/// Lists a user's favorite game ids from newest save to oldest.
pub async fn list(pool: &PgPool, user_id: i64) -> DbResult<Vec<i64>> {
    users_sessions::ensure_active(pool, user_id).await?;
    Ok(sqlx::query_scalar(
        "SELECT game_id FROM game_favorites WHERE user_id = $1 ORDER BY created_at DESC, game_id DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?)
}

/// Sets one favorite to an explicit state. The game must still be in the catalog.
pub async fn set(pool: &PgPool, user_id: i64, game_id: i64, favorite: bool) -> DbResult<()> {
    let mut tx = pool.begin().await?;
    users_sessions::ensure_active(&mut *tx, user_id).await?;
    let exists = sqlx::query_scalar::<_, i64>("SELECT id FROM games WHERE id = $1")
        .bind(game_id)
        .fetch_optional(&mut *tx)
        .await?
        .is_some();
    if !exists {
        return Err(DbError::NotFound(format!("game {game_id}")));
    }

    if favorite {
        sqlx::query(
            "INSERT INTO game_favorites (user_id, game_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(user_id)
        .bind(game_id)
        .execute(&mut *tx)
        .await?;
    } else {
        sqlx::query("DELETE FROM game_favorites WHERE user_id = $1 AND game_id = $2")
            .bind(user_id)
            .bind(game_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Adds the submitted catalog ids to a user's favorites without changing existing timestamps.
///
/// Callers provide validated, de-duplicated positive ids. Missing ids are returned separately so
/// device state can remove only values the catalog has confirmed unavailable.
pub async fn merge(pool: &PgPool, user_id: i64, game_ids: &[i64]) -> DbResult<MergeResult> {
    let mut tx = pool.begin().await?;
    users_sessions::ensure_active(&mut *tx, user_id).await?;

    let existing: HashSet<i64> = sqlx::query_scalar("SELECT id FROM games WHERE id = ANY($1)")
        .bind(game_ids)
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .collect();
    let accepted_game_ids: Vec<i64> = game_ids
        .iter()
        .copied()
        .filter(|id| existing.contains(id))
        .collect();
    let missing_game_ids: Vec<i64> = game_ids
        .iter()
        .copied()
        .filter(|id| !existing.contains(id))
        .collect();

    if !accepted_game_ids.is_empty() {
        sqlx::query(
            "INSERT INTO game_favorites (user_id, game_id)
             SELECT $1, id FROM UNNEST($2::bigint[]) AS id
             ON CONFLICT DO NOTHING",
        )
        .bind(user_id)
        .bind(&accepted_game_ids)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    Ok(MergeResult {
        accepted_game_ids,
        missing_game_ids,
    })
}
