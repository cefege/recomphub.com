//! YouTube Data API v3 search for gameplay videos, one query per `(game, target)` pair.
//!
//! `run_youtube_sync` (`worker youtube-sync`) spends at most `youtube_daily_searches` searches per
//! run. A quota error stops the run cleanly and leaves the unfinished pairs due for the next run.

use std::collections::HashSet;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use recomphub_db::PgPool;
use recomphub_db::media;
use recomphub_db::settings;
use recomphub_db::videos::{self, NewVideo, YoutubeCandidate};
use recomphub_shared::normalize_title;
use serde::Deserialize;

const SEARCH_URL: &str = "https://www.googleapis.com/youtube/v3/search";
const USER_AGENT: &str = "recomphub/1.0 (+https://recomphub.com)";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Used when the `youtube_daily_searches` setting is missing.
const DEFAULT_DAILY_SEARCHES: i64 = 90;
/// Each `search.list` call costs this many units of the default 10,000-unit daily quota.
pub const UNITS_PER_SEARCH: u32 = 100;
const MAX_RESULTS: u32 = 5;
/// Videos kept per query, after the title filter.
pub const KEEP_PER_QUERY: usize = 3;
/// Shortest name token that counts when matching a video title to a game name.
const MIN_TOKEN_LEN: usize = 3;

/// One video returned by a search.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoHit {
    pub youtube_id: String,
    pub title: String,
    pub channel: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
}

/// Result of one search call.
#[derive(Debug, Clone, PartialEq)]
pub enum SearchOutcome {
    Hits(Vec<VideoHit>),
    QuotaExceeded,
}

/// Counts for one `youtube-sync` run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct YoutubeReport {
    /// Searches that returned results.
    pub searches: usize,
    /// Videos upserted.
    pub stored: usize,
    /// Set when YouTube reported the daily quota as exhausted.
    pub quota_exhausted: bool,
}

/// The search query for a target, from plan §7. `port` is the game's most-starred published port.
pub fn query_for(target: &str, game: &str, port: &str) -> Result<String> {
    Ok(match target {
        "windows" => format!("{game} {port} PC gameplay"),
        "macos" => format!("{game} {port} Mac"),
        "linux" => format!("{game} {port} Linux Steam Deck"),
        "web" => format!("{game} browser port"),
        "android" => format!("{game} {port} Android"),
        "ios" => format!("{game} iOS port"),
        "any" => format!("{game} recomp"),
        other => bail!("no YouTube query template for target {other:?}"),
    })
}

/// Normalized name tokens of length [`MIN_TOKEN_LEN`] or more.
fn tokens(s: &str) -> HashSet<String> {
    normalize_title(s)
        .split_whitespace()
        .filter(|t| t.chars().count() >= MIN_TOKEN_LEN)
        .map(str::to_string)
        .collect()
}

/// True when the video title shares at least one normalized token (length 3 or more) with the
/// game name.
pub fn title_matches(title: &str, game_name: &str) -> bool {
    let game = tokens(game_name);
    tokens(title).iter().any(|t| game.contains(t))
}

/// Thumbnail URL for a video id.
pub fn thumb_url(youtube_id: &str) -> String {
    format!("https://i.ytimg.com/vi/{youtube_id}/hqdefault.jpg")
}

/// Whether a failed response is YouTube's daily quota error (HTTP 403, reason `quotaExceeded`).
pub fn is_quota_error(status: u16, body: &str) -> bool {
    #[derive(Deserialize)]
    struct ErrorBody {
        error: ErrorDetail,
    }
    #[derive(Deserialize)]
    struct ErrorDetail {
        #[serde(default)]
        errors: Vec<ErrorReason>,
    }
    #[derive(Deserialize)]
    struct ErrorReason {
        reason: String,
    }
    if status != 403 {
        return false;
    }
    serde_json::from_str::<ErrorBody>(body)
        .map(|b| b.error.errors.iter().any(|e| e.reason == "quotaExceeded"))
        .unwrap_or(false)
}

#[derive(Deserialize)]
struct SearchBody {
    #[serde(default)]
    items: Vec<SearchItem>,
}

#[derive(Deserialize)]
struct SearchItem {
    id: ItemId,
    snippet: Snippet,
}

#[derive(Deserialize)]
struct ItemId {
    #[serde(rename = "videoId")]
    video_id: Option<String>,
}

#[derive(Deserialize)]
struct Snippet {
    title: String,
    #[serde(rename = "channelTitle", default)]
    channel_title: Option<String>,
    #[serde(rename = "publishedAt", default)]
    published_at: Option<String>,
}

/// Parses a `search.list` response body. Results without a video id are dropped.
pub fn parse_search(body: &str) -> Result<Vec<VideoHit>> {
    let parsed: SearchBody =
        serde_json::from_str(body).context("parsing YouTube search response")?;
    let mut hits = Vec::with_capacity(parsed.items.len());
    for item in parsed.items {
        let Some(youtube_id) = item.id.video_id.filter(|v| !v.is_empty()) else {
            continue;
        };
        let published_at = item
            .snippet
            .published_at
            .as_deref()
            .map(|s| {
                DateTime::parse_from_rfc3339(s)
                    .map(|d| d.with_timezone(&Utc))
                    .with_context(|| format!("invalid YouTube publishedAt {s}"))
            })
            .transpose()?;
        hits.push(VideoHit {
            youtube_id,
            title: item.snippet.title,
            channel: item.snippet.channel_title.filter(|c| !c.is_empty()),
            published_at,
        });
    }
    Ok(hits)
}

/// Runs one `search.list` call. The API key is redacted from any transport error.
pub async fn search(http: &reqwest::Client, api_key: &str, query: &str) -> Result<SearchOutcome> {
    let max_results = MAX_RESULTS.to_string();
    let resp = http
        .get(SEARCH_URL)
        .query(&[
            ("part", "snippet"),
            ("type", "video"),
            ("videoEmbeddable", "true"),
            ("maxResults", max_results.as_str()),
            ("safeSearch", "moderate"),
            ("q", query),
            ("key", api_key),
        ])
        .send()
        .await
        .map_err(|e| anyhow!("YouTube search request failed: {}", e.without_url()))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| anyhow!("reading YouTube search response: {}", e.without_url()))?;
    if is_quota_error(status.as_u16(), &body) {
        return Ok(SearchOutcome::QuotaExceeded);
    }
    if !status.is_success() {
        let snippet: String = body.chars().take(200).collect();
        bail!("YouTube search returned HTTP {status}: {snippet}");
    }
    Ok(SearchOutcome::Hits(parse_search(&body)?))
}

/// Searches for every due `(game, target)` pair, up to the daily budget, and stores matching videos.
///
/// Each searched pair is marked run, even when it finds nothing, so it is not searched again for
/// 30 days. A quota error stops the run without marking the pair that hit it.
pub async fn run_youtube_sync(pool: &PgPool, api_key: &str) -> Result<YoutubeReport> {
    let budget = settings::get_i64(pool, "youtube_daily_searches", DEFAULT_DAILY_SEARCHES).await?;
    let mut report = YoutubeReport::default();
    if budget <= 0 {
        return Ok(report);
    }
    let http = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .context("building the YouTube HTTP client")?;
    let candidates = videos::youtube_candidates(pool, budget).await?;
    let mut calls: i64 = 0;
    for candidate in candidates {
        if calls >= budget {
            break;
        }
        let Some(port) = candidate.port_name.as_deref() else {
            tracing::warn!(
                game_id = candidate.game_id,
                "game has no published port name; skipping"
            );
            continue;
        };
        let query = query_for(&candidate.target, &candidate.game_name, port)?;
        calls += 1;
        match search(&http, api_key, &query).await? {
            SearchOutcome::QuotaExceeded => {
                tracing::warn!("YouTube daily quota exhausted; stopping youtube-sync");
                report.quota_exhausted = true;
                break;
            }
            SearchOutcome::Hits(hits) => {
                report.searches += 1;
                report.stored += store_hits(pool, &candidate, hits).await?;
                videos::mark_query_run(pool, candidate.game_id, &candidate.target).await?;
            }
        }
    }
    Ok(report)
}

/// Stores the first [`KEEP_PER_QUERY`] hits whose title matches the game. Returns how many were stored.
async fn store_hits(
    pool: &PgPool,
    candidate: &YoutubeCandidate,
    hits: Vec<VideoHit>,
) -> Result<usize> {
    let kept: Vec<VideoHit> = hits
        .into_iter()
        .filter(|h| title_matches(&h.title, &candidate.game_name))
        .take(KEEP_PER_QUERY)
        .collect();
    for hit in &kept {
        let thumb = media::enqueue(pool, "video_thumb", &thumb_url(&hit.youtube_id)).await?;
        videos::upsert_video(
            pool,
            &NewVideo {
                game_id: candidate.game_id,
                port_id: None,
                target: candidate.target.clone(),
                youtube_id: hit.youtube_id.clone(),
                title: hit.title.clone(),
                channel: hit.channel.clone(),
                published_at: hit.published_at,
                thumb_media_id: Some(thumb),
            },
        )
        .await?;
    }
    Ok(kept.len())
}
