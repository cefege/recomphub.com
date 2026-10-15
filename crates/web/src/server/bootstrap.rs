//! One-time server startup: database pool, migrations, the live hub, and the
//! periodic maintenance tasks. The deployment runs one replica, so each task runs once.

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use recomphub_db::{activity, hits, users_sessions};
use recomphub_shared::live::ServerMsg;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tokio::sync::OnceCell;

use super::{live, state};

const RANKING_EVERY: Duration = Duration::from_secs(5 * 60);
const PURGE_EVERY: Duration = Duration::from_secs(60 * 60);

static STARTED: OnceCell<()> = OnceCell::const_new();

/// Connects the pool, runs migrations, starts presence and activity, and spawns
/// the ranking and purge loops. Repeated calls return immediately after the first success.
pub async fn start(database_url: &str) -> Result<()> {
    STARTED
        .get_or_try_init(|| async { start_once(database_url).await })
        .await
        .map(|_| ())
}

async fn start_once(database_url: &str) -> Result<()> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await
        .context("connecting to Postgres")?;
    recomphub_db::migrate(&pool)
        .await
        .context("running database migrations")?;
    state::init_pool(pool).map_err(|_| anyhow!("the database pool was already initialized"))?;
    let pool = state::pool().map_err(|err| anyhow!("{err}"))?;

    live::start(pool);
    tokio::spawn(ranking_loop(pool));
    tokio::spawn(purge_loop(pool));
    Ok(())
}

/// Refreshes the popularity ranking every five minutes and tells open pages to re-fetch.
async fn ranking_loop(pool: &'static PgPool) {
    loop {
        tokio::time::sleep(RANKING_EVERY).await;
        match hits::refresh_ranking(pool).await {
            Ok(()) => live::broadcast(ServerMsg::RankingUpdated),
            Err(err) => tracing::error!(error = ?err, "refreshing the ranking failed"),
        }
    }
}

/// Deletes old dedupe rows, salts, activity and expired sessions every hour.
async fn purge_loop(pool: &'static PgPool) {
    loop {
        tokio::time::sleep(PURGE_EVERY).await;
        if let Err(err) = hits::purge_old(pool).await {
            tracing::error!(error = ?err, "purging old hit rows failed");
        }
        if let Err(err) = activity::purge_old(pool).await {
            tracing::error!(error = ?err, "purging old activity failed");
        }
        if let Err(err) = users_sessions::purge_expired_sessions(pool).await {
            tracing::error!(error = ?err, "purging expired sessions failed");
        }
    }
}
