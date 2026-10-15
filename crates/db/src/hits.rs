//! Page-hit counting: visitor hashing with daily salts, dedupe, daily totals and ranking refresh.

use crate::DbResult;
use chrono::NaiveDate;
use rand::RngCore;
use sha2::{Digest, Sha256};
use sqlx::PgPool;

const BOT_WORDS: &[&str] = &["bot", "crawl", "spider", "slurp", "headless", "preview"];

/// True when the user agent looks like a crawler or a headless browser. These are never counted.
pub fn is_bot_user_agent(user_agent: &str) -> bool {
    let ua = user_agent.to_lowercase();
    BOT_WORDS.iter().any(|w| ua.contains(w))
}

/// Visitor hash: sha256(daily_salt || ip || user_agent). The same visitor gets a new hash each day.
pub fn visitor_hash(salt: &[u8], ip: &str, user_agent: &str) -> Vec<u8> {
    let mut h = Sha256::new();
    h.update(salt);
    h.update(ip.as_bytes());
    h.update(user_agent.as_bytes());
    h.finalize().to_vec()
}

/// The salt for `day`. The first caller creates it; later callers read the stored value.
pub async fn daily_salt(pool: &PgPool, day: NaiveDate) -> DbResult<Vec<u8>> {
    let mut fresh = [0u8; 32];
    rand::rng().fill_bytes(&mut fresh);
    sqlx::query("INSERT INTO daily_salts (day, salt) VALUES ($1, $2) ON CONFLICT (day) DO NOTHING")
        .bind(day)
        .bind(fresh.to_vec())
        .execute(pool)
        .await?;
    let salt = sqlx::query_scalar::<_, Vec<u8>>("SELECT salt FROM daily_salts WHERE day = $1")
        .bind(day)
        .fetch_one(pool)
        .await?;
    Ok(salt)
}

/// Counts one hit for `(game, visitor, day)`. The hit is counted only the first time for that
/// visitor on that day. Returns whether a hit was counted.
pub async fn record_hit(
    pool: &PgPool,
    game_id: i64,
    visitor: &[u8],
    day: NaiveDate,
) -> DbResult<bool> {
    let mut tx = pool.begin().await?;
    let inserted = sqlx::query(
        "INSERT INTO hit_dedupe (day, visitor_hash, game_id) VALUES ($1, $2, $3)
         ON CONFLICT DO NOTHING",
    )
    .bind(day)
    .bind(visitor.to_vec())
    .bind(game_id)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if inserted {
        sqlx::query(
            "INSERT INTO game_hits_daily (game_id, day, hits) VALUES ($1, $2, 1)
             ON CONFLICT (game_id, day) DO UPDATE SET hits = game_hits_daily.hits + 1",
        )
        .bind(game_id)
        .bind(day)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(inserted)
}

/// Rebuilds the score ranking. Uses CONCURRENTLY, so readers are not blocked.
pub async fn refresh_ranking(pool: &PgPool) -> DbResult<()> {
    sqlx::query("REFRESH MATERIALIZED VIEW CONCURRENTLY game_score_ranking")
        .execute(pool)
        .await?;
    Ok(())
}

/// Deletes dedupe rows and salts older than 2 days. Returns the total deleted.
pub async fn purge_old(pool: &PgPool) -> DbResult<u64> {
    let a = sqlx::query("DELETE FROM hit_dedupe WHERE day < current_date - 2")
        .execute(pool)
        .await?
        .rows_affected();
    let b = sqlx::query("DELETE FROM daily_salts WHERE day < current_date - 2")
        .execute(pool)
        .await?
        .rows_affected();
    Ok(a + b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_bots_case_insensitively() {
        assert!(is_bot_user_agent("Googlebot/2.1"));
        assert!(is_bot_user_agent("Mozilla/5.0 HeadlessChrome/120"));
        assert!(is_bot_user_agent(
            "Slackbot-LinkExpanding 1.0 (+https://api.slack.com)"
        ));
        assert!(!is_bot_user_agent(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) Safari/605.1.15"
        ));
    }

    #[test]
    fn visitor_hash_depends_on_every_input() {
        let a = visitor_hash(b"salt", "1.2.3.4", "ua");
        assert_eq!(a, visitor_hash(b"salt", "1.2.3.4", "ua"));
        assert_ne!(a, visitor_hash(b"salt2", "1.2.3.4", "ua"));
        assert_ne!(a, visitor_hash(b"salt", "1.2.3.5", "ua"));
        assert_ne!(a, visitor_hash(b"salt", "1.2.3.4", "ub"));
    }
}
