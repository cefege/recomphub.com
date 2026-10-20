//! Discovery of new recompilation repositories from GitHub search and Reddit.
//!
//! Every hit becomes a `candidate` submission for moderators. Nothing is published here.

use std::collections::HashSet;
use std::time::Duration;

use anyhow::{Context, Result};
use recomphub_db::PgPool;
use recomphub_db::submissions;
use serde_json::{Value, json};

use crate::github::{GithubClient, SearchRepo};
use crate::github_links::canonical_repo_url;
use crate::reddit;

/// GitHub search queries from plan §6. `fork:false` is appended to each one.
pub const GITHUB_QUERIES: &[&str] = &[
    "topic:recomp",
    "topic:recompilation",
    "topic:static-recompilation",
    "topic:n64recomp",
    "topic:xenonrecomp",
    "topic:decompilation",
    "recomp in:name",
    "recompiled in:name",
    "N64Recomp in:readme",
    "XenonRecomp in:readme",
    "ReXGlue in:readme",
    "\"pc port\" in:name,description",
    "topic:decomp",
    "topic:source-port",
    "topic:pc-port",
    "topic:native-port",
    "decomp in:name",
    "\"static recompilation\" in:readme",
    "\"source port\" in:name,description",
    "\"your own copy of the original game\" in:readme",
];

/// Subreddit Atom feeds scanned for linked GitHub repositories.
pub const REDDIT_FEED_URLS: [&str; 3] = [
    "https://www.reddit.com/r/recomps/new/.rss?limit=100",
    "https://www.reddit.com/r/decomps/new/.rss?limit=100",
    "https://www.reddit.com/r/macgaming/new/.rss?limit=100",
];
pub const REDDIT_USER_AGENT: &str = "recomphub/1.0 (+https://recomphub.com)";
/// Result pages read per query (100 repositories each).
pub const SEARCH_PAGES: u32 = 5;
/// Spacing between GitHub search calls, to stay under the search rate limit.
pub const REQUEST_SPACING: Duration = Duration::from_millis(2500);
const REDDIT_TIMEOUT: Duration = Duration::from_secs(30);

/// Counts for one `discover` run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DiscoverReport {
    /// Distinct GitHub repositories returned by the searches.
    pub seen: usize,
    /// GitHub repositories queued as candidates.
    pub kept: usize,
    /// Candidate submissions created (GitHub and Reddit together).
    pub inserted: usize,
    /// Set when a subreddit feed was skipped (a non-200 response).
    pub reddit_skipped: bool,
}


/// Payload stored on a GitHub-discovered candidate.
pub fn github_payload(repo: &SearchRepo) -> Value {
    json!({
        "repo_url": format!("https://github.com/{}", repo.full_name.to_lowercase()),
        "name": repo.name,
        "description": repo.description,
        "stars": repo.stars,
        "source_url": repo.html_url,
        "source_title": repo.full_name,
    })
}

/// Payload stored on a Reddit-discovered candidate. The name is the repository name, since the
/// feed carries no other metadata.
pub fn reddit_payload(repo: &reddit::RedditRepo) -> Value {
    let name = repo
        .repo_url
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();
    json!({
        "repo_url": repo.repo_url,
        "name": name,
        "description": Value::Null,
        "stars": Value::Null,
        "source_url": repo.source_url,
        "source_title": repo.source_title,
    })
}

/// Runs every GitHub query, then the configured subreddit feeds.
///
/// Requires `GITHUB_TOKEN` through `client`: unauthenticated search is limited to 10 requests a
/// minute, which the queries exceed.
pub async fn run_discover(pool: &PgPool, client: &GithubClient) -> Result<DiscoverReport> {
    let mut report = DiscoverReport::default();
    let mut seen: HashSet<String> = HashSet::new();
    let mut first_call = true;
    for query in GITHUB_QUERIES {
        let full_query = format!("{query} fork:false");
        for page in 1..=SEARCH_PAGES {
            if !first_call {
                tokio::time::sleep(REQUEST_SPACING).await;
            }
            first_call = false;
            let repos = client
                .search_repositories(&full_query, page)
                .await
                .with_context(|| format!("GitHub search {full_query:?} page {page}"))?;
            let last_page = repos.len() < 100;
            for repo in repos {
                let Some(canonical) =
                    canonical_repo_url(&format!("https://github.com/{}", repo.full_name))
                else {
                    continue;
                };
                if !seen.insert(canonical) {
                    continue;
                }
                report.seen += 1;
                report.kept += 1;
                if submissions::add_candidate(pool, "discovery:github", &github_payload(&repo))
                    .await?
                {
                    report.inserted += 1;
                }
            }
            if last_page {
                break;
            }
        }
    }
    run_reddit(pool, &mut report).await?;
    Ok(report)
}

/// Fetches each subreddit feed. A non-200 response logs a warning and skips only that feed.
async fn run_reddit(pool: &PgPool, report: &mut DiscoverReport) -> Result<()> {
    let http = reqwest::Client::builder()
        .user_agent(REDDIT_USER_AGENT)
        .timeout(REDDIT_TIMEOUT)
        .build()
        .context("building the Reddit HTTP client")?;
    for url in REDDIT_FEED_URLS {
        let resp = http
            .get(url)
            .send()
            .await
            .with_context(|| format!("requesting {url}"))?;
        let status = resp.status();
        if !status.is_success() {
            tracing::warn!(%status, url, "subreddit feed returned a non-200 response; skipping it");
            report.reddit_skipped = true;
            continue;
        }
        let body = resp
            .bytes()
            .await
            .with_context(|| format!("reading {url}"))?;
        for repo in reddit::parse_feed(&body)? {
            if submissions::add_candidate(pool, "discovery:reddit", &reddit_payload(&repo)).await? {
                report.inserted += 1;
            }
        }
    }
    Ok(())
}
