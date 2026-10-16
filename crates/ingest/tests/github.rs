//! GitHub contributor selection, GraphQL batching and author rows. No network.

use recomphub_ingest::detect::AiDetection;
use recomphub_ingest::github::{
    AuthorSpec, Contributor, ContributorStat, RepoPreview, author_specs, batch_query, parse_batch,
    parse_repo, select_contributors,
};
use recomphub_shared::TargetPlatform;
use serde_json::json;

fn stat(login: &str, contributions: i64) -> ContributorStat {
    ContributorStat {
        login: login.to_string(),
        avatar_url: format!("https://avatars.example/{login}"),
        contributions,
    }
}

fn logins(list: &[Contributor]) -> Vec<&str> {
    list.iter().map(|c| c.login.as_str()).collect()
}

fn preview(owner: &str, owner_is_org: bool, contributors: Vec<Contributor>) -> RepoPreview {
    RepoPreview {
        github_repo_id: 1,
        repo_url: format!("https://github.com/{owner}/game-port"),
        owner: owner.to_string(),
        owner_is_org,
        owner_name: Some("Owner Name".to_string()),
        owner_avatar_url: format!("https://avatars.example/{owner}"),
        name: "game-port".to_string(),
        description: None,
        homepage_url: None,
        default_branch: "main".to_string(),
        stars: 10,
        forks: 1,
        pushed_at: None,
        archived: false,
        license: None,
        latest_release_tag: None,
        latest_release_url: None,
        latest_release_at: None,
        latest_release_prerelease: false,
        release_assets: Vec::new(),
        detected_platforms: vec![TargetPlatform::Windows],
        ai: AiDetection {
            usage: None,
            tools: Vec::new(),
            evidence: Vec::new(),
        },
        readme_youtube_ids: Vec::new(),
        source: Default::default(),
        contributors,
    }
}

#[test]
fn contributors_need_at_least_ten_percent_of_commits() {
    // Total 100 including the owner. "ten" is exactly 10% and qualifies. "nine" is 9% and does not.
    let stats = vec![
        stat("owner", 60),
        stat("big", 21),
        stat("ten", 10),
        stat("nine", 9),
    ];
    let picked = select_contributors(&stats, "owner", false);
    assert_eq!(logins(&picked), vec!["big", "ten"]);
    assert_eq!(picked[0].commits, 21);
}

#[test]
fn owner_is_never_listed_as_a_contributor_regardless_of_case() {
    let stats = vec![stat("Owner", 90), stat("helper", 10)];
    let picked = select_contributors(&stats, "owner", false);
    assert_eq!(logins(&picked), vec!["helper"]);
}

#[test]
fn user_repos_keep_five_contributors_and_org_repos_keep_three() {
    let stats: Vec<ContributorStat> = (0..8).map(|i| stat(&format!("c{i}"), 100)).collect();
    assert_eq!(select_contributors(&stats, "owner", false).len(), 5);
    assert_eq!(select_contributors(&stats, "owner", true).len(), 3);
}

#[test]
fn no_commits_means_no_contributors() {
    assert!(select_contributors(&[stat("a", 0)], "owner", false).is_empty());
    assert!(select_contributors(&[], "owner", false).is_empty());
}

#[test]
fn user_owned_repo_lists_owner_first_then_credited_contributors() {
    let contributors = vec![Contributor {
        login: "helper".to_string(),
        avatar_url: "https://avatars.example/helper".to_string(),
        commits: 12,
    }];
    let specs = author_specs(&preview("solo", false, contributors));

    assert_eq!(specs.len(), 2);
    assert_eq!(specs[0].login, "solo");
    assert_eq!(specs[0].role, "owner");
    assert_eq!(specs[0].commits, None);
    assert_eq!(specs[1].login, "helper");
    assert_eq!(specs[1].role, "contributor");
    assert_eq!(specs[1].commits, Some(12));
}

#[test]
fn organization_repo_has_no_owner_row() {
    let contributors = vec![Contributor {
        login: "dev".to_string(),
        avatar_url: "https://avatars.example/dev".to_string(),
        commits: 3,
    }];
    let specs: Vec<AuthorSpec> = author_specs(&preview("bigorg", true, contributors));
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].login, "dev");
    assert_eq!(specs[0].role, "contributor");
}

#[test]
fn batch_query_aliases_each_repository_in_order() {
    let query = batch_query(&[
        ("alpha".to_string(), "one".to_string()),
        ("beta".to_string(), "two".to_string()),
    ]);
    assert!(query.contains("r0:"), "{query}");
    assert!(query.contains("r1:"), "{query}");
    assert!(query.contains("\"alpha\""), "{query}");
    assert!(query.contains("\"two\""), "{query}");
}

#[test]
fn parse_batch_reads_present_repos_and_null_as_deleted() {
    let data = json!({
        "r0": {
            "databaseId": 555,
            "nameWithOwner": "owner/new-name",
            "stargazerCount": 42,
            "forkCount": 3,
            "pushedAt": "2026-01-02T03:04:05Z",
            "isArchived": false,
            "homepageUrl": null,
            "licenseInfo": {"spdxId": "MIT"},
            "releases": {"nodes": []}
        },
        "r1": null
    });
    let parsed = parse_batch(&data, 2).expect("well-formed batch");
    assert_eq!(parsed.len(), 2);

    let repo = parsed[0].as_ref().expect("r0 exists");
    assert_eq!(repo.database_id, 555);
    assert_eq!(repo.name_with_owner, "owner/new-name");
    assert_eq!(repo.stargazer_count, 42);
    assert_eq!(
        repo.license_info
            .as_ref()
            .and_then(|l| l.spdx_id.as_deref()),
        Some("MIT")
    );

    assert!(parsed[1].is_none(), "a null alias is a deleted repository");
}

#[test]
fn parse_batch_fails_when_an_alias_is_missing() {
    let data = json!({"r0": null});
    assert!(parse_batch(&data, 2).is_err());
}

#[test]
fn parse_repo_splits_owner_and_name_and_rejects_other_hosts() {
    assert_eq!(
        parse_repo("https://github.com/owner/game-port").unwrap(),
        ("owner".to_string(), "game-port".to_string())
    );
    assert!(parse_repo("https://gitlab.com/owner/game-port").is_err());
}
