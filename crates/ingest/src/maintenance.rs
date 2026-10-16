//! Maintenance signals for a port: how recently and how often the default branch changes, plus
//! release cadence and contributor count. They come from their own GraphQL query over chunks of
//! repos (see `maintenance_query`), so the metadata batch is unchanged. The weekly sparkline comes
//! from one REST call per port (see `GithubClient::weekly_commits`). Callers pass `now` so the
//! logic is testable.

use std::collections::HashSet;

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use recomphub_db::ports::MaintenanceSync;
use recomphub_shared::{MaintenanceInput, maintenance_level, maintenance_score};
use serde::Deserialize;
use serde_json::{Value, json};

/// Days of commit history used for the distinct-author count.
const AUTHOR_WINDOW_DAYS: i64 = 180;
/// Commits fetched for the author count. The count is a lower bound on busy repos.
const AUTHOR_SAMPLE: u32 = 100;
/// Releases fetched for the release count. The count is a lower bound on busy repos.
const RELEASE_SAMPLE: u32 = 100;
/// Weeks kept for the sparkline.
pub const SPARKLINE_WEEKS: usize = 12;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlBranchRef {
    #[serde(default)]
    pub target: Option<GqlCommitTarget>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlCommitTarget {
    #[serde(default)]
    pub commits_30d: GqlHistory,
    #[serde(default)]
    pub commits_90d: GqlHistory,
    #[serde(default)]
    pub commits_365d: GqlHistory,
    #[serde(default)]
    pub recent_commits: GqlHistory,
    #[serde(default)]
    pub latest_commit: GqlHistory,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlHistory {
    #[serde(default)]
    pub total_count: u32,
    #[serde(default)]
    pub nodes: Vec<GqlCommit>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlCommit {
    pub committed_date: Option<String>,
    pub author: Option<GqlGitActor>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GqlGitActor {
    pub email: Option<String>,
    pub user: Option<GqlUser>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GqlUser {
    pub login: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlReleaseDates {
    #[serde(default)]
    pub nodes: Vec<GqlReleaseNode>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlReleaseNode {
    pub published_at: Option<String>,
}

/// One repo from the maintenance query.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlMaintenanceRepo {
    pub is_archived: bool,
    #[serde(default)]
    pub default_branch_ref: Option<GqlBranchRef>,
    #[serde(default)]
    pub recent_releases: Option<GqlReleaseDates>,
}

/// Selection for one repo. Windows are measured back from `now`.
fn maintenance_selection(now: DateTime<Utc>) -> String {
    let at = |days: i64| (now - Duration::days(days)).to_rfc3339_opts(SecondsFormat::Secs, true);
    format!(
        "isArchived defaultBranchRef {{ target {{ ... on Commit {{ \
            commits30d: history(since: \"{d30}\") {{ totalCount }} \
            commits90d: history(since: \"{d90}\") {{ totalCount }} \
            commits365d: history(since: \"{d365}\") {{ totalCount }} \
            recentCommits: history(since: \"{d180}\", first: {sample}) {{ nodes {{ author {{ email user {{ login }} }} }} }} \
            latestCommit: history(first: 1) {{ nodes {{ committedDate }} }} \
        }} }} }} \
        recentReleases: releases(first: {release_sample}, orderBy: {{ field: CREATED_AT, direction: DESC }}) {{ nodes {{ publishedAt }} }}",
        d30 = at(30),
        d90 = at(90),
        d365 = at(365),
        d180 = at(AUTHOR_WINDOW_DAYS),
        sample = AUTHOR_SAMPLE,
        release_sample = RELEASE_SAMPLE,
    )
}

/// GraphQL query for maintenance facts of `repos`, aliased `r0..rN` in order. Owner and name are
/// JSON-escaped. Callers chunk the repo list so each query stays small.
pub fn maintenance_query(repos: &[(String, String)], now: DateTime<Utc>) -> String {
    let fields = maintenance_selection(now);
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

/// Reads aliases `r0..r{count-1}` from a maintenance response. `None` is a repo that is gone.
pub fn parse_maintenance_batch(
    data: &Value,
    count: usize,
) -> Result<Vec<Option<GqlMaintenanceRepo>>> {
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

/// Raw facts about one repo, before they are reduced to a level and score.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaintenanceFacts {
    pub archived: bool,
    pub last_commit_at: Option<DateTime<Utc>>,
    pub commits_30d: u32,
    pub commits_90d: u32,
    pub commits_365d: u32,
    pub committers_180d: u32,
    pub release_dates: Vec<DateTime<Utc>>,
}

fn parse_ts(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// Identity of a commit author, or `None` for bots and authors with neither a login nor an email.
/// Logins win over emails so one person with several addresses counts once.
pub fn author_key(login: Option<&str>, email: Option<&str>) -> Option<String> {
    if let Some(login) = login.map(str::trim).filter(|l| !l.is_empty()) {
        if login.ends_with("[bot]") {
            return None;
        }
        return Some(format!("login:{}", login.to_ascii_lowercase()));
    }
    email
        .map(|e| e.trim().to_ascii_lowercase())
        .filter(|e| !e.is_empty())
        .map(|e| format!("email:{e}"))
}

/// Distinct human authors among the given commits.
pub fn committers(commits: &[GqlCommit]) -> u32 {
    let keys: HashSet<String> = commits
        .iter()
        .filter_map(|c| c.author.as_ref())
        .filter_map(|a| {
            author_key(
                a.user.as_ref().map(|u| u.login.as_str()),
                a.email.as_deref(),
            )
        })
        .collect();
    count(keys.len())
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Reads the maintenance facts out of one repo of the activity query.
pub fn facts_from_repo(repo: &GqlMaintenanceRepo) -> MaintenanceFacts {
    let target = repo
        .default_branch_ref
        .as_ref()
        .and_then(|b| b.target.as_ref());
    let last_commit_at = target
        .and_then(|t| t.latest_commit.nodes.first())
        .and_then(|c| c.committed_date.as_deref())
        .and_then(parse_ts);
    let release_dates = repo
        .recent_releases
        .as_ref()
        .map(|r| {
            r.nodes
                .iter()
                .filter_map(|n| n.published_at.as_deref().and_then(parse_ts))
                .collect()
        })
        .unwrap_or_default();
    MaintenanceFacts {
        archived: repo.is_archived,
        last_commit_at,
        commits_30d: target.map_or(0, |t| t.commits_30d.total_count),
        commits_90d: target.map_or(0, |t| t.commits_90d.total_count),
        commits_365d: target.map_or(0, |t| t.commits_365d.total_count),
        committers_180d: target.map_or(0, |t| committers(&t.recent_commits.nodes)),
        release_dates,
    }
}

/// Level, score and stored counts for one repo. `weekly` is the sparkline, or `None` to keep the
/// stored weeks.
pub fn summarize(
    facts: &MaintenanceFacts,
    weekly: Option<Vec<i32>>,
    now: DateTime<Utc>,
) -> MaintenanceSync {
    let days_since = |t: DateTime<Utc>| (now - t).num_days();
    let last_release = facts.release_dates.iter().max().copied();
    let year_ago = now - Duration::days(365);
    let releases_365d = facts
        .release_dates
        .iter()
        .filter(|t| **t >= year_ago)
        .count();
    let input = MaintenanceInput {
        archived: facts.archived,
        days_since_commit: facts.last_commit_at.map(days_since),
        commits_30d: facts.commits_30d,
        commits_90d: facts.commits_90d,
        commits_365d: facts.commits_365d,
        committers_180d: facts.committers_180d,
        days_since_release: last_release.map(days_since),
    };
    MaintenanceSync {
        last_commit_at: facts.last_commit_at,
        commits_30d: i32::try_from(facts.commits_30d).unwrap_or(i32::MAX),
        commits_90d: i32::try_from(facts.commits_90d).unwrap_or(i32::MAX),
        commits_365d: i32::try_from(facts.commits_365d).unwrap_or(i32::MAX),
        committers_180d: i32::try_from(facts.committers_180d).unwrap_or(i32::MAX),
        releases_365d: i32::try_from(releases_365d).unwrap_or(i32::MAX),
        weekly_commits: weekly,
        level: maintenance_level(input),
        score: maintenance_score(input),
    }
}

/// Last `SPARKLINE_WEEKS` entries of GitHub's `all` participation array (oldest first).
pub fn sparkline(all: &[i32]) -> Vec<i32> {
    all[all.len().saturating_sub(SPARKLINE_WEEKS)..].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 10, 12, 0, 0).unwrap()
    }

    fn facts() -> MaintenanceFacts {
        MaintenanceFacts {
            archived: false,
            last_commit_at: None,
            commits_30d: 0,
            commits_90d: 0,
            commits_365d: 0,
            committers_180d: 0,
            release_dates: Vec::new(),
        }
    }

    #[test]
    fn author_key_prefers_login_and_skips_bots() {
        assert_eq!(
            author_key(Some("Alice"), Some("a@x.test")),
            Some("login:alice".to_string())
        );
        assert_eq!(author_key(Some("dependabot[bot]"), None), None);
        assert_eq!(
            author_key(None, Some(" Bob@X.test ")),
            Some("email:bob@x.test".to_string())
        );
        assert_eq!(author_key(Some("  "), Some("")), None);
        assert_eq!(author_key(None, None), None);
    }

    #[test]
    fn committers_dedupes_case_insensitively() {
        let commit = |login: Option<&str>, email: Option<&str>| GqlCommit {
            committed_date: None,
            author: Some(GqlGitActor {
                email: email.map(str::to_string),
                user: login.map(|l| GqlUser {
                    login: l.to_string(),
                }),
            }),
        };
        let commits = vec![
            commit(Some("Alice"), None),
            commit(Some("alice"), Some("a@x.test")),
            commit(None, Some("bob@x.test")),
            commit(Some("renovate[bot]"), None),
            commit(None, None),
        ];
        assert_eq!(committers(&commits), 2);
    }

    #[test]
    fn release_counts_include_exactly_one_year_back() {
        let f = MaintenanceFacts {
            release_dates: vec![
                now() - Duration::days(365),
                now() - Duration::days(366),
                now() - Duration::days(10),
            ],
            ..facts()
        };
        let sync = summarize(&f, None, now());
        assert_eq!(sync.releases_365d, 2);
    }

    #[test]
    fn days_since_release_uses_the_newest_release() {
        let f = MaintenanceFacts {
            release_dates: vec![now() - Duration::days(200), now() - Duration::days(20)],
            ..facts()
        };
        let sync = summarize(&f, None, now());
        assert_eq!(sync.level, recomphub_shared::MaintenanceLevel::Active);
    }

    #[test]
    fn archived_repo_is_archived_with_zero_score() {
        let f = MaintenanceFacts {
            archived: true,
            last_commit_at: Some(now()),
            commits_30d: 50,
            ..facts()
        };
        let sync = summarize(&f, None, now());
        assert_eq!(sync.level, recomphub_shared::MaintenanceLevel::Archived);
        assert_eq!(sync.score, 0);
    }

    #[test]
    fn repo_without_history_is_dormant_with_zero_score() {
        let sync = summarize(&facts(), None, now());
        assert_eq!(sync.level, recomphub_shared::MaintenanceLevel::Dormant);
        assert_eq!(sync.score, 0);
        assert_eq!(sync.last_commit_at, None);
    }

    #[test]
    fn weekly_values_pass_through_or_keep_none() {
        let sync = summarize(&facts(), Some(vec![1, 2]), now());
        assert_eq!(sync.weekly_commits, Some(vec![1, 2]));
        assert_eq!(summarize(&facts(), None, now()).weekly_commits, None);
    }

    #[test]
    fn sparkline_keeps_the_last_twelve_weeks() {
        let all: Vec<i32> = (0..52).collect();
        assert_eq!(sparkline(&all), (40..52).collect::<Vec<i32>>());
        assert_eq!(sparkline(&[3, 4]), vec![3, 4]);
        assert!(sparkline(&[]).is_empty());
    }

    #[test]
    fn maintenance_query_measures_windows_back_from_now() {
        let query = maintenance_query(&[("o".to_string(), "n".to_string())], now());
        assert!(
            query.contains("r0: repository(owner: \"o\", name: \"n\")"),
            "{query}"
        );
        assert!(query.contains("isArchived defaultBranchRef"), "{query}");
        assert!(
            query.contains("commits30d: history(since: \"2026-09-10T12:00:00Z\")"),
            "{query}"
        );
        assert!(
            query.contains("commits365d: history(since: \"2025-10-10T12:00:00Z\")"),
            "{query}"
        );
        assert!(
            query.contains("recentCommits: history(since: \"2026-04-13T12:00:00Z\", first: 100)"),
            "{query}"
        );
        assert!(
            query.contains("recentReleases: releases(first: 100"),
            "{query}"
        );
    }
}
