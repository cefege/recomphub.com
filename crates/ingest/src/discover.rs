//! Discovery of new recompilation repositories from GitHub search and r/recomps.
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

pub const REDDIT_FEED_URL: &str = "https://www.reddit.com/r/recomps/new/.rss?limit=100";
pub const REDDIT_USER_AGENT: &str = "recomphub/1.0 (+https://recomphub.com)";
/// A GitHub search hit is kept when it has at least this many stars, or any release.
pub const KEEP_MIN_STARS: i64 = 5;
/// Result pages read per query (100 repositories each). Page 1 holds the most recently updated
/// repositories and gets the release check; deeper pages reach older projects and keep only
/// repositories with `KEEP_MIN_STARS`, so a broad query cannot trigger thousands of calls.
pub const SEARCH_PAGES: u32 = 5;
/// Spacing between GitHub search calls, to stay under the search rate limit.
pub const REQUEST_SPACING: Duration = Duration::from_millis(2500);
const REDDIT_TIMEOUT: Duration = Duration::from_secs(30);

/// Counts for one `discover` run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DiscoverReport {
    /// Distinct GitHub repositories returned by the searches.
    pub seen: usize,
    /// GitHub repositories that passed the keep filter.
    pub kept: usize,
    /// Candidate submissions created (GitHub and Reddit together).
    pub inserted: usize,
    /// Set when the r/recomps feed was skipped (a non-200 response).
    pub reddit_skipped: bool,
}

/// The keep rule: `stars >= KEEP_MIN_STARS` or the repository has a release.
pub fn keep(stars: i64, has_release: bool) -> bool {
    stars >= KEEP_MIN_STARS || has_release
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

/// Runs every GitHub query, then the r/recomps feed.
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
                let kept = if repo.stars >= KEEP_MIN_STARS {
                    true
                } else if page == 1 {
                    tokio::time::sleep(REQUEST_SPACING).await;
                    let has_release = client
                        .has_release(&repo.full_name)
                        .await
                        .with_context(|| format!("checking releases of {}", repo.full_name))?;
                    keep(repo.stars, has_release)
                } else {
                    false
                };
                if !kept {
                    continue;
                }
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

/// Fetches r/recomps. A non-200 response logs a warning and skips only this source.
async fn run_reddit(pool: &PgPool, report: &mut DiscoverReport) -> Result<()> {
    let http = reqwest::Client::builder()
        .user_agent(REDDIT_USER_AGENT)
        .timeout(REDDIT_TIMEOUT)
        .build()
        .context("building the Reddit HTTP client")?;
    let resp = http
        .get(REDDIT_FEED_URL)
        .send()
        .await
        .context("requesting the r/recomps feed")?;
    let status = resp.status();
    if !status.is_success() {
        tracing::warn!(%status, "r/recomps feed returned a non-200 response; skipping Reddit");
        report.reddit_skipped = true;
        return Ok(());
    }
    let body = resp.bytes().await.context("reading the r/recomps feed")?;
    for repo in reddit::parse_feed(&body)? {
        if submissions::add_candidate(pool, "discovery:reddit", &reddit_payload(&repo)).await? {
            report.inserted += 1;
        }
    }
    Ok(())
}
