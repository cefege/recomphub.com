//! GitHub client.
//!
//! - `preview` (REST, token optional): repo metadata, release, AI detection and contributors for
//!   the submission form. Writes nothing.
//! - `run_github_sync` (`worker github-sync`): every 6 hours, metadata for due published ports via
//!   one GraphQL request per 50 repos. Requires `GITHUB_TOKEN`.
//! - The weekly AI and author scan in the same job uses the REST preview for each due port.
//!
//! Field ownership (plan §2) is enforced by `recomphub_db::ports::{update_github, replace_github_authors}`.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use recomphub_db::PgPool;
use recomphub_db::media;
use recomphub_db::ports::{
    self, AiEvidence as DbAiEvidence, AiSync, AuthorInput, GithubSync, PortRow,
};
use recomphub_shared::{AiUsage, TargetPlatform};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::detect::{self, AiDetection, CommitInfo};
use crate::github_links;

const API: &str = "https://api.github.com";
const GRAPHQL_URL: &str = "https://api.github.com/graphql";
const USER_AGENT: &str = "recomphub/1.0 (+https://recomphub.com)";
const API_VERSION: &str = "2022-11-28";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const RAW: &str = "application/vnd.github.raw";
const JSON_ACCEPT: &str = "application/vnd.github+json";
const ABSENT: &[StatusCode] = &[StatusCode::NOT_FOUND];
/// Statuses that mean "no data here" for list endpoints (empty repo, or no contributors).
const ABSENT_LIST: &[StatusCode] = &[
    StatusCode::NOT_FOUND,
    StatusCode::CONFLICT,
    StatusCode::NO_CONTENT,
];

/// Published ports per GraphQL request.
pub const GRAPHQL_BATCH: usize = 50;
/// Ports AI-scanned per `run_github_sync` call. The weekly scan is spread across 6-hour runs.
pub const SCAN_LIMIT_PER_RUN: i64 = 200;
/// A contributor must have at least this share of commits to be credited.
pub const MIN_CONTRIBUTOR_SHARE: f64 = 0.10;
/// Contributors credited for a user-owned repo, not counting the owner.
pub const USER_CONTRIBUTOR_CAP: usize = 5;
/// Contributors credited for an organization-owned repo.
pub const ORG_CONTRIBUTOR_CAP: usize = 3;
/// Contributors fetched per repo. Shares are computed over this list.
const CONTRIBUTOR_PAGE: u32 = 100;
const AVATAR_SUFFIX: &str = "&s=80";

/// A release as stored on a port.
#[derive(Debug, Clone, PartialEq)]
pub struct ReleaseInfo {
    pub tag: String,
    pub url: String,
    pub published_at: Option<DateTime<Utc>>,
    pub prerelease: bool,
    pub assets: Vec<String>,
}

/// A credited contributor, already filtered by `select_contributors`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contributor {
    pub login: String,
    pub avatar_url: String,
    pub commits: i32,
}

/// Everything the submission form and the weekly scan need about one repository.
#[derive(Debug, Clone)]
pub struct RepoPreview {
    pub github_repo_id: i64,
    pub repo_url: String,
    pub owner: String,
    pub owner_is_org: bool,
    pub owner_name: Option<String>,
    pub owner_avatar_url: String,
    pub name: String,
    pub description: Option<String>,
    pub homepage_url: Option<String>,
    pub default_branch: String,
    pub stars: i32,
    pub forks: i32,
    pub pushed_at: Option<DateTime<Utc>>,
    pub archived: bool,
    pub license: Option<String>,
    pub latest_release_tag: Option<String>,
    pub latest_release_url: Option<String>,
    pub latest_release_at: Option<DateTime<Utc>>,
    pub latest_release_prerelease: bool,
    pub release_assets: Vec<String>,
    pub detected_platforms: Vec<TargetPlatform>,
    pub ai: AiDetection,
    pub contributors: Vec<Contributor>,
}

/// One author row to write for a port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorSpec {
    pub login: String,
    pub name: String,
    pub avatar_url: String,
    pub role: &'static str,
    pub commits: Option<i32>,
}

/// Counts from one `run_github_sync` call.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct GithubSyncReport {
    /// Ports whose metadata was refreshed through GraphQL.
    pub synced: usize,
    /// Ports whose AI and author scan ran.
    pub scanned: usize,
    /// Ports hidden because their repository no longer exists.
    pub hidden: usize,
    /// Ports whose scan request failed; they stay due and are retried on the next run.
    pub failed: usize,
}

/// HTTP client for the GitHub REST and GraphQL APIs.
#[derive(Clone)]
pub struct GithubClient {
    http: reqwest::Client,
    token: Option<String>,
}

impl GithubClient {
    /// Creates a client. A blank token counts as absent.
    pub fn new(token: Option<String>) -> Result<Self> {
        let token = token.filter(|t| !t.trim().is_empty());
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .context("building the GitHub HTTP client")?;
        Ok(Self { http, token })
    }

    /// Reads `GITHUB_TOKEN`. The token is optional for REST previews.
    pub fn from_env() -> Result<Self> {
        Self::new(std::env::var("GITHUB_TOKEN").ok())
    }

    fn graphql_token(&self) -> Result<&str> {
        self.token.as_deref().ok_or_else(|| {
            anyhow!("GITHUB_TOKEN is required: the GitHub GraphQL API rejects anonymous requests")
        })
    }

    /// GET a REST path and return the body text. Returns `None` for any status in `absent`.
    async fn rest_raw(
        &self,
        path: &str,
        accept: &str,
        absent: &[StatusCode],
    ) -> Result<Option<String>> {
        let url = format!("{API}{path}");
        let mut req = self
            .http
            .get(&url)
            .header(reqwest::header::ACCEPT, accept)
            .header("X-GitHub-Api-Version", API_VERSION);
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }
        let resp = req.send().await.with_context(|| format!("GET {path}"))?;
        let status = resp.status();
        if absent.contains(&status) {
            return Ok(None);
        }
        let body = resp
            .text()
            .await
            .with_context(|| format!("reading GET {path}"))?;
        if !status.is_success() {
            bail!("GET {path} returned HTTP {status}: {}", snippet(&body));
        }
        Ok(Some(body))
    }

    async fn rest_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        absent: &[StatusCode],
    ) -> Result<Option<T>> {
        match self.rest_raw(path, JSON_ACCEPT, absent).await? {
            None => Ok(None),
            Some(body) => serde_json::from_str(&body)
                .with_context(|| format!("parsing GET {path}"))
                .map(Some),
        }
    }

    /// POST one GraphQL query and return its `data`. Errors other than `NOT_FOUND` fail.
    async fn graphql(&self, query: &str) -> Result<Value> {
        let token = self.graphql_token()?;
        let resp = self
            .http
            .post(GRAPHQL_URL)
            .bearer_auth(token)
            .json(&json!({ "query": query }))
            .send()
            .await
            .context("GitHub GraphQL request")?;
        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .context("parsing GitHub GraphQL response")?;
        if !status.is_success() {
            bail!(
                "GitHub GraphQL returned HTTP {status}: {}",
                snippet(&body.to_string())
            );
        }
        if let Some(errors) = body.get("errors").and_then(Value::as_array) {
            let fatal: Vec<&Value> = errors
                .iter()
                .filter(|e| e.get("type").and_then(Value::as_str) != Some("NOT_FOUND"))
                .collect();
            if !fatal.is_empty() {
                bail!(
                    "GitHub GraphQL error: {}",
                    snippet(&Value::Array(fatal.into_iter().cloned().collect()).to_string())
                );
            }
        }
        body.get("data")
            .filter(|d| !d.is_null())
            .cloned()
            .ok_or_else(|| anyhow!("GitHub GraphQL response has no data"))
    }

    /// Latest published release, including prereleases. Drafts are skipped.
    async fn latest_release(&self, base: &str) -> Result<Option<ReleaseInfo>> {
        let list: Vec<ReleaseJson> = self
            .rest_json(&format!("{base}/releases?per_page=10"), ABSENT)
            .await?
            .unwrap_or_default();
        match list.into_iter().find(|r| !r.draft) {
            None => Ok(None),
            Some(r) => Ok(Some(ReleaseInfo {
                tag: r.tag_name,
                url: r.html_url,
                published_at: r.published_at.as_deref().map(parse_ts).transpose()?,
                prerelease: r.prerelease,
                assets: r.assets.into_iter().map(|a| a.name).collect(),
            })),
        }
    }

    /// Repositories matching a search query, most recently updated first. One REST call.
    pub async fn search_repositories(&self, query: &str) -> Result<Vec<SearchRepo>> {
        let url = reqwest::Url::parse_with_params(
            &format!("{API}/search/repositories"),
            &[("q", query), ("sort", "updated"), ("per_page", "100")],
        )
        .context("building the repository search URL")?;
        let path = format!("{}?{}", url.path(), url.query().unwrap_or_default());
        let body: SearchJson = self
            .rest_json(&path, &[])
            .await?
            .ok_or_else(|| anyhow!("GitHub repository search returned no body"))?;
        Ok(body.items)
    }

    /// Whether a repository has at least one published (non-draft) release, prereleases included.
    pub async fn has_release(&self, full_name: &str) -> Result<bool> {
        let list: Vec<ReleaseJson> = self
            .rest_json(&format!("/repos/{full_name}/releases?per_page=10"), ABSENT)
            .await?
            .unwrap_or_default();
        Ok(list.iter().any(|r| !r.draft))
    }

    /// Full REST snapshot of one repository. `None` when the repository does not exist.
    pub async fn fetch_preview(&self, owner: &str, repo: &str) -> Result<Option<RepoPreview>> {
        let base = format!("/repos/{owner}/{repo}");
        let Some(r): Option<RepoJson> = self.rest_json(&base, ABSENT).await? else {
            return Ok(None);
        };
        let release = self.latest_release(&base).await?;
        let root: Vec<ContentEntry> = self
            .rest_json(&format!("{base}/contents/"), ABSENT_LIST)
            .await?
            .unwrap_or_default();
        let root_names: Vec<String> = root.into_iter().map(|e| e.name).collect();
        let root_refs: Vec<&str> = root_names.iter().map(String::as_str).collect();
        let copilot = self
            .rest_raw(
                &format!("{base}/contents/.github/copilot-instructions.md"),
                RAW,
                ABSENT,
            )
            .await?
            .is_some();
        let readme = self
            .rest_raw(&format!("{base}/readme"), RAW, ABSENT)
            .await?;
        let commits: Vec<CommitJson> = self
            .rest_json(&format!("{base}/commits?per_page=100"), ABSENT_LIST)
            .await?
            .unwrap_or_default();
        let commit_infos: Vec<CommitInfo> = commits
            .into_iter()
            .map(|c| CommitInfo {
                message: c.commit.message,
                author_login: c.author.and_then(|a| a.login),
            })
            .collect();
        let ai = detect::detect_ai(&root_refs, copilot, readme.as_deref(), &commit_infos);
        let owner_is_org = r.owner.kind.eq_ignore_ascii_case("Organization");
        let owner_name = if owner_is_org {
            None
        } else {
            self.rest_json::<UserJson>(&format!("/users/{}", r.owner.login), ABSENT)
                .await?
                .and_then(|u| u.name)
                .filter(|n| !n.trim().is_empty())
        };
        let stats: Vec<ContributorStat> = self
            .rest_json(
                &format!("{base}/contributors?per_page={CONTRIBUTOR_PAGE}"),
                ABSENT_LIST,
            )
            .await?
            .unwrap_or_default();
        let contributors = select_contributors(&stats, &r.owner.login, owner_is_org);
        let asset_names: Vec<String> = release
            .as_ref()
            .map(|x| x.assets.clone())
            .unwrap_or_default();
        let asset_refs: Vec<&str> = asset_names.iter().map(String::as_str).collect();
        let detected_platforms = detect::detect_platforms(&asset_refs);
        let repo_url =
            github_links::canonical_repo_url(&format!("https://github.com/{}", r.full_name))
                .ok_or_else(|| {
                    anyhow!(
                        "GitHub returned an unusable repository name: {}",
                        r.full_name
                    )
                })?;
        Ok(Some(RepoPreview {
            github_repo_id: r.id,
            repo_url,
            owner: r.owner.login,
            owner_is_org,
            owner_name,
            owner_avatar_url: r.owner.avatar_url,
            name: r.name,
            description: non_empty(r.description),
            homepage_url: non_empty(r.homepage),
            default_branch: r.default_branch,
            stars: r.stargazers_count,
            forks: r.forks_count,
            pushed_at: r.pushed_at.as_deref().map(parse_ts).transpose()?,
            archived: r.archived,
            license: license_name(r.license.and_then(|l| l.spdx_id).as_deref()),
            latest_release_tag: release.as_ref().map(|x| x.tag.clone()),
            latest_release_url: release.as_ref().map(|x| x.url.clone()),
            latest_release_at: release.as_ref().and_then(|x| x.published_at),
            latest_release_prerelease: release.as_ref().is_some_and(|x| x.prerelease),
            release_assets: asset_names,
            detected_platforms,
            ai,
            contributors,
        }))
    }
}

/// Public preview entry point for the submission form. Writes nothing.
/// Fails with a clear error when the repository does not exist.
pub async fn preview(token: Option<&str>, owner: &str, repo: &str) -> Result<RepoPreview> {
    let client = GithubClient::new(token.map(str::to_string))?;
    client
        .fetch_preview(owner, repo)
        .await?
        .ok_or_else(|| anyhow!("GitHub repository {owner}/{repo} was not found"))
}

/// Splits a stored repo URL into `(owner, repo)`.
pub fn parse_repo(url: &str) -> Result<(String, String)> {
    github_links::normalize_repo_url(url)
        .ok_or_else(|| anyhow!("not a GitHub repository URL: {url}"))
}

/// Filters and orders the contributor list. Pure; see plan §5.
///
/// The owner is excluded here. Organization owners keep up to `ORG_CONTRIBUTOR_CAP`, and user
/// owners keep up to `USER_CONTRIBUTOR_CAP`. Each contributor needs at least
/// `MIN_CONTRIBUTOR_SHARE` of the summed contributions in `stats`.
pub fn select_contributors(
    stats: &[ContributorStat],
    owner: &str,
    owner_is_org: bool,
) -> Vec<Contributor> {
    let total: i64 = stats.iter().map(|s| s.contributions.max(0)).sum();
    if total <= 0 {
        return Vec::new();
    }
    let cap = if owner_is_org {
        ORG_CONTRIBUTOR_CAP
    } else {
        USER_CONTRIBUTOR_CAP
    };
    let mut sorted: Vec<&ContributorStat> = stats.iter().collect();
    sorted.sort_by_key(|s| std::cmp::Reverse(s.contributions));
    sorted
        .into_iter()
        .filter(|s| !s.login.eq_ignore_ascii_case(owner))
        .filter(|s| s.contributions as f64 >= MIN_CONTRIBUTOR_SHARE * total as f64)
        .take(cap)
        .map(|s| Contributor {
            login: s.login.clone(),
            avatar_url: s.avatar_url.clone(),
            commits: s.contributions.min(i64::from(i32::MAX)) as i32,
        })
        .collect()
}

/// Author rows for a repo, in display order: the owner first for user-owned repos, then the
/// credited contributors. Organization-owned repos have no owner row.
pub fn author_specs(prev: &RepoPreview) -> Vec<AuthorSpec> {
    let mut out = Vec::new();
    if !prev.owner_is_org {
        out.push(AuthorSpec {
            login: prev.owner.clone(),
            name: prev
                .owner_name
                .clone()
                .unwrap_or_else(|| prev.owner.clone()),
            avatar_url: prev.owner_avatar_url.clone(),
            role: "owner",
            commits: None,
        });
    }
    out.extend(prev.contributors.iter().map(|c| AuthorSpec {
        login: c.login.clone(),
        name: c.login.clone(),
        avatar_url: c.avatar_url.clone(),
        role: "contributor",
        commits: Some(c.commits),
    }));
    out
}

/// Turns author specs into db rows. Enqueues each avatar as `avatarUrl&s=80`.
pub async fn author_inputs(pool: &PgPool, specs: &[AuthorSpec]) -> Result<Vec<AuthorInput>> {
    let mut out = Vec::with_capacity(specs.len());
    for s in specs {
        let avatar_media_id = if s.avatar_url.is_empty() {
            None
        } else {
            Some(media::enqueue(pool, "avatar", &format!("{}{AVATAR_SUFFIX}", s.avatar_url)).await?)
        };
        out.push(AuthorInput {
            github_login: Some(s.login.clone()),
            name: s.name.clone(),
            avatar_media_id,
            role: s.role.to_string(),
            commits: s.commits,
        });
    }
    Ok(out)
}

/// Builds the db metadata row from a repo snapshot. Shared by the REST preview and the GraphQL sync.
#[allow(clippy::too_many_arguments)]
fn sync_row(
    github_repo_id: i64,
    repo_url: String,
    homepage_url: Option<String>,
    license: Option<String>,
    stars: i32,
    forks: i32,
    pushed_at: Option<DateTime<Utc>>,
    archived: bool,
    release: Option<&ReleaseInfo>,
) -> GithubSync {
    let assets: Vec<&str> = release
        .map(|r| r.assets.iter().map(String::as_str).collect())
        .unwrap_or_default();
    let detected_platforms = detect::detect_platforms(&assets)
        .into_iter()
        .map(|p| p.as_str().to_string())
        .collect();
    GithubSync {
        github_repo_id,
        repo_url,
        homepage_url,
        license,
        github_stars: stars,
        github_forks: forks,
        pushed_at,
        archived,
        latest_version: release.map(|r| r.tag.clone()),
        latest_release_url: release.map(|r| r.url.clone()),
        latest_release_at: release.and_then(|r| r.published_at),
        latest_is_prerelease: release.map(|r| r.prerelease),
        detected_platforms,
    }
}

/// Metadata sync row for a preview.
pub fn preview_sync(prev: &RepoPreview) -> GithubSync {
    let release = prev.latest_release_tag.as_ref().map(|tag| ReleaseInfo {
        tag: tag.clone(),
        url: prev.latest_release_url.clone().unwrap_or_default(),
        published_at: prev.latest_release_at,
        prerelease: prev.latest_release_prerelease,
        assets: prev.release_assets.clone(),
    });
    sync_row(
        prev.github_repo_id,
        prev.repo_url.clone(),
        prev.homepage_url.clone(),
        prev.license.clone(),
        prev.stars,
        prev.forks,
        prev.pushed_at,
        prev.archived,
        release.as_ref(),
    )
}

/// AI sync input for a detection. `usage: None` leaves the stored AI fields untouched.
pub fn ai_sync(ai: &AiDetection) -> AiSync {
    AiSync {
        usage: ai.usage.map(|u: AiUsage| u.as_str().to_string()),
        tools: ai.tools.clone(),
        evidence: ai
            .evidence
            .iter()
            .map(|e| DbAiEvidence {
                signal: e.signal.clone(),
                detail: e.detail.clone(),
            })
            .collect(),
    }
}

/// GraphQL query that fetches `repos` by alias `r0..rN`. Owner and name are JSON-escaped.
pub fn batch_query(repos: &[(String, String)]) -> String {
    let fields = "databaseId nameWithOwner stargazerCount forkCount pushedAt isArchived homepageUrl \
        licenseInfo { spdxId } \
        releases(first: 1, orderBy: {field: CREATED_AT, direction: DESC}) { nodes { tagName url publishedAt isPrerelease \
        releaseAssets(first: 50) { nodes { name } } } }";
    let body: String = repos
        .iter()
        .enumerate()
        .map(|(i, (owner, name))| {
            format!(
                "r{i}: repository(owner: {}, name: {}) {{ {fields} }}\n",
                json!(owner),
                json!(name)
            )
        })
        .collect();
    format!("query {{\n{body}}}")
}

/// Reads aliases `r0..r{count-1}` from a batch response. `None` is a repository that no longer exists.
pub fn parse_batch(data: &Value, count: usize) -> Result<Vec<Option<GqlRepo>>> {
    (0..count)
        .map(|i| {
            let key = format!("r{i}");
            let value = data
                .get(&key)
                .ok_or_else(|| anyhow!("GraphQL response is missing alias {key}"))?;
            if value.is_null() {
                return Ok(None);
            }
            serde_json::from_value(value.clone())
                .with_context(|| format!("parsing GraphQL alias {key}"))
                .map(Some)
        })
        .collect()
}

/// One repository from the GraphQL batch.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlRepo {
    pub database_id: i64,
    pub name_with_owner: String,
    pub stargazer_count: i32,
    pub fork_count: i32,
    pub pushed_at: Option<String>,
    pub is_archived: bool,
    pub homepage_url: Option<String>,
    pub license_info: Option<GqlLicense>,
    pub releases: GqlReleases,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlLicense {
    pub spdx_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GqlReleases {
    pub nodes: Vec<GqlRelease>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlRelease {
    pub tag_name: String,
    pub url: String,
    pub published_at: Option<String>,
    pub is_prerelease: bool,
    pub release_assets: GqlAssets,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GqlAssets {
    pub nodes: Vec<GqlAsset>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GqlAsset {
    pub name: String,
}

impl GqlRepo {
    /// Metadata sync row. `repo_url` follows `nameWithOwner`, so renamed repos are rewritten.
    pub fn to_sync(&self) -> Result<GithubSync> {
        let repo_url = github_links::canonical_repo_url(&format!(
            "https://github.com/{}",
            self.name_with_owner
        ))
        .ok_or_else(|| {
            anyhow!(
                "GitHub returned an unusable repository name: {}",
                self.name_with_owner
            )
        })?;
        let release = match self.releases.nodes.first() {
            None => None,
            Some(r) => Some(ReleaseInfo {
                tag: r.tag_name.clone(),
                url: r.url.clone(),
                published_at: r.published_at.as_deref().map(parse_ts).transpose()?,
                prerelease: r.is_prerelease,
                assets: r
                    .release_assets
                    .nodes
                    .iter()
                    .map(|a| a.name.clone())
                    .collect(),
            }),
        };
        Ok(sync_row(
            self.database_id,
            repo_url,
            non_empty(self.homepage_url.clone()),
            license_name(
                self.license_info
                    .as_ref()
                    .and_then(|l| l.spdx_id.as_deref()),
            ),
            self.stargazer_count,
            self.fork_count,
            self.pushed_at.as_deref().map(parse_ts).transpose()?,
            self.is_archived,
            release.as_ref(),
        ))
    }
}

/// Runs the 6-hourly metadata sync and the weekly AI and author scan.
///
/// The GraphQL sync needs `GITHUB_TOKEN`, and this fails before writing anything when it is missing.
/// Repositories that no longer exist are hidden. The per-port update and the author write run in
/// one transaction.
pub async fn run_github_sync(pool: &PgPool, client: &GithubClient) -> Result<GithubSyncReport> {
    client.graphql_token()?;
    let mut report = GithubSyncReport::default();

    loop {
        let due = ports::due_github_sync(pool, GRAPHQL_BATCH as i64).await?;
        if due.is_empty() {
            break;
        }
        let keys = due
            .iter()
            .map(|p| parse_repo(&p.repo_url))
            .collect::<Result<Vec<_>>>()?;
        let data = client.graphql(&batch_query(&keys)).await?;
        let repos = parse_batch(&data, due.len())?;
        for (port, repo) in due.iter().zip(repos) {
            match repo {
                None => {
                    hide_port(pool, port).await?;
                    report.hidden += 1;
                }
                Some(r) => {
                    let sync = r.to_sync()?;
                    if hide_if_duplicate(pool, port, &sync).await? {
                        report.hidden += 1;
                        continue;
                    }
                    let mut tx = pool.begin().await?;
                    ports::update_github(&mut tx, port.id, &sync, None).await?;
                    tx.commit().await?;
                    report.synced += 1;
                }
            }
        }
    }

    let scan = ports::due_ai_scan(pool, SCAN_LIMIT_PER_RUN).await?;
    for port in &scan {
        let (owner, repo) = parse_repo(&port.repo_url)?;
        // One flaky request must not stop the scan; the port stays due and is retried next run.
        let fetched = match client.fetch_preview(&owner, &repo).await {
            Ok(fetched) => fetched,
            Err(e) => {
                tracing::warn!(port_id = port.id, repo = %port.repo_url, error = %format!("{e:#}"), "scan failed; retrying next run");
                report.failed += 1;
                continue;
            }
        };
        match fetched {
            None => {
                hide_port(pool, port).await?;
                report.hidden += 1;
            }
            Some(prev) => {
                let sync = preview_sync(&prev);
                if hide_if_duplicate(pool, port, &sync).await? {
                    report.hidden += 1;
                    continue;
                }
                let ai = ai_sync(&prev.ai);
                let authors = author_inputs(pool, &author_specs(&prev)).await?;
                let mut tx = pool.begin().await?;
                ports::update_github(&mut tx, port.id, &sync, Some(&ai)).await?;
                ports::replace_github_authors(&mut tx, port.id, &authors).await?;
                tx.commit().await?;
                report.scanned += 1;
            }
        }
    }
    Ok(report)
}

async fn hide_port(pool: &PgPool, port: &PortRow) -> Result<()> {
    let mut tx = pool.begin().await?;
    ports::set_status(&mut tx, port.id, "hidden").await?;
    tx.commit().await?;
    tracing::warn!(port_id = port.id, repo = %port.repo_url, "repository no longer exists; port hidden");
    Ok(())
}

/// Hides `port` when another port already holds its repository, which happens when a repo was
/// imported under both its old and its new name. Returns whether it was hidden.
async fn hide_if_duplicate(pool: &PgPool, port: &PortRow, sync: &GithubSync) -> Result<bool> {
    let Some(other) =
        ports::other_port_with_repo(pool, port.id, sync.github_repo_id, &sync.repo_url).await?
    else {
        return Ok(false);
    };
    let mut tx = pool.begin().await?;
    ports::set_status(&mut tx, port.id, "hidden").await?;
    tx.commit().await?;
    tracing::warn!(port_id = port.id, duplicate_of = other, repo = %sync.repo_url, "repository already listed under another port; port hidden");
    Ok(true)
}

fn snippet(s: &str) -> String {
    s.chars().take(200).collect()
}

fn parse_ts(s: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .with_context(|| format!("invalid GitHub timestamp {s}"))
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// GitHub reports "NOASSERTION" for unknown licenses. Treat it as absent.
fn license_name(spdx: Option<&str>) -> Option<String> {
    spdx.filter(|s| !s.is_empty() && *s != "NOASSERTION")
        .map(str::to_string)
}

#[derive(Deserialize)]
struct RepoJson {
    id: i64,
    full_name: String,
    name: String,
    description: Option<String>,
    homepage: Option<String>,
    stargazers_count: i32,
    forks_count: i32,
    pushed_at: Option<String>,
    archived: bool,
    default_branch: String,
    license: Option<LicenseJson>,
    owner: OwnerJson,
}

#[derive(Deserialize)]
struct LicenseJson {
    spdx_id: Option<String>,
}

#[derive(Deserialize)]
struct OwnerJson {
    login: String,
    avatar_url: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct ReleaseJson {
    tag_name: String,
    html_url: String,
    published_at: Option<String>,
    prerelease: bool,
    draft: bool,
    assets: Vec<AssetJson>,
}

#[derive(Deserialize)]
struct AssetJson {
    name: String,
}

#[derive(Deserialize)]
struct ContentEntry {
    name: String,
}

#[derive(Deserialize)]
struct CommitJson {
    commit: CommitBody,
    author: Option<CommitAuthor>,
}

#[derive(Deserialize)]
struct CommitBody {
    message: String,
}

/// GitHub sends `{}` for commits whose email is not linked to an account.
#[derive(Deserialize)]
struct CommitAuthor {
    login: Option<String>,
}

#[derive(Deserialize)]
struct UserJson {
    name: Option<String>,
}

/// One entry from `GET /repos/{o}/{r}/contributors`. `contributions` is the commit count.
#[derive(Debug, Clone, Deserialize)]
pub struct ContributorStat {
    pub login: String,
    pub avatar_url: String,
    pub contributions: i64,
}

/// One repository from `GET /search/repositories`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SearchRepo {
    pub full_name: String,
    pub name: String,
    pub html_url: String,
    pub description: Option<String>,
    #[serde(rename = "stargazers_count")]
    pub stars: i64,
}

#[derive(Deserialize)]
struct SearchJson {
    items: Vec<SearchRepo>,
}
