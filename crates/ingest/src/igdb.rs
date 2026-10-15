//! IGDB client and catalog import.
//!
//! Requests run one at a time with at least 300 ms between them. A 429 retries after 2 s, up to
//! three times. The access token comes from Twitch client credentials and is cached for the run.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::DateTime;
use recomphub_db::games::{self, GameFilter, IgdbGame, PlatformUpsert};
use recomphub_db::{PgPool, settings};
use recomphub_shared::normalize_title;
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio::time::{Instant, sleep, sleep_until};

use crate::platform_map;

const API_BASE: &str = "https://api.igdb.com/v4";
const TOKEN_URL: &str = "https://id.twitch.tv/oauth2/token";
const USER_AGENT: &str = "recomphub/1.0 (+https://recomphub.com)";
const MIN_INTERVAL: Duration = Duration::from_millis(300);
const RETRY_DELAY: Duration = Duration::from_secs(2);
const MAX_RETRIES: u32 = 3;
/// Page size for catalog pages and for `where id = (...)` batches.
pub const PAGE_SIZE: i64 = 500;
/// Games whose last IGDB sync is older than this many days are re-fetched.
const STALE_AFTER_DAYS: i32 = 30;
/// Refresh the token this many seconds before Twitch says it expires.
const TOKEN_MARGIN_SECS: u64 = 300;

/// Game type names that the catalog accepts. Ids are resolved by name at run time.
pub const ALLOWED_GAME_TYPES: &[&str] =
    &["Main Game", "Remake", "Remaster", "Expanded Game", "Port"];

const GAME_FIELDS: &str = "fields id,name,slug,summary,first_release_date,alternative_names.name,platforms,cover.image_id,screenshots.image_id,genres.name,involved_companies.company.name,involved_companies.developer,involved_companies.publisher,total_rating_count,game_type;";

const PLATFORM_FIELDS: &str = "fields id,name,slug,abbreviation,generation,platform_family.name;";

/// A game returned by a title search, with its IGDB game type.
#[derive(Debug, Clone)]
pub struct IgdbHit {
    pub game: IgdbGame,
    pub game_type_id: Option<i64>,
    /// Name of the game type (for example `Main Game`), when IGDB returned a known id.
    pub game_type_name: Option<String>,
}

/// Outcome of matching a source title to a local or IGDB game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchResult {
    Matched(i64),
    Ambiguous,
    NotFound,
}

/// Counts from one `catalog_sync` run.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CatalogReport {
    /// Games upserted from catalog pages.
    pub imported: usize,
    /// Games re-fetched because their last sync was older than 30 days.
    pub resynced: usize,
}

struct State {
    token: Option<(String, Instant)>,
    last_request: Option<Instant>,
    game_types: Option<HashMap<i64, String>>,
}

/// IGDB API client. Build it with `IgdbClient::from_env`.
pub struct IgdbClient {
    http: reqwest::Client,
    client_id: String,
    client_secret: String,
    state: Mutex<State>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

#[derive(Deserialize)]
struct RawNamed {
    name: String,
}

#[derive(Deserialize)]
struct RawImage {
    image_id: String,
}

#[derive(Deserialize)]
struct RawInvolved {
    company: Option<RawNamed>,
    #[serde(default)]
    developer: bool,
    #[serde(default)]
    publisher: bool,
}

#[derive(Deserialize)]
struct RawGame {
    id: i64,
    name: String,
    slug: String,
    summary: Option<String>,
    first_release_date: Option<i64>,
    #[serde(default)]
    alternative_names: Vec<RawNamed>,
    #[serde(default)]
    platforms: Vec<i32>,
    cover: Option<RawImage>,
    #[serde(default)]
    screenshots: Vec<RawImage>,
    #[serde(default)]
    genres: Vec<RawNamed>,
    #[serde(default)]
    involved_companies: Vec<RawInvolved>,
    total_rating_count: Option<i64>,
    game_type: Option<i64>,
}

#[derive(Deserialize)]
struct RawGameType {
    id: i64,
    #[serde(rename = "type")]
    name: String,
}

#[derive(Deserialize)]
struct RawPlatform {
    id: i32,
    name: String,
    slug: Option<String>,
    abbreviation: Option<String>,
    generation: Option<i32>,
    platform_family: Option<RawNamed>,
}

impl IgdbClient {
    /// Reads `TWITCH_CLIENT_ID` and `TWITCH_CLIENT_SECRET`. Fails when either is missing or empty.
    pub fn from_env() -> Result<Self> {
        Self::new(
            env_value("TWITCH_CLIENT_ID")?,
            env_value("TWITCH_CLIENT_SECRET")?,
        )
    }

    pub fn new(client_id: String, client_secret: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(30))
            .build()
            .context("building the IGDB HTTP client")?;
        Ok(Self {
            http,
            client_id,
            client_secret,
            state: Mutex::new(State {
                token: None,
                last_request: None,
                game_types: None,
            }),
        })
    }

    /// Searches games by title. Results are not filtered by game type.
    pub async fn search_games(&self, title: &str) -> Result<Vec<IgdbHit>> {
        self.search(title, None).await
    }

    /// Fetches one game by IGDB id, or `None` when IGDB does not return it.
    pub async fn fetch_game(&self, igdb_id: i64) -> Result<Option<IgdbGame>> {
        let text = self.query_text("games", &by_ids_body(&[igdb_id])).await?;
        Ok(parse_games(&text)?.into_iter().next())
    }

    pub(crate) async fn games_by_ids(&self, ids: &[i64]) -> Result<Vec<IgdbGame>> {
        let text = self.query_text("games", &by_ids_body(ids)).await?;
        parse_games(&text)
    }

    pub(crate) async fn games_page(&self, type_ids: &[i64], offset: i64) -> Result<Vec<IgdbGame>> {
        let text = self
            .query_text("games", &catalog_body(type_ids, offset))
            .await?;
        parse_games(&text)
    }

    pub(crate) async fn platforms_by_ids(&self, ids: &[i32]) -> Result<Vec<PlatformUpsert>> {
        let body = format!(
            "{PLATFORM_FIELDS} where id = ({}); limit {PAGE_SIZE};",
            join_ids(ids)
        );
        parse_platforms(&self.query_text("platforms", &body).await?)
    }

    pub(crate) async fn platforms_by_slugs(&self, slugs: &[&str]) -> Result<Vec<PlatformUpsert>> {
        let quoted: Vec<String> = slugs.iter().map(|s| format!("\"{s}\"")).collect();
        let body = format!(
            "{PLATFORM_FIELDS} where slug = ({}); limit {PAGE_SIZE};",
            quoted.join(",")
        );
        parse_platforms(&self.query_text("platforms", &body).await?)
    }

    /// Resolves the allowed game types by name. Fails listing the returned names when one is missing.
    pub async fn allowed_game_type_ids(&self) -> Result<Vec<i64>> {
        let types = self.game_types().await?;
        let mut ids = Vec::with_capacity(ALLOWED_GAME_TYPES.len());
        for wanted in ALLOWED_GAME_TYPES {
            match types.iter().find(|(_, name)| name.as_str() == *wanted) {
                Some((id, _)) => ids.push(*id),
                None => {
                    let mut names: Vec<&str> = types.values().map(String::as_str).collect();
                    names.sort_unstable();
                    bail!(
                        "IGDB has no game type named \"{wanted}\". Game types returned: {}",
                        names.join(", ")
                    );
                }
            }
        }
        Ok(ids)
    }

    async fn game_types(&self) -> Result<HashMap<i64, String>> {
        let cached = self.state.lock().await.game_types.clone();
        if let Some(types) = cached {
            return Ok(types);
        }
        let text = self
            .query_text("game_types", "fields id,type; limit 500;")
            .await?;
        let raw: Vec<RawGameType> =
            serde_json::from_str(&text).context("IGDB game_types response")?;
        let map: HashMap<i64, String> = raw.into_iter().map(|t| (t.id, t.name)).collect();
        self.state.lock().await.game_types = Some(map.clone());
        Ok(map)
    }

    async fn search(&self, title: &str, platform_igdb_id: Option<i32>) -> Result<Vec<IgdbHit>> {
        let clean = clean_search_title(title);
        if clean.is_empty() {
            bail!("IGDB search needs a non-empty title");
        }
        let text = self
            .query_text("games", &search_body(&clean, platform_igdb_id))
            .await?;
        let types = self.game_types().await?;
        let raw: Vec<RawGame> = serde_json::from_str(&text).context("IGDB games response")?;
        Ok(raw
            .into_iter()
            .map(|g| {
                let game_type_id = g.game_type;
                let game_type_name = game_type_id.and_then(|id| types.get(&id).cloned());
                IgdbHit {
                    game: g.into_game(),
                    game_type_id,
                    game_type_name,
                }
            })
            .collect())
    }

    /// POSTs to an IGDB endpoint and returns the raw response text. Serializes requests, enforces
    /// the spacing, and retries 429 responses.
    async fn query_text(&self, endpoint: &str, body: &str) -> Result<String> {
        let mut state = self.state.lock().await;
        let mut attempt = 0;
        loop {
            if let Some(last) = state.last_request {
                let next = last + MIN_INTERVAL;
                if next > Instant::now() {
                    sleep_until(next).await;
                }
            }
            state.last_request = Some(Instant::now());
            let token = self.access_token(&mut state).await?;
            let response = self
                .http
                .post(format!("{API_BASE}/{endpoint}"))
                .header("Client-ID", &self.client_id)
                .bearer_auth(&token)
                .header("Accept", "application/json")
                .body(body.to_owned())
                .send()
                .await
                .with_context(|| format!("IGDB {endpoint} request failed"))?;
            let status = response.status();
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS && attempt < MAX_RETRIES {
                attempt += 1;
                sleep(RETRY_DELAY).await;
                continue;
            }
            let text = response
                .text()
                .await
                .with_context(|| format!("reading IGDB {endpoint} response"))?;
            if !status.is_success() {
                bail!(
                    "IGDB {endpoint} returned HTTP {status}: {}",
                    truncate(&text, 300)
                );
            }
            return Ok(text);
        }
    }

    async fn access_token(&self, state: &mut State) -> Result<String> {
        if let Some((token, expires)) = &state.token
            && *expires > Instant::now()
        {
            return Ok(token.clone());
        }
        let response = self
            .http
            .post(TOKEN_URL)
            .query(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("grant_type", "client_credentials"),
            ])
            .send()
            .await
            .context("Twitch token request failed")?;
        let status = response.status();
        let text = response
            .text()
            .await
            .context("reading Twitch token response")?;
        if !status.is_success() {
            bail!(
                "Twitch rejected the IGDB credentials (HTTP {status}); check TWITCH_CLIENT_ID and TWITCH_CLIENT_SECRET"
            );
        }
        let parsed: TokenResponse =
            serde_json::from_str(&text).context("Twitch token response was not understood")?;
        let lifetime = parsed.expires_in.saturating_sub(TOKEN_MARGIN_SECS).max(60);
        state.token = Some((
            parsed.access_token.clone(),
            Instant::now() + Duration::from_secs(lifetime),
        ));
        Ok(parsed.access_token)
    }
}

fn env_value(name: &str) -> Result<String> {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(value.trim().to_string()),
        _ => bail!(
            "{name} is not set; IGDB access needs a Twitch application client id and secret (see .env.example)"
        ),
    }
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

fn join_ids<T: std::fmt::Display>(ids: &[T]) -> String {
    ids.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// Strips double quotes and collapses whitespace, because IGDB search strings are quoted.
pub fn clean_search_title(title: &str) -> String {
    title
        .replace('"', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Request body for one catalog page: allowed game types, ranked by rating count.
pub fn catalog_body(type_ids: &[i64], offset: i64) -> String {
    format!(
        "{GAME_FIELDS} where game_type = ({}) & total_rating_count > 0 & version_parent = null; sort total_rating_count desc; limit {PAGE_SIZE}; offset {offset};",
        join_ids(type_ids)
    )
}

/// Request body that fetches games by id.
pub fn by_ids_body(ids: &[i64]) -> String {
    format!(
        "{GAME_FIELDS} where id = ({}); limit {PAGE_SIZE};",
        join_ids(ids)
    )
}

/// Request body for a title search, optionally limited to one IGDB platform.
pub fn search_body(title: &str, platform_igdb_id: Option<i32>) -> String {
    let platform = platform_igdb_id
        .map(|id| format!(" where platforms = ({id});"))
        .unwrap_or_default();
    format!("search \"{title}\"; {GAME_FIELDS}{platform} limit 10;")
}

/// True when `target` (already normalized) equals the normalized name or an alternative name.
pub fn title_matches(target: &str, name: &str, alt_names: &[String]) -> bool {
    normalize_title(name) == target || alt_names.iter().any(|alt| normalize_title(alt) == target)
}

/// Parses IGDB `games` JSON into catalog records.
pub fn parse_games(text: &str) -> Result<Vec<IgdbGame>> {
    let raw: Vec<RawGame> = serde_json::from_str(text).context("IGDB games response")?;
    Ok(raw.into_iter().map(RawGame::into_game).collect())
}

/// Parses IGDB `platforms` JSON into platform upserts.
pub fn parse_platforms(text: &str) -> Result<Vec<PlatformUpsert>> {
    let raw: Vec<RawPlatform> = serde_json::from_str(text).context("IGDB platforms response")?;
    Ok(raw
        .into_iter()
        .map(|p| PlatformUpsert {
            igdb_id: p.id,
            slug: p.slug,
            name: p.name,
            abbreviation: p.abbreviation,
            family: p.platform_family.map(|f| f.name),
            generation: p.generation,
        })
        .collect())
}

fn image_url(size: &str, image_id: &str) -> String {
    format!("https://images.igdb.com/igdb/image/upload/{size}/{image_id}.jpg")
}

fn company_names(companies: &[RawInvolved], keep: fn(&RawInvolved) -> bool) -> Option<String> {
    let mut names: Vec<String> = Vec::new();
    for company in companies.iter().filter(|c| keep(c)) {
        if let Some(name) = company.company.as_ref().map(|c| c.name.clone())
            && !names.contains(&name)
        {
            names.push(name);
        }
    }
    (!names.is_empty()).then(|| names.join(", "))
}

impl RawGame {
    fn into_game(self) -> IgdbGame {
        let developer = company_names(&self.involved_companies, |c| c.developer);
        let publisher = company_names(&self.involved_companies, |c| c.publisher);
        let screenshot_urls: Vec<String> = self
            .screenshots
            .iter()
            .take(6)
            .map(|s| image_url("t_1080p", &s.image_id))
            .collect();
        IgdbGame {
            igdb_id: self.id,
            name: self.name,
            slug: self.slug,
            alt_names: self.alternative_names.into_iter().map(|n| n.name).collect(),
            summary: self.summary,
            first_release_date: self
                .first_release_date
                .and_then(|ts| DateTime::from_timestamp(ts, 0))
                .map(|d| d.date_naive()),
            developer,
            publisher,
            genres: self.genres.into_iter().map(|g| g.name).collect(),
            popularity: self
                .total_rating_count
                .unwrap_or(0)
                .clamp(0, i64::from(i32::MAX)) as i32,
            platform_igdb_ids: self.platforms,
            cover_url: self.cover.map(|c| image_url("t_cover_big_2x", &c.image_id)),
            og_url: screenshot_urls.first().cloned(),
            screenshot_urls,
        }
    }
}

/// Ensures every platform id in `ids` is in the `platforms` table, fetching missing ones from IGDB.
pub async fn ensure_platforms(pool: &PgPool, client: &IgdbClient, ids: &[i32]) -> Result<()> {
    let known = games::known_igdb_platform_ids(pool).await?;
    let mut missing: Vec<i32> = Vec::new();
    for id in ids {
        if !known.contains(id) && !missing.contains(id) {
            missing.push(*id);
        }
    }
    if missing.is_empty() {
        return Ok(());
    }
    let fetched = client.platforms_by_ids(&missing).await?;
    for platform in &fetched {
        games::upsert_platform(pool, platform).await?;
    }
    for id in &missing {
        if !fetched.iter().any(|p| p.igdb_id == *id) {
            tracing::warn!(
                igdb_platform_id = id,
                "IGDB returned no platform for this id"
            );
        }
    }
    Ok(())
}

/// Upserts a batch of games in one transaction, after making sure their platforms exist.
/// Returns how many games were written.
async fn upsert_batch(pool: &PgPool, client: &IgdbClient, batch: &[IgdbGame]) -> Result<usize> {
    let mut platform_ids: Vec<i32> = batch
        .iter()
        .flat_map(|g| g.platform_igdb_ids.iter().copied())
        .collect();
    platform_ids.sort_unstable();
    platform_ids.dedup();
    ensure_platforms(pool, client, &platform_ids).await?;
    let mut tx = pool.begin().await?;
    for game in batch {
        games::sync_igdb(&mut tx, game).await?;
    }
    tx.commit().await?;
    Ok(batch.len())
}

/// Imports one game by IGDB id through the catalog code path. Returns the local game id, or `None`
/// when IGDB has no such game.
pub async fn import_game(pool: &PgPool, client: &IgdbClient, igdb_id: i64) -> Result<Option<i64>> {
    let Some(game) = client.fetch_game(igdb_id).await? else {
        return Ok(None);
    };
    ensure_platforms(pool, client, &game.platform_igdb_ids).await?;
    let mut tx = pool.begin().await?;
    let id = games::sync_igdb(&mut tx, &game).await?;
    tx.commit().await?;
    Ok(Some(id))
}

/// Catalog sync (plan §3). Pages through allowed game types, ranked by rating count, until the
/// local game count reaches `catalog_target_size`. Then re-fetches games whose last IGDB sync is
/// older than 30 days.
pub async fn catalog_sync(pool: &PgPool, client: &IgdbClient) -> Result<CatalogReport> {
    let target = settings::get_i64(pool, "catalog_target_size", 5000).await?;
    let type_ids = client.allowed_game_type_ids().await?;
    let mut report = CatalogReport::default();
    let mut offset = 0;
    let mut count = games::count(pool, &GameFilter::default()).await?;
    while count < target {
        let batch = client.games_page(&type_ids, offset).await?;
        if batch.is_empty() {
            break;
        }
        report.imported += upsert_batch(pool, client, &batch).await?;
        count = games::count(pool, &GameFilter::default()).await?;
        if (batch.len() as i64) < PAGE_SIZE {
            break;
        }
        offset += PAGE_SIZE;
    }

    let stale: Vec<i64> = sqlx::query_scalar(
        "SELECT igdb_id FROM games
         WHERE igdb_synced_at IS NULL OR igdb_synced_at < now() - ($1::int * interval '1 day')
         ORDER BY igdb_synced_at NULLS FIRST, id",
    )
    .bind(STALE_AFTER_DAYS)
    .fetch_all(pool)
    .await?;
    for chunk in stale.chunks(PAGE_SIZE as usize) {
        let batch = client.games_by_ids(chunk).await?;
        report.resynced += upsert_batch(pool, client, &batch).await?;
    }
    Ok(report)
}

/// Resolves every slug in `platform_map::IGDB_SLUGS` through IGDB, stores the platforms, and
/// returns a map from slug to IGDB platform id. Fails listing the slugs that IGDB does not know.
pub async fn resolve_platform_slugs(
    pool: &PgPool,
    client: &IgdbClient,
) -> Result<HashMap<String, i32>> {
    let fetched = client.platforms_by_slugs(platform_map::IGDB_SLUGS).await?;
    let mut map = HashMap::new();
    for platform in &fetched {
        if let Some(slug) = &platform.slug {
            map.insert(slug.clone(), platform.igdb_id);
        }
        games::upsert_platform(pool, platform).await?;
    }
    let missing: Vec<&str> = platform_map::IGDB_SLUGS
        .iter()
        .copied()
        .filter(|slug| !map.contains_key(*slug))
        .collect();
    if !missing.is_empty() {
        bail!(
            "IGDB has no platform for these slugs: {}. Correct platform_map against IGDB's /platforms output.",
            missing.join(", ")
        );
    }
    Ok(map)
}

/// Matches a source title to a local game, then to IGDB (plan §4).
///
/// Local rows are tried first, by normalized name or alternative name, restricted to games on
/// the platform. Otherwise IGDB is searched on the platform, and exact normalized matches are
/// resolved: one match is upserted and matched, several resolve to the highest-rated Main Game,
/// and none in that case gives `Ambiguous`.
pub async fn match_game(
    pool: &PgPool,
    client: &IgdbClient,
    title: &str,
    platform_igdb_id: i32,
) -> Result<MatchResult> {
    let target = normalize_title(title);
    if target.is_empty() {
        return Ok(MatchResult::NotFound);
    }

    let local: Vec<(i64, String, Vec<String>)> = sqlx::query_as(
        "SELECT g.id, g.name, g.alt_names
         FROM games g
         JOIN game_platforms gp ON gp.game_id = g.id
         JOIN platforms p ON p.id = gp.platform_id
         WHERE p.igdb_id = $1
         ORDER BY g.popularity DESC, g.id",
    )
    .bind(platform_igdb_id)
    .fetch_all(pool)
    .await?;
    if let Some((id, _, _)) = local
        .iter()
        .find(|(_, name, alts)| title_matches(&target, name, alts))
    {
        return Ok(MatchResult::Matched(*id));
    }

    let hits = client.search(title, Some(platform_igdb_id)).await?;
    let exact: Vec<&IgdbHit> = hits
        .iter()
        .filter(|h| title_matches(&target, &h.game.name, &h.game.alt_names))
        .collect();
    let chosen = match exact.as_slice() {
        [] => return Ok(MatchResult::NotFound),
        [only] => *only,
        several => match several
            .iter()
            .filter(|h| h.game_type_name.as_deref() == Some("Main Game"))
            .max_by_key(|h| h.game.popularity)
        {
            Some(main) => *main,
            None => return Ok(MatchResult::Ambiguous),
        },
    };
    let id = import_hit(pool, client, &chosen.game).await?;
    Ok(MatchResult::Matched(id))
}

async fn import_hit(pool: &PgPool, client: &IgdbClient, game: &IgdbGame) -> Result<i64> {
    ensure_platforms(pool, client, &game.platform_igdb_ids).await?;
    let mut tx = pool.begin().await?;
    let id = games::sync_igdb(&mut tx, game).await?;
    tx.commit().await?;
    Ok(id)
}

/// Local game ids on one IGDB platform. Used by callers that need every candidate.
pub async fn local_games_on_platform(
    pool: &PgPool,
    platform_igdb_id: i32,
) -> Result<Vec<(i64, String)>> {
    sqlx::query_as(
        "SELECT g.id, g.name FROM games g
         JOIN game_platforms gp ON gp.game_id = g.id
         JOIN platforms p ON p.id = gp.platform_id
         WHERE p.igdb_id = $1 ORDER BY g.id",
    )
    .bind(platform_igdb_id)
    .fetch_all(pool)
    .await
    .map_err(|e| anyhow!("listing games on platform {platform_igdb_id}: {e}"))
}
