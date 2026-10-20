//! Bounded, asynchronous Google Organic discovery for port gameplay videos.
//!
//! This module deliberately never opens a YouTube watch page: Google supplies the candidate URL,
//! title, and snippet; only a published candidate's static `i.ytimg.com` thumbnail is downloaded.

use std::collections::{HashMap, HashSet};
use std::env;

use anyhow::{Context, Result, anyhow, bail};
use recomphub_db::PgPool;
use recomphub_db::media;
use recomphub_db::video_discovery::{
    self, DECISION_PENDING, DECISION_PUBLISHED, DiscoveryCandidate, DiscoveryPort,
    NewDiscoveryCandidate,
};
use serde_json::{Value, json};

use crate::video_links::is_youtube_id;

pub const DISCOVERY_LIMIT: usize = 100;
const API_BASE: &str = "https://api.dataforseo.com/v3/serp/google/organic";
const EXCLUDED_TITLE_WORDS: &[&str] = &[
    "trailer",
    "teaser",
    "announcement",
    "announce",
    "reveal",
    "promo",
    "live",
    "livestream",
    "shorts",
];

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MediaDiscoveryReport {
    pub expired: u64,
    pub collected: u32,
    pub submitted: u32,
    pub auto_published: u32,
    pub pending: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrganicResult {
    pub url: String,
    pub title: String,
    pub description: String,
    pub rank: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateDecision {
    Publish,
    Pending(String),
    Ignore,
}

pub struct DataForSeoClient {
    http: reqwest::Client,
    login: String,
    password: String,
}

impl DataForSeoClient {
    /// Reads credentials exclusively at runtime. Validation happens before callers can mutate the
    /// database, so a suspended/unconfigured CronJob cannot create local work accidentally.
    pub fn from_env() -> Result<Self> {
        let login = env::var("DATAFORSEO_LOGIN").context("DATAFORSEO_LOGIN is required")?;
        let password = env::var("DATAFORSEO_PASSWORD").context("DATAFORSEO_PASSWORD is required")?;
        if login.trim().is_empty() || password.trim().is_empty() {
            bail!("DATAFORSEO_LOGIN and DATAFORSEO_PASSWORD must not be empty");
        }
        let http = reqwest::Client::builder()
            .user_agent("recomphub/1.0 (+https://recomphub.com)")
            .build()
            .context("building DataForSEO client")?;
        Ok(Self { http, login, password })
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value> {
        let response = self
            .http
            .post(format!("{API_BASE}/{path}"))
            .basic_auth(&self.login, Some(&self.password))
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST DataForSEO {path}"))?
            .error_for_status()
            .with_context(|| format!("DataForSEO {path} returned an error"))?;
        response.json().await.context("decoding DataForSEO response")
    }

    pub async fn submit(&self, ports: &[DiscoveryPort]) -> Result<HashMap<i64, String>> {
        let payload = task_payloads(ports)?;
        let response = self.post("task_post", Value::Array(payload)).await?;
        parse_submitted_tasks(&response)
    }

    pub async fn ready_task_ids(&self) -> Result<HashSet<String>> {
        let response = self.post("tasks_ready", json!({})).await?;
        Ok(ready_task_ids(&response).into_iter().collect())
    }

    pub async fn results(&self, provider_task_id: &str) -> Result<Vec<OrganicResult>> {
        let response = self.post(&format!("task_get/{provider_task_id}"), json!({})).await?;
        parse_organic_results(&response)
    }
}

/// Plain first-page, normal-priority Google Organic requests. No paid query operators are used.
pub fn task_payloads(ports: &[DiscoveryPort]) -> Result<Vec<Value>> {
    if ports.is_empty() || ports.len() > DISCOVERY_LIMIT {
        bail!("DataForSEO task batches must contain 1 through {DISCOVERY_LIMIT} ports");
    }
    Ok(ports
        .iter()
        .map(|port| {
            json!({
                "keyword": query_for(port),
                "location_code": 2840,
                "language_code": "en",
                "device": "desktop",
                "depth": 10,
                "priority": "normal",
                "tag": format!("port:{}", port.port_id),
            })
        })
        .collect())
}

pub fn kind_term(kind: &str) -> &'static str {
    match kind {
        "static_recomp" | "native_layer" => "recompilation",
        "decompilation" | "decomp_port" => "decompilation",
        "reimplementation" | "fan_remake" | "source_port" => "reimplementation",
        _ => "recompilation",
    }
}

pub fn query_for(port: &DiscoveryPort) -> String {
    if normalized(&port.port_name) != normalized(&port.game_name) {
        format!("{} {} gameplay", port.game_name, port.port_name)
    } else {
        format!("{} {} gameplay", port.game_name, kind_term(&port.kind))
    }
}

fn port_id_from_tag(tag: &str) -> Option<i64> {
    tag.strip_prefix("port:")?.parse().ok()
}


/// Provider task IDs keyed by the local port tag. Responses without a successful task ID are
/// rejected instead of writing a task we cannot collect later.
pub fn parse_submitted_tasks(value: &Value) -> Result<HashMap<i64, String>> {
    let tasks = value
        .get("tasks")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("DataForSEO response has no tasks"))?;
    let mut result = HashMap::new();
    for task in tasks {
        let status = task.get("status_code").and_then(Value::as_i64).unwrap_or(0);
        if status != 20_000 {
            continue;
        }
        let tag = task.get("tag").and_then(Value::as_str).unwrap_or_default();
        let Some(port_id) = port_id_from_tag(tag) else { continue };
        let id = task
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("submitted DataForSEO task for port {port_id} has no id"))?;
        result.insert(port_id, id.to_string());
    }
    Ok(result)
}

/// IDs listed by the free Tasks Ready endpoint. Unknown account tasks are merely returned here;
/// the worker intersects them with its own submitted rows before any task is fetched.
pub fn ready_task_ids(value: &Value) -> Vec<String> {
    value
        .get("tasks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|task| task.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

pub fn parse_organic_results(value: &Value) -> Result<Vec<OrganicResult>> {
    let tasks = value
        .get("tasks")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("DataForSEO result response has no tasks"))?;
    let mut results = Vec::new();
    for task in tasks {
        for result in task.get("result").and_then(Value::as_array).into_iter().flatten() {
            for item in result.get("items").and_then(Value::as_array).into_iter().flatten() {
                if item.get("type").and_then(Value::as_str) != Some("organic") {
                    continue;
                }
                let (Some(url), Some(title)) = (
                    item.get("url").and_then(Value::as_str),
                    item.get("title").and_then(Value::as_str),
                ) else {
                    continue;
                };
                let rank = item
                    .get("rank_absolute")
                    .and_then(Value::as_i64)
                    .unwrap_or_default() as i32;
                results.push(OrganicResult {
                    url: url.to_string(),
                    title: title.to_string(),
                    description: item
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    rank,
                });
            }
        }
    }
    Ok(results)
}

/// Canonicalizes only a normal YouTube watch URL with a valid ID. Shorts, channels, embeds,
/// redirectors, and malformed IDs do not enter review.
pub fn canonical_youtube_watch_url(url: &str) -> Option<(String, String)> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?.trim_start_matches("www.");
    if host != "youtube.com" || parsed.path() != "/watch" {
        return None;
    }
    let id = parsed
        .query_pairs()
        .find(|(key, _)| key == "v")
        .map(|(_, value)| value.into_owned())?;
    is_youtube_id(&id).then(|| (id.clone(), format!("https://www.youtube.com/watch?v={id}")))
}

fn normalized(text: &str) -> String {
    text.to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn contains_name(text: &str, name: &str) -> bool {
    let text = format!(" {} ", normalized(text));
    let name = normalized(name);
    !name.is_empty() && text.contains(&format!(" {name} "))
}

fn contains_excluded_word(text: &str) -> bool {
    let normalized = normalized(text);
    let words: HashSet<&str> = normalized.split_whitespace().collect();
    EXCLUDED_TITLE_WORDS.iter().any(|word| words.contains(word))
}

pub fn qualify(port: &DiscoveryPort, result: &OrganicResult) -> CandidateDecision {
    let Some(_) = canonical_youtube_watch_url(&result.url) else {
        return CandidateDecision::Ignore;
    };
    let text = format!("{} {}", result.title, result.description);
    if contains_excluded_word(&text) {
        return CandidateDecision::Ignore;
    }
    if !(1..=3).contains(&result.rank) {
        return CandidateDecision::Pending("Google rank is outside the automatic top-three window".to_string());
    }
    if !contains_name(&text, &port.game_name) {
        return CandidateDecision::Pending("title and snippet do not name the game".to_string());
    }
    let same_name = normalized(&port.port_name) == normalized(&port.game_name);
    let qualifying_name = if same_name {
        kind_term(&port.kind)
    } else {
        &port.port_name
    };
    if !contains_name(&text, qualifying_name) {
        return CandidateDecision::Pending(if same_name {
            format!("title and snippet do not name the {}", kind_term(&port.kind))
        } else {
            "title and snippet do not name the port".to_string()
        });
    }
    CandidateDecision::Publish
}
fn thumbnail_url(youtube_id: &str) -> String {
    format!("https://i.ytimg.com/vi/{youtube_id}/hqdefault.jpg")
}


/// Materializes a candidate and queues its thumbnail. Every publication path, including
/// moderator approval, uses this function so public videos do not lose their thumbnail job.
pub async fn publish_and_enqueue_thumbnail(
    pool: &PgPool,
    candidate_id: i64,
    reviewer_id: Option<i64>,
    stem: &str,
) -> Result<DiscoveryCandidate> {
    let candidate = video_discovery::candidate(pool, candidate_id).await?;
    if candidate.decision == video_discovery::DECISION_REJECTED {
        bail!("rejected video discovery candidate {candidate_id}");
    }
    let thumbnail_url = thumbnail_url(&candidate.youtube_id);
    let thumbnail =
        media::enqueue(pool, "video_thumb", &thumbnail_url, &format!("{stem}-video")).await?;
    video_discovery::attach_thumbnail(pool, candidate_id, thumbnail).await?;
    Ok(video_discovery::publish_candidate(pool, candidate_id, reviewer_id).await?)
}

async fn store_result(
    pool: &PgPool,
    port: &DiscoveryPort,
    result: &OrganicResult,
    report: &mut MediaDiscoveryReport,
) -> Result<()> {
    let Some((youtube_id, url)) = canonical_youtube_watch_url(&result.url) else {
        return Ok(());
    };
    let decision = qualify(port, result);
    let (decision, reason) = match decision {
        CandidateDecision::Ignore => return Ok(()),
        CandidateDecision::Publish => (DECISION_PUBLISHED, None),
        CandidateDecision::Pending(reason) => (DECISION_PENDING, Some(reason)),
    };
    let candidate = video_discovery::store_candidate(
        pool,
        &NewDiscoveryCandidate {
            port_id: port.port_id,
            game_id: port.game_id,
            youtube_id,
            url,
            title: result.title.clone(),
            description: result.description.clone(),
            rank: Some(result.rank),
            source: video_discovery::CANDIDATE_SOURCE_GOOGLE,
            decision,
            decision_reason: reason,
        },
    )
    .await?;
    match candidate.decision.as_str() {
        DECISION_PUBLISHED => {
            publish_and_enqueue_thumbnail(pool, candidate.id, None, &port.port_name).await?;
            report.auto_published += 1;
        }
        DECISION_PENDING => report.pending += 1,
        _ => {}
    }
    Ok(())
}

/// Records README YouTube IDs as trusted, port-attributed candidates. IDs are validated by the
/// parser; no page metadata is loaded and thumbnails are only queued after publication.
pub async fn record_readme_videos(
    pool: &PgPool,
    port_id: i64,
    game_id: i64,
    port_name: &str,
    youtube_ids: &[String],
) -> Result<()> {
    for youtube_id in youtube_ids {
        if !is_youtube_id(youtube_id) || video_discovery::is_rejected(pool, port_id, youtube_id).await? {
            continue;
        }
        let candidate = video_discovery::store_candidate(
            pool,
            &NewDiscoveryCandidate {
                port_id,
                game_id,
                youtube_id: youtube_id.clone(),
                url: format!("https://www.youtube.com/watch?v={youtube_id}"),
                title: port_name.to_string(),
                description: String::new(),
                rank: None,
                source: video_discovery::CANDIDATE_SOURCE_README,
                decision: DECISION_PUBLISHED,
                decision_reason: None,
            },
        )
        .await?;
        if candidate.decision == DECISION_PUBLISHED {
            publish_and_enqueue_thumbnail(pool, candidate.id, None, port_name).await?;
        }
    }
    Ok(())
}

/// Retrieves completed local tasks before submitting a single normal-priority batch of due ports.
pub async fn run_media_discovery(
    pool: &PgPool,
    submit_limit: usize,
    retrieve_limit: usize,
) -> Result<MediaDiscoveryReport> {
    if !(1..=DISCOVERY_LIMIT).contains(&submit_limit)
        || !(1..=DISCOVERY_LIMIT).contains(&retrieve_limit)
    {
        bail!("submit_limit and retrieve_limit must be in 1..={DISCOVERY_LIMIT}");
    }
    let client = DataForSeoClient::from_env()?;
    let mut report = MediaDiscoveryReport::default();
    report.expired = video_discovery::expire_uncollected(pool).await?;

    let submitted = video_discovery::submitted_tasks(pool, retrieve_limit as i64).await?;
    let ready = match client.ready_task_ids().await {
        Ok(ids) => ids,
        Err(error) => {
            let message = format!("{error:#}");
            for task in &submitted {
                video_discovery::mark_task_error(pool, task.id, &message).await?;
            }
            return Err(error);
        }
    };
    let by_provider: HashMap<&str, &recomphub_db::video_discovery::DiscoveryTask> = submitted
        .iter()
        .filter_map(|task| task.provider_task_id.as_deref().map(|id| (id, task)))
        .collect();
    for provider_id in ready {
        let Some(task) = by_provider.get(provider_id.as_str()) else { continue };
        match client.results(&provider_id).await {
            Ok(results) => {
                let Some(port) = video_discovery::discovery_port(pool, task.port_id).await? else {
                    video_discovery::mark_task_error(
                        pool,
                        task.id,
                        "task port is no longer published",
                    )
                    .await?;
                    continue;
                };
                for result in results {
                    store_result(pool, &port, &result, &mut report).await?;
                }
                video_discovery::mark_task_collected(pool, task.id).await?;
                report.collected += 1;
            }
            Err(error) => video_discovery::mark_task_error(pool, task.id, &format!("{error:#}")).await?,
        }
    }

    let ports = video_discovery::ports_due_for_discovery(pool, submit_limit as i64).await?;
    if !ports.is_empty() {
        let submitted_ids = client.submit(&ports).await?;
        for port in &ports {
            let provider_id = submitted_ids
                .get(&port.port_id)
                .ok_or_else(|| anyhow!("DataForSEO did not accept task for port {}", port.port_id))?;
            let task = video_discovery::new_task(port, query_for(port), provider_id.clone());
            video_discovery::record_provider_task(pool, &task).await?;
            report.submitted += 1;
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(name: &str, game: &str, kind: &str) -> DiscoveryPort {
        DiscoveryPort {
            port_id: 7,
            game_id: 3,
            port_name: name.to_string(),
            game_name: game.to_string(),
            kind: kind.to_string(),
        }
    }

    #[test]
    fn google_tasks_are_bounded_plain_and_tagged() {
        let tasks = task_payloads(&[port("Zelda64Recomp", "The Legend of Zelda", "static_recomp")])
            .expect("valid task");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["keyword"], "The Legend of Zelda Zelda64Recomp gameplay");
        assert_eq!(tasks[0]["priority"], "normal");
        assert_eq!(tasks[0]["depth"], 10);
        assert_eq!(tasks[0]["tag"], "port:7");
    }

    #[test]
    fn qualification_requires_normal_watch_url_and_named_top_result() {
        let port = port("Ship of Harkinian", "The Legend of Zelda: Ocarina of Time", "static_recomp");
        let accepted = OrganicResult {
            url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ&utm=x".to_string(),
            title: "The Legend of Zelda Ocarina of Time Ship of Harkinian gameplay".to_string(),
            description: String::new(),
            rank: 1,
        };
        assert_eq!(qualify(&port, &accepted), CandidateDecision::Publish);

        let shorts = OrganicResult { url: "https://www.youtube.com/shorts/dQw4w9WgXcQ".to_string(), ..accepted };
        assert_eq!(qualify(&port, &shorts), CandidateDecision::Ignore);
    }

    #[test]
    fn publication_thumbnail_is_the_static_youtube_asset() {
        assert_eq!(
            thumbnail_url("dQw4w9WgXcQ"),
            "https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg"
        );
    }
}
