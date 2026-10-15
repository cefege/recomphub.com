//! `worker`: scheduled jobs for RecompHub. Each subcommand is one CronJob in `deploy/production/workers.yaml`.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use recomphub_db::PgPool;
use recomphub_ingest::github::{self, GithubClient};
use recomphub_ingest::igdb::{self, IgdbClient};
use recomphub_ingest::{discover, media, youtube};
use recomphub_worker::imports;
use sqlx::postgres::PgPoolOptions;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "worker", version, about = "RecompHub scheduled jobs")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Fill the game catalog from IGDB up to `catalog_target_size`, and refresh entries older than 30 days.
    CatalogSync,
    /// Download pending images and write them as AVIF (and the og JPEG).
    MediaSync,
    /// Import the Recompendium catalog (nio03/unricopie) from its GitHub tarball.
    ImportRecompendium,
    /// Import the PortsDR index page.
    ImportPortsdr,
    /// Refresh GitHub metadata, releases, authors and AI usage for stale ports.
    GithubSync,
    /// Find new recomp repositories on GitHub search and r/recomps as candidates.
    Discover,
    /// Fetch YouTube gameplay videos per game and platform, within the daily search budget.
    YoutubeSync,
}

#[tokio::main]
async fn main() -> ExitCode {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let cli = Cli::parse();
    match run(cli.command).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error!(error = %format!("{e:#}"), "worker failed");
            ExitCode::FAILURE
        }
    }
}

async fn run(command: Command) -> Result<()> {
    match command {
        Command::CatalogSync => {
            let pool = pool().await?;
            let client = IgdbClient::from_env()?;
            let report = igdb::catalog_sync(&pool, &client).await?;
            info!(?report, "catalog-sync finished");
        }
        Command::MediaSync => {
            let pool = pool().await?;
            let dir = media_dir();
            let report = media::process_pending(&pool, &dir, media::BATCH_SIZE).await?;
            info!(?report, "media-sync finished");
        }
        Command::ImportRecompendium => {
            let pool = pool().await?;
            let client = IgdbClient::from_env()?;
            let http = imports::http_client()?;
            let token = optional_env("GITHUB_TOKEN");
            let report =
                imports::import_recompendium(&pool, &client, &http, token.as_deref()).await?;
            info!(?report, "import-recompendium finished");
        }
        Command::ImportPortsdr => {
            let pool = pool().await?;
            let client = IgdbClient::from_env()?;
            let http = imports::http_client()?;
            let report = imports::import_portsdr(&pool, &client, &http).await?;
            info!(?report, "import-portsdr finished");
        }
        Command::GithubSync => {
            let pool = pool().await?;
            let client = GithubClient::from_env()?;
            let report = github::run_github_sync(&pool, &client).await?;
            info!(?report, "github-sync finished");
        }
        Command::Discover => {
            let pool = pool().await?;
            let client = GithubClient::from_env()?;
            let report = discover::run_discover(&pool, &client).await?;
            info!(?report, "discover finished");
        }
        Command::YoutubeSync => {
            let api_key = required_env("YOUTUBE_API_KEY")?;
            let pool = pool().await?;
            let report = youtube::run_youtube_sync(&pool, &api_key).await?;
            info!(?report, "youtube-sync finished");
        }
    }
    Ok(())
}

/// Connects to `DATABASE_URL` and applies migrations. Migrations are safe to run from several jobs at once.
async fn pool() -> Result<PgPool> {
    let url = required_env("DATABASE_URL")?;
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&url)
        .await
        .context("connecting to DATABASE_URL")?;
    recomphub_db::migrate(&pool)
        .await
        .context("applying migrations")?;
    Ok(pool)
}

fn required_env(name: &str) -> Result<String> {
    optional_env(name)
        .with_context(|| format!("{name} is not set; add it to the environment or .env"))
}

fn optional_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn media_dir() -> PathBuf {
    PathBuf::from(optional_env("MEDIA_DIR").unwrap_or_else(|| "./data/media".to_string()))
}
