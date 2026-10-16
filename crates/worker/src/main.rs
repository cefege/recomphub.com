//! `worker`: scheduled jobs for RecompHub. Each subcommand is one CronJob in `deploy/production/workers.yaml`.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use recomphub_db::PgPool;
use recomphub_db::secrets::Sealer;
use recomphub_ingest::github::{self, GithubClient};
use recomphub_ingest::igdb::{self, IgdbClient};
use recomphub_ingest::{
    best_sellers, describe, digest, discover, game_faqs, importance, media, port_videos, youtube,
    ytdlp::YtDlp,
};
use recomphub_worker::imports;
use sqlx::postgres::PgPoolOptions;
use tracing::{error, info, warn};
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
    /// Import the reviewed seed list (`seed/curated-ports.json`) from a URL.
    ImportCurated {
        #[arg(long, default_value = imports::CURATED_URL)]
        url: String,
    },
    /// Refresh GitHub metadata, releases, authors and AI usage for stale ports.
    GithubSync,
    /// Re-derive port display names from the README text github-sync already stored. No GitHub calls.
    DeriveNames {
        /// Print every change and write nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Find new recomp repositories on GitHub search and r/recomps as candidates.
    Discover,
    /// Fetch YouTube gameplay videos per game and platform, within the daily search budget.
    YoutubeSync,
    /// Find videos of people playing each port, from README links and yt-dlp search (no API quota).
    PortVideos,
    /// Match Wikipedia best-selling lists to games and refresh `games.sales_score`.
    SalesSync {
        /// Fetch and parse the lists, print a sample of each, and write nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Generate descriptions for ports from their stored GitHub text, using the Anthropic Messages API.
    DescribePorts {
        /// Print the request the next run would send for the first port needing a description, and call nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Generate FAQs for game pages from their ports, using the AI providers configured at /admin.
    DescribeGames {
        /// Print the prompt and schema for one game (the first needing a FAQ, or --game), and call nothing.
        #[arg(long)]
        dry_run: bool,
        /// Game slug to preview with --dry-run.
        #[arg(long)]
        game: Option<String>,
    },
    /// Record today's platform parity snapshot and backfill earlier days missing from `stats_daily`.
    StatsSnapshot,
    /// What real members did since the last run: new comments, ratings, submissions and
    /// reports waiting or just decided. Nothing scheduled runs this; run it on request.
    Digest {
        /// Show everything since this many hours ago, and leave the "last run" marker alone,
        /// instead of advancing it from the stored value.
        #[arg(long)]
        hours: Option<i64>,
    },
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
        Command::ImportCurated { url } => {
            let pool = pool().await?;
            let client = IgdbClient::from_env()?;
            let http = imports::http_client()?;
            let report = imports::import_curated(&pool, &client, &http, &url).await?;
            info!(?report, "import-curated finished");
        }
        Command::GithubSync => {
            let pool = pool().await?;
            let client = GithubClient::from_env()?;
            let report = github::run_github_sync(&pool, &client).await?;
            info!(?report, "github-sync finished");
        }
        Command::DeriveNames { dry_run } => {
            let pool = pool().await?;
            let changes = github::rederive_names(&pool, dry_run).await?;
            if dry_run {
                for (id, old, new) in &changes {
                    println!("{id}\t{old}\t->\t{new}");
                }
            }
            info!(renamed = changes.len(), dry_run, "derive-names finished");
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
        Command::PortVideos => {
            let pool = pool().await?;
            let yt = YtDlp::from_env().await?;
            let report = port_videos::run_port_video_sync(&pool, &yt).await?;
            info!(?report, "port-videos finished");
        }
        Command::DescribePorts { dry_run } => {
            let pool = pool().await?;
            if dry_run {
                match describe::preview_prompt(&pool).await? {
                    Some((port_id, system, user)) => {
                        println!("port {port_id}");
                        println!("system:\n{system}");
                        println!("user:\n{user}");
                    }
                    None => println!("no port needs a description"),
                }
            } else {
                match Sealer::from_env() {
                    Ok(sealer) => {
                        let http = imports::http_client()?;
                        let report =
                            describe::describe_ports(&pool, &http, &sealer, describe::RUN_LIMIT)
                                .await?;
                        info!(?report, "describe-ports finished");
                    }
                    Err(e) => warn!(error = %e, "describe-ports skipped"),
                }
            }
        }
        Command::DescribeGames { dry_run, game } => {
            let pool = pool().await?;
            if dry_run {
                match game_faqs::preview(&pool, game.as_deref()).await? {
                    Some(p) => {
                        println!("game {}", p.game_id);
                        println!("system:\n{}", p.system);
                        println!("user:\n{}", p.user);
                        println!("schema:\n{}", p.schema);
                    }
                    None => println!("no game has a port or decompilation"),
                }
            } else {
                match Sealer::from_env() {
                    Ok(sealer) => {
                        let http = imports::http_client()?;
                        let report =
                            game_faqs::describe_games(&pool, &http, &sealer, game_faqs::RUN_LIMIT)
                                .await?;
                        info!(?report, "describe-games finished");
                    }
                    Err(e) => warn!(error = %e, "describe-games skipped"),
                }
            }
        }
        Command::StatsSnapshot => {
            let pool = pool().await?;
            let report = recomphub_db::stats::run_snapshot_now(&pool).await?;
            info!(?report, "stats-snapshot finished");
        }
        Command::Digest { hours } => {
            let pool = pool().await?;
            let since = hours.map(|h| chrono::Utc::now() - chrono::Duration::hours(h));
            let d = digest::run(&pool, since).await?;
            print_digest(&d);
        }
        Command::SalesSync { dry_run } => {
            if dry_run {
                let http = best_sellers::http_client()?;
                for fetched in best_sellers::fetch_all(&http).await {
                    match fetched.entries {
                        Ok(entries) => {
                            println!(
                                "{} ({:?}): {} entries",
                                fetched.list.title,
                                fetched.list.scope,
                                entries.len()
                            );
                            for entry in entries.iter().take(5) {
                                println!("  {entry:?}");
                            }
                        }
                        Err(err) => println!("{} FAILED: {err:#}", fetched.list.title),
                    }
                }
            } else {
                let pool = pool().await?;
                let client = IgdbClient::from_env()?;
                let report = importance::sales_sync(&pool, &client).await?;
                info!(?report, "sales-sync finished");
                if !report.failed.is_empty() {
                    bail!(
                        "sales-sync failed for {} list(s): {}",
                        report.failed.len(),
                        report.failed.join("; ")
                    );
                }
            }
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

fn site_url() -> String {
    optional_env("SITE_URL").unwrap_or_else(|| "https://recomphub.com".to_string())
}

/// Plain-text rendering of a digest: one section per kind, oldest first, with enough to act on
/// without opening the site. Empty sections are skipped; an entirely empty digest says so.
fn print_digest(d: &recomphub_ingest::digest::Digest) {
    let site = site_url();
    let who = |author: &str, email: &Option<String>| match email {
        Some(e) => format!("{author} <{e}>"),
        None => author.to_string(),
    };
    println!(
        "Since {} ({} ago)",
        d.since.format("%Y-%m-%d %H:%M UTC"),
        humanize_span(d.until - d.since)
    );
    if d.is_empty() {
        println!("Nothing new.");
        return;
    }
    if !d.comments.is_empty() {
        println!("\nComments ({}):", d.comments.len());
        for c in &d.comments {
            let body = if c.body.chars().count() > 140 {
                format!("{}…", c.body.chars().take(140).collect::<String>())
            } else {
                c.body.clone()
            };
            println!(
                "  {} on {} — \"{}\"\n    {site}/games/{}/{}",
                who(&c.author, &c.email),
                c.game_name,
                body,
                c.game_slug,
                c.port_slug
            );
        }
    }
    if !d.ratings.is_empty() {
        println!("\nRatings ({}):", d.ratings.len());
        for r in &d.ratings {
            println!(
                "  {} rated {} {}★\n    {site}/games/{}/{}",
                who(&r.author, &r.email),
                r.game_name,
                r.stars,
                r.game_slug,
                r.port_slug
            );
        }
    }
    if !d.pending_submissions.is_empty() {
        println!("\nWaiting for review ({}):", d.pending_submissions.len());
        for s in &d.pending_submissions {
            print_submission(s, &who);
        }
        println!("    {site}/moderation");
    }
    if !d.reviewed_submissions.is_empty() {
        println!(
            "\nDecided since last check ({}):",
            d.reviewed_submissions.len()
        );
        for s in &d.reviewed_submissions {
            print_submission(s, &who);
        }
    }
}

fn print_submission(
    s: &recomphub_ingest::digest::DigestSubmission,
    who: &impl Fn(&str, &Option<String>) -> String,
) {
    let repo = s
        .repo_url
        .as_deref()
        .map(|u| format!(" ({u})"))
        .unwrap_or_default();
    println!(
        "  #{} [{}/{}] {} — {}{repo}",
        s.id,
        s.kind,
        s.status,
        who(&s.author, &s.email),
        s.subject
    );
}

fn humanize_span(span: chrono::TimeDelta) -> String {
    let hours = span.num_hours();
    if hours < 1 {
        format!("{}m", span.num_minutes().max(0))
    } else if hours < 48 {
        format!("{hours}h")
    } else {
        format!("{}d", hours / 24)
    }
}
