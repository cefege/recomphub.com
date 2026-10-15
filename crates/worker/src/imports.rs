//! Seed importers: Recompendium (GitHub tarball) and PortsDR (HTML) turned into published ports,
//! fills of existing ports, or `candidate` submissions when the game cannot be matched.

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use recomphub_db::PgPool;
use recomphub_db::games::platform_by_igdb_id;
use recomphub_db::ports::{
    AuthorInput, PortFull, PortInput, add_source, fill_empty, find_by_repo_url, insert_port,
    set_credited_authors,
};
use recomphub_db::submissions::add_candidate;
use recomphub_ingest::igdb::{self, IgdbClient, MatchResult};
use recomphub_ingest::platform_map::igdb_slug_for;
use recomphub_ingest::portsdr::{PortsDrCard, parse_cards};
use recomphub_ingest::recompendium::{RecompEntry, parse_entry};
use recomphub_shared::{AiSource, AiUsage, Playability, TargetPlatform};
use serde_json::{Value, json};
use tracing::warn;

const TARBALL_URL: &str = "https://api.github.com/repos/nio03/unricopie/tarball";
const PORTSDR_URL: &str = "https://portsdr.com/";
const USER_AGENT: &str = "recomphub/1.0 (+https://recomphub.com)";
const HTTP_TIMEOUT: Duration = Duration::from_secs(120);

/// One source record normalized to the port shape, before matching.
#[derive(Debug, Clone)]
pub struct SourcePort {
    /// Import tag appended to `ports.sources`, for example `import:portsdr`.
    pub source: &'static str,
    pub game_title: String,
    /// The original console or platform text, kept for moderators.
    pub console: Option<String>,
    /// IGDB slug of the original platform, when the platform map knows it.
    pub platform_slug: Option<&'static str>,
    pub port: PortInput,
    pub authors: Vec<AuthorInput>,
}

/// What `place` did with one source record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// An existing port with the same repo received NULL or empty fields.
    Filled(i64),
    /// A new port was published directly under a matched game.
    Published(i64),
    /// The record went to the moderation queue. `inserted` is false when a candidate already existed.
    Candidate { inserted: bool },
}

/// Counts for one import run.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ImportReport {
    pub seen: u32,
    pub filled: u32,
    pub published: u32,
    pub candidates: u32,
    pub duplicates: u32,
    pub skipped: u32,
}

impl ImportReport {
    fn record(&mut self, placement: Placement) {
        match placement {
            Placement::Filled(_) => self.filled += 1,
            Placement::Published(_) => self.published += 1,
            Placement::Candidate { inserted: true } => self.candidates += 1,
            Placement::Candidate { inserted: false } => self.duplicates += 1,
        }
    }
}

/// Maps a Recompendium entry to a source record. `ai_usage` and `playability` are set explicitly.
pub fn recompendium_source(entry: &RecompEntry) -> SourcePort {
    let authors = entry
        .authors
        .iter()
        .map(|name| AuthorInput {
            github_login: None,
            name: name.clone(),
            avatar_media_id: None,
            role: "credited".to_string(),
            commits: None,
        })
        .collect();
    SourcePort {
        source: "import:recompendium",
        game_title: entry.game_title.clone(),
        console: entry.source_platform.clone(),
        platform_slug: entry.source_platform.as_deref().and_then(igdb_slug_for),
        port: PortInput {
            name: entry.port_name.clone(),
            kind: entry.kind.as_str().to_string(),
            repo_url: entry.repo_url.clone(),
            homepage_url: entry.homepage_url.clone(),
            import_description: entry.import_description.clone(),
            requirements: entry.requirements.clone(),
            toolchain: entry.toolchain.clone(),
            license: entry.license.clone(),
            playability: entry.playability.as_str().to_string(),
            ai_usage: entry.ai_usage.as_str().to_string(),
            ai_source: entry.ai_source.as_ref().map(|s| s.as_str().to_string()),
            ..PortInput::default()
        },
        authors,
    }
}

/// Maps a PortsDR card to a source record. Cards carry no authors and no description.
pub fn portsdr_source(card: &PortsDrCard) -> SourcePort {
    let (ai_usage, ai_source) = if card.ai_assisted {
        (AiUsage::Assisted, Some(AiSource::Imported))
    } else {
        (AiUsage::Unknown, None)
    };
    SourcePort {
        source: "import:portsdr",
        game_title: card.game_title.clone(),
        console: card.original_platform.clone(),
        platform_slug: card.original_platform.as_deref().and_then(igdb_slug_for),
        port: PortInput {
            name: card.port_name.clone(),
            kind: card.kind.as_str().to_string(),
            repo_url: card.repo_url.clone(),
            declared_platforms: card
                .declared_platforms
                .iter()
                .map(|p: &TargetPlatform| p.as_str().to_string())
                .collect(),
            playability: Playability::Unknown.as_str().to_string(),
            ai_usage: ai_usage.as_str().to_string(),
            ai_source: ai_source.map(|s| s.as_str().to_string()),
            ..PortInput::default()
        },
        authors: Vec::new(),
    }
}

/// Places one source record: fills an existing port, publishes under a matched game, or queues a candidate.
pub async fn place(
    pool: &PgPool,
    client: &IgdbClient,
    platforms: &HashMap<String, i32>,
    src: SourcePort,
) -> Result<Placement> {
    if let Some(port_id) = find_by_repo_url(pool, &src.port.repo_url)
        .await
        .context("looking up port by repo")?
        .map(|row| row.id)
    {
        let mut tx = pool.begin().await.context("beginning fill transaction")?;
        fill_empty(&mut tx, port_id, &src.port).await?;
        add_source(&mut tx, port_id, src.source).await?;
        if !src.authors.is_empty() {
            set_credited_authors(&mut tx, port_id, &src.authors).await?;
        }
        tx.commit().await.context("committing fill transaction")?;
        return Ok(Placement::Filled(port_id));
    }

    let Some(slug) = src.platform_slug else {
        return queue_candidate(pool, &src).await;
    };
    let igdb_id = *platforms
        .get(slug)
        .with_context(|| format!("platform slug {slug} missing from resolved platforms"))?;
    match igdb::match_game(pool, client, &src.game_title, igdb_id).await? {
        MatchResult::Matched(game_id) => publish(pool, igdb_id, game_id, src).await,
        MatchResult::Ambiguous | MatchResult::NotFound => queue_candidate(pool, &src).await,
    }
}

async fn publish(
    pool: &PgPool,
    igdb_id: i32,
    game_id: i64,
    mut src: SourcePort,
) -> Result<Placement> {
    let platform = platform_by_igdb_id(pool, igdb_id)
        .await
        .context("looking up source platform")?
        .with_context(|| format!("platform igdb id {igdb_id} is not in the platforms table"))?;
    src.port.source_platform_id = Some(platform.id);
    let full = PortFull {
        input: src.port,
        authors: src.authors,
    };
    let sources = [src.source.to_string()];
    let mut tx = pool
        .begin()
        .await
        .context("beginning publish transaction")?;
    let port_id = insert_port(&mut tx, game_id, &full, &sources).await?;
    tx.commit()
        .await
        .context("committing publish transaction")?;
    Ok(Placement::Published(port_id))
}

async fn queue_candidate(pool: &PgPool, src: &SourcePort) -> Result<Placement> {
    let payload = candidate_payload(src)?;
    let inserted = add_candidate(pool, src.source, &payload).await?;
    Ok(Placement::Candidate { inserted })
}

/// A candidate payload uses the `new_port` shape (flattened `PortInput` plus `authors`), so approval
/// can deserialize it as `PortFull`. The extra keys are for the moderator.
fn candidate_payload(src: &SourcePort) -> Result<Value> {
    let full = PortFull {
        input: src.port.clone(),
        authors: src.authors.clone(),
    };
    let mut value = serde_json::to_value(&full).context("serializing candidate payload")?;
    let obj = value
        .as_object_mut()
        .context("candidate payload is not a JSON object")?;
    obj.insert("game_title".to_string(), json!(src.game_title));
    obj.insert("console".to_string(), json!(src.console));
    obj.insert("needs_game_match".to_string(), json!(true));
    Ok(value)
}

/// Extracts the Recompendium YAML files we import from the GitHub tarball. The tarball has one
/// top-level directory, which is stripped. Only `src/data/{recomps,ports,decomps}/*.yaml` is kept.
pub fn recompendium_files(tarball: &[u8]) -> Result<Vec<(String, String)>> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(tarball));
    let mut out = Vec::new();
    for entry in archive.entries().context("reading tarball entries")? {
        let mut entry = entry.context("reading tarball entry")?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let raw = entry
            .path()
            .context("reading tarball entry path")?
            .into_owned();
        let rel: PathBuf = raw.components().skip(1).collect();
        let Some(rel) = rel.to_str() else {
            continue;
        };
        if !is_recompendium_yaml(rel) {
            continue;
        }
        let mut content = String::new();
        entry
            .read_to_string(&mut content)
            .with_context(|| format!("reading {rel} from tarball"))?;
        out.push((rel.to_string(), content));
    }
    Ok(out)
}

fn is_recompendium_yaml(rel: &str) -> bool {
    let parts: Vec<&str> = rel.split('/').collect();
    matches!(
        parts.as_slice(),
        ["src", "data", dir, file]
            if matches!(*dir, "recomps" | "ports" | "decomps") && file.ends_with(".yaml")
    )
}

/// Downloads the Recompendium repository tarball. The token is optional and only raises the rate limit.
pub async fn download_tarball(http: &reqwest::Client, token: Option<&str>) -> Result<Vec<u8>> {
    let mut req = http
        .get(TARBALL_URL)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json");
    if let Some(token) = token {
        req = req.bearer_auth(token);
    }
    let resp = req
        .send()
        .await
        .context("requesting the recompendium tarball")?
        .error_for_status()
        .context("GitHub refused the recompendium tarball request")?;
    let bytes = resp
        .bytes()
        .await
        .context("reading the recompendium tarball body")?;
    Ok(bytes.to_vec())
}

/// Shared HTTP client for the importers.
pub fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(HTTP_TIMEOUT)
        .build()
        .context("building the HTTP client")
}

/// Resolves console slugs to IGDB platform ids and makes sure each one is in the `platforms` table.
async fn prepare_platforms(pool: &PgPool, client: &IgdbClient) -> Result<HashMap<String, i32>> {
    let platforms = igdb::resolve_platform_slugs(pool, client).await?;
    let ids: Vec<i32> = platforms.values().copied().collect();
    igdb::ensure_platforms(pool, client, &ids).await?;
    Ok(platforms)
}

/// Imports the Recompendium catalog. Parse errors skip one file. Network and database errors stop the run.
pub async fn import_recompendium(
    pool: &PgPool,
    client: &IgdbClient,
    http: &reqwest::Client,
    github_token: Option<&str>,
) -> Result<ImportReport> {
    let platforms = prepare_platforms(pool, client).await?;
    let tarball = download_tarball(http, github_token).await?;
    let files = recompendium_files(&tarball)?;
    if files.is_empty() {
        bail!(
            "the recompendium tarball contained no src/data/{{recomps,ports,decomps}} YAML files"
        );
    }
    let mut report = ImportReport::default();
    for (path, yaml) in files {
        report.seen += 1;
        let entry = match parse_entry(&path, &yaml) {
            Ok(Some(entry)) => entry,
            Ok(None) => {
                report.skipped += 1;
                continue;
            }
            Err(e) => {
                warn!(path = %path, error = %format!("{e:#}"), "skipping unparseable recompendium file");
                report.skipped += 1;
                continue;
            }
        };
        let placement = place(pool, client, &platforms, recompendium_source(&entry)).await?;
        report.record(placement);
    }
    Ok(report)
}

/// Imports the PortsDR index page. Stops with an error when no cards parse, so a layout change is visible.
pub async fn import_portsdr(
    pool: &PgPool,
    client: &IgdbClient,
    http: &reqwest::Client,
) -> Result<ImportReport> {
    let platforms = prepare_platforms(pool, client).await?;
    let html = http
        .get(PORTSDR_URL)
        .send()
        .await
        .context("requesting portsdr.com")?
        .error_for_status()
        .context("portsdr.com returned an error status")?
        .text()
        .await
        .context("reading the portsdr.com body")?;
    let cards = parse_cards(&html);
    if cards.is_empty() {
        bail!("no port cards parsed from portsdr.com; the page layout may have changed");
    }
    let mut report = ImportReport::default();
    for card in &cards {
        report.seen += 1;
        let placement = place(pool, client, &platforms, portsdr_source(card)).await?;
        report.record(placement);
    }
    Ok(report)
}
