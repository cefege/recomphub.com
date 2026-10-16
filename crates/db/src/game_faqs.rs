//! Generated game FAQs: the stored questions and answers, and the last failed attempt per game.
//! Written by `describe-games` in `recomphub-ingest`; read by the game page and its JSON-LD.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use recomphub_shared::dto::FaqItem;
use sqlx::{FromRow, PgExecutor};

use crate::DbResult;

/// A generated FAQ ready to store.
pub struct NewGameFaq<'a> {
    pub game_id: i64,
    pub items: &'a [FaqItem],
    pub source_hash: &'a str,
    pub provider: &'a str,
    pub model: &'a str,
}

/// The last failed generation attempt for one game.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct FaqAttempt {
    pub game_id: i64,
    pub source_hash: String,
    pub attempted_at: DateTime<Utc>,
    pub error: String,
}

/// Stored FAQ items for a game. Empty when none has been generated.
pub async fn items(ex: impl PgExecutor<'_>, game_id: i64) -> DbResult<Vec<FaqItem>> {
    let stored: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT items FROM game_faqs WHERE game_id = $1")
            .bind(game_id)
            .fetch_optional(ex)
            .await?;
    match stored {
        Some(value) => match serde_json::from_value(value) {
            Ok(items) => Ok(items),
            Err(e) => {
                tracing::warn!(game_id, error = %e, "stored FAQ does not decode; using the built-in FAQ");
                Ok(Vec::new())
            }
        },
        None => Ok(Vec::new()),
    }
}

/// Source hash of every stored FAQ, keyed by game id.
pub async fn stored_hashes(ex: impl PgExecutor<'_>) -> DbResult<HashMap<i64, String>> {
    let rows: Vec<(i64, String)> = sqlx::query_as("SELECT game_id, source_hash FROM game_faqs")
        .fetch_all(ex)
        .await?;
    Ok(rows.into_iter().collect())
}

/// Every failed attempt still on record.
pub async fn attempts(ex: impl PgExecutor<'_>) -> DbResult<Vec<FaqAttempt>> {
    Ok(sqlx::query_as::<_, FaqAttempt>(
        "SELECT game_id, source_hash, attempted_at, error FROM game_faq_attempts",
    )
    .fetch_all(ex)
    .await?)
}

/// Stores a generated FAQ, replacing any earlier one for the game.
pub async fn save(ex: impl PgExecutor<'_>, d: &NewGameFaq<'_>) -> DbResult<()> {
    let items = serde_json::to_value(d.items)?;
    sqlx::query(
        "INSERT INTO game_faqs (game_id, items, source_hash, provider, model, generated_at)
         VALUES ($1, $2, $3, $4, $5, now())
         ON CONFLICT (game_id) DO UPDATE SET
             items = EXCLUDED.items,
             source_hash = EXCLUDED.source_hash,
             provider = EXCLUDED.provider,
             model = EXCLUDED.model,
             generated_at = EXCLUDED.generated_at",
    )
    .bind(d.game_id)
    .bind(items)
    .bind(d.source_hash)
    .bind(d.provider)
    .bind(d.model)
    .execute(ex)
    .await?;
    Ok(())
}

/// Records a failed attempt, replacing the previous one for the game.
pub async fn record_failure(
    ex: impl PgExecutor<'_>,
    game_id: i64,
    source_hash: &str,
    error: &str,
) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO game_faq_attempts (game_id, source_hash, attempted_at, error)
         VALUES ($1, $2, now(), $3)
         ON CONFLICT (game_id) DO UPDATE SET
             source_hash = EXCLUDED.source_hash,
             attempted_at = EXCLUDED.attempted_at,
             error = EXCLUDED.error",
    )
    .bind(game_id)
    .bind(source_hash)
    .bind(error)
    .execute(ex)
    .await?;
    Ok(())
}

/// Removes stored FAQs for games that no longer have any port or decompilation.
pub async fn prune_without_ports(ex: impl PgExecutor<'_>) -> DbResult<u64> {
    let done = sqlx::query(
        "DELETE FROM game_faqs f
         USING games g
         WHERE g.id = f.game_id AND g.port_count = 0 AND g.decomp_count = 0",
    )
    .execute(ex)
    .await?;
    Ok(done.rows_affected())
}

/// Games that have at least one port or decompilation, ported games first, then by importance.
/// Generation follows this order. Games without projects keep the deterministic FAQ.
pub async fn candidate_ids(ex: impl PgExecutor<'_>) -> DbResult<Vec<i64>> {
    let ids: Vec<(i64,)> = sqlx::query_as(
        "SELECT id FROM games
         WHERE port_count > 0 OR decomp_count > 0
         ORDER BY (port_count > 0) DESC, importance DESC, id",
    )
    .fetch_all(ex)
    .await?;
    Ok(ids.into_iter().map(|(id,)| id).collect())
}
