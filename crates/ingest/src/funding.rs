//! Donation links for GitHub authors, refreshed by github-sync.
//!
//! Sources, in precedence order: `FUNDING.yml` in each of the login's port repositories (most
//! starred first), `FUNDING.yml` in the `<login>/.github` profile repository, donation URLs on
//! the profile (blog, social accounts, bio), and the login's GitHub Sponsors listing. For each
//! platform the first source that has a link wins; that source's links are kept in full (GitHub
//! and custom accept several values).

use anyhow::Result;
use chrono::{Duration, Utc};
use recomphub_db::PgPool;
use recomphub_db::author_funding::{self, FundingRecord, FundingSource};
use recomphub_shared::FundingPlatform;
use reqwest::StatusCode;
use serde_json::Value;

use crate::github::{GRAPHQL_BATCH, GithubClient, parse_repo};

/// A login is re-checked once its last check is older than this.
const FUNDING_TTL_DAYS: i64 = 7;
/// Logins looked up per github-sync run; the rest wait for the next run.
const FUNDING_LOGINS_PER_RUN: i64 = 100;
/// Port repositories checked per login for `FUNDING.yml`.
const REPOS_PER_LOGIN: i64 = 3;
/// GitHub and custom accept up to four values per key.
const MAX_VALUES_PER_KEY: usize = 4;
const MAX_URL_LEN: usize = 500;
const JSON: &str = "application/vnd.github+json";
const RAW: &str = "application/vnd.github.raw";
const ABSENT: &[StatusCode] = &[StatusCode::NOT_FOUND];
/// Paths GitHub reads `FUNDING.yml` from, in order.
const FUNDING_PATHS: &[&str] = &[".github/FUNDING.yml", "FUNDING.yml", "docs/FUNDING.yml"];

/// One platform link found in a source, before dedupe and precedence.
pub type Link = (FundingPlatform, String);

/// Refreshes funding for the logins whose check is due. Returns how many logins were checked.
/// Rate limits and outages stop the run; the failing login is not stamped, so it is retried first.
/// Other per-login failures are logged and the login is stamped, so one bad login cannot starve the queue.
/// A failed GraphQL batch fails the whole call so Sponsors links are never dropped on a flaky request.
pub async fn refresh_due(pool: &PgPool, client: &GithubClient) -> Result<usize> {
    let cutoff = Utc::now() - Duration::days(FUNDING_TTL_DAYS);
    let due = author_funding::due_logins(pool, cutoff, FUNDING_LOGINS_PER_RUN).await?;
    let mut logins = Vec::with_capacity(due.len());
    for login in due {
        if valid_login(&login) {
            logins.push(login);
        } else {
            // Not a GitHub login, so nothing can be fetched. Stamp it so it stops taking a slot.
            let mut conn = pool.acquire().await?;
            author_funding::mark_checked(&mut conn, &login).await?;
        }
    }

    let mut checked = 0;
    'batches: for chunk in logins.chunks(GRAPHQL_BATCH) {
        let sponsors = sponsors_listing(client, chunk).await?;
        for (login, has_sponsors) in chunk.iter().zip(sponsors) {
            match collect_links(pool, client, login, has_sponsors).await {
                Ok(records) => {
                    let mut tx = pool.begin().await?;
                    author_funding::replace_links(&mut tx, login, &records).await?;
                    tx.commit().await?;
                }
                Err(e) if is_transient(&e) => {
                    // Rate limit or outage: stop here. This login keeps its old links and is not
                    // stamped, so the next run retries it first.
                    tracing::warn!(login = %login, error = %format!("{e:#}"), "funding check hit a transient error; stopping run");
                    break 'batches;
                }
                Err(e) => {
                    // Specific to this login (e.g. a 451 or a malformed body). Stamp it so it does
                    // not block the queue; its old links stay until the next TTL.
                    tracing::warn!(login = %login, error = %format!("{e:#}"), "funding check failed; skipping login");
                    let mut conn = pool.acquire().await?;
                    author_funding::mark_checked(&mut conn, login).await?;
                }
            }
            checked += 1;
        }
    }
    Ok(checked)
}

/// Rate limits, outages and dropped connections. These stop the run; anything else is specific to
/// one login, so the run moves on.
fn is_transient(err: &anyhow::Error) -> bool {
    let transport = err.chain().any(|cause| {
        cause
            .downcast_ref::<reqwest::Error>()
            .is_some_and(|e| e.is_timeout() || e.is_connect())
    });
    let msg = format!("{err:#}");
    transport
        || msg.contains("HTTP 429")
        || msg.contains("HTTP 5")
        || (msg.contains("HTTP 403") && msg.contains("rate limit"))
}

/// Reads every source for one login and merges them into the rows to store.
async fn collect_links(
    pool: &PgPool,
    client: &GithubClient,
    login: &str,
    has_sponsors: bool,
) -> Result<Vec<FundingRecord>> {
    let mut sources: Vec<(FundingSource, Vec<Link>)> = Vec::new();

    for url in author_funding::owned_repo_urls(pool, login, REPOS_PER_LOGIN).await? {
        let Ok((owner, repo)) = parse_repo(&url) else {
            continue;
        };
        if let Some(body) = fetch_funding_file(client, &owner, &repo).await? {
            sources.push((FundingSource::Repo, parse_funding_yml(&body)));
        }
    }

    if let Some(body) = fetch_funding_file(client, login, ".github").await? {
        sources.push((FundingSource::Profile, parse_funding_yml(&body)));
    }
    sources.push((FundingSource::Profile, profile_links(client, login).await?));

    if has_sponsors {
        sources.push((
            FundingSource::Sponsors,
            vec![(FundingPlatform::Github, sponsors_url(login))],
        ));
    }

    Ok(merge(&sources))
}

/// Returns the first `FUNDING.yml` body found in `owner/repo`, or `None` if there is none.
async fn fetch_funding_file(
    client: &GithubClient,
    owner: &str,
    repo: &str,
) -> Result<Option<String>> {
    for path in FUNDING_PATHS {
        let api_path = format!("/repos/{owner}/{repo}/contents/{path}");
        if let Some(body) = client.rest_raw(&api_path, RAW, ABSENT).await? {
            return Ok(Some(body));
        }
    }
    Ok(None)
}

/// Donation links the login declares on its GitHub profile: the `blog` field, social accounts and
/// URLs in the bio. Only known donation hosts are kept.
async fn profile_links(client: &GithubClient, login: &str) -> Result<Vec<Link>> {
    let mut candidates: Vec<String> = Vec::new();
    if let Some(body) = client
        .rest_raw(&format!("/users/{login}"), JSON, ABSENT)
        .await?
    {
        let user: Value = serde_json::from_str(&body)?;
        if let Some(blog) = user.get("blog").and_then(Value::as_str) {
            candidates.push(blog.to_string());
        }
        if let Some(bio) = user.get("bio").and_then(Value::as_str) {
            candidates.extend(bio_urls(bio).map(str::to_string));
        }
    }
    if let Some(body) = client
        .rest_raw(&format!("/users/{login}/social_accounts"), JSON, ABSENT)
        .await?
    {
        let accounts: Value = serde_json::from_str(&body)?;
        candidates.extend(
            accounts
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|a| a.get("url").and_then(Value::as_str).map(str::to_string)),
        );
    }
    Ok(candidates.iter().filter_map(|c| donation_link(c)).collect())
}

/// Candidate URLs in a bio: whitespace-separated tokens minus surrounding punctuation. Only that
/// punctuation is trimmed, so a handle that ends in `-` or `_` survives.
fn bio_urls(bio: &str) -> impl Iterator<Item = &str> {
    bio.split_whitespace()
        .map(|token| token.trim_matches(BIO_PUNCTUATION))
}

const BIO_PUNCTUATION: &[char] = &[
    '(', ')', '[', ']', '<', '>', '{', '}', '.', ',', ';', ':', '!', '?', '"', '\'',
];

/// For each login, whether it has a GitHub Sponsors listing. Logins that are not users (orgs,
/// unknown names) come back `false`.
async fn sponsors_listing(client: &GithubClient, logins: &[String]) -> Result<Vec<bool>> {
    if logins.is_empty() {
        return Ok(Vec::new());
    }
    let data = client.graphql(&sponsors_query(logins)).await?;
    Ok((0..logins.len())
        .map(|i| {
            data.get(alias(i).as_str())
                .and_then(|u| u.get("hasSponsorsListing"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .collect())
}

fn alias(i: usize) -> String {
    format!("u{i}")
}

/// One aliased `repositoryOwner` field per login, so orgs are covered too. Callers validate logins
/// first.
fn sponsors_query(logins: &[String]) -> String {
    let fields: String = logins
        .iter()
        .enumerate()
        .map(|(i, login)| {
            format!(
                "{}: repositoryOwner(login: \"{login}\") {{ ... on Sponsorable {{ hasSponsorsListing }} }} ",
                alias(i)
            )
        })
        .collect();
    format!("{{ {fields}}}")
}

/// GitHub Sponsors profile URL for a login.
fn sponsors_url(login: &str) -> String {
    format!("https://github.com/sponsors/{login}")
}

/// A GitHub login: 1 to 39 ASCII letters, digits or hyphens. Anything else never reaches the
/// GraphQL query text or a REST path.
pub fn valid_login(login: &str) -> bool {
    (1..=39).contains(&login.len()) && login.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Parses a `FUNDING.yml` body into `(platform, url)` links. Unknown keys, malformed YAML and
/// unusable values are skipped; a bad file yields no links rather than an error.
pub fn parse_funding_yml(text: &str) -> Vec<Link> {
    let Ok(doc) = serde_yaml::from_str::<serde_yaml::Value>(text) else {
        return Vec::new();
    };
    let Some(map) = doc.as_mapping() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, value) in map {
        let Some(key) = key.as_str() else {
            continue;
        };
        let Ok(platform) = key.parse::<FundingPlatform>() else {
            continue;
        };
        // PayPal has no FUNDING key; it only appears inside `custom`.
        if platform == FundingPlatform::Paypal {
            continue;
        }
        let raw: Vec<String> = match value {
            serde_yaml::Value::String(s) => vec![s.clone()],
            serde_yaml::Value::Sequence(seq) => seq
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect(),
            _ => continue,
        };
        for value in raw.iter().take(MAX_VALUES_PER_KEY) {
            if platform == FundingPlatform::Custom {
                out.extend(custom_link(value));
            } else if let Some(url) = funding_url(platform, value) {
                out.push((platform, url));
            }
        }
    }
    out
}

/// The public URL for one `FUNDING.yml` value, or `None` if the value is not a valid handle.
pub fn funding_url(platform: FundingPlatform, value: &str) -> Option<String> {
    let v = value.trim();
    let url = match platform {
        FundingPlatform::Github => format!("https://github.com/sponsors/{}", handle(v)?),
        FundingPlatform::Patreon => format!("https://www.patreon.com/{}", handle(v)?),
        FundingPlatform::OpenCollective => format!("https://opencollective.com/{}", handle(v)?),
        FundingPlatform::KoFi => format!("https://ko-fi.com/{}", handle(v)?),
        FundingPlatform::Tidelift => {
            format!("https://tidelift.com/funding/github/{}", path(v, 2, 4)?)
        }
        FundingPlatform::CommunityBridge => {
            format!(
                "https://funding.communitybridge.org/projects/{}",
                handle(v)?
            )
        }
        FundingPlatform::Liberapay => format!("https://liberapay.com/{}", handle(v)?),
        FundingPlatform::Issuehunt => format!("https://issuehunt.io/r/{}", path(v, 1, 2)?),
        FundingPlatform::Polar => format!("https://polar.sh/{}", handle(v)?),
        FundingPlatform::BuyMeACoffee => format!("https://www.buymeacoffee.com/{}", handle(v)?),
        FundingPlatform::ThanksDev => format!("https://thanks.dev/{}", path(v, 1, 4)?),
        FundingPlatform::Paypal
        | FundingPlatform::Custom
        | FundingPlatform::Boosty
        | FundingPlatform::Fanbox => return None,
    };
    Some(url)
}

/// Turns one `custom:` entry into a link. A URL on a known donation host keeps that platform;
/// anything else is `Custom`.
pub fn custom_link(raw: &str) -> Option<Link> {
    let url = http_url(raw)?;
    Some(donation_link(url.as_str()).unwrap_or((FundingPlatform::Custom, url.to_string())))
}

/// A usable http(s) URL from a raw value. Values without a scheme get `https://`. Other schemes,
/// credentials, whitespace and over-long values are rejected.
fn http_url(raw: &str) -> Option<reqwest::Url> {
    let raw = raw.trim();
    if raw.is_empty()
        || raw.len() > MAX_URL_LEN
        || raw.chars().any(|c| c.is_whitespace() || c == '\\')
    {
        return None;
    }
    let full = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("https://{raw}")
    };
    let url = reqwest::Url::parse(&full).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    url.host_str()?;
    (url.as_str().len() <= MAX_URL_LEN).then_some(url)
}

/// The donation link for a URL from a profile or `custom:`, or `None` unless its host is a known
/// donation platform and the URL names a creator. A bare host (`ko-fi.com/`) names nobody. Fanbox
/// creators live on their own subdomain, so `<creator>.fanbox.cc` needs no path. PayPal needs
/// `paypal.me` or a `/donate` path; GitHub needs a `/sponsors/<login>` path.
pub fn donation_link(raw: &str) -> Option<Link> {
    let url = http_url(raw)?;
    let host = url.host_str()?;
    let host = host.strip_prefix("www.").unwrap_or(host);
    let path = url.path();
    let creator_subdomain = host.ends_with(".fanbox.cc");
    if path.trim_matches('/').is_empty() && !creator_subdomain {
        return None;
    }
    let platform = match host {
        "patreon.com" => FundingPlatform::Patreon,
        "ko-fi.com" => FundingPlatform::KoFi,
        "buymeacoffee.com" => FundingPlatform::BuyMeACoffee,
        "liberapay.com" => FundingPlatform::Liberapay,
        "opencollective.com" => FundingPlatform::OpenCollective,
        "polar.sh" => FundingPlatform::Polar,
        "thanks.dev" => FundingPlatform::ThanksDev,
        "boosty.to" => FundingPlatform::Boosty,
        "fanbox.cc" => FundingPlatform::Fanbox,
        "paypal.me" => FundingPlatform::Paypal,
        "paypal.com" if path.starts_with("/donate") => FundingPlatform::Paypal,
        "github.com"
            if path
                .strip_prefix("/sponsors/")
                .is_some_and(|login| !login.is_empty()) =>
        {
            FundingPlatform::Github
        }
        _ if creator_subdomain => FundingPlatform::Fanbox,
        _ => return None,
    };
    Some((platform, url.to_string()))
}

/// Merges sources in precedence order. A platform is taken from the first source that has at
/// least one link for it; that source contributes all of its links for the platform, deduped.
pub fn merge(sources: &[(FundingSource, Vec<Link>)]) -> Vec<FundingRecord> {
    let mut decided: Vec<FundingPlatform> = Vec::new();
    let mut out: Vec<FundingRecord> = Vec::new();
    for (source, links) in sources {
        let mut claimed: Vec<FundingPlatform> = Vec::new();
        for (platform, url) in links {
            if decided.contains(platform) {
                continue;
            }
            if out.iter().any(|r| r.platform == *platform && r.url == *url) {
                continue;
            }
            claimed.push(*platform);
            out.push(FundingRecord {
                platform: *platform,
                url: url.clone(),
                source: *source,
            });
        }
        decided.extend(claimed);
    }
    out
}

/// A single handle (no `/`), e.g. a username or project slug.
fn handle(v: &str) -> Option<&str> {
    path(v, 1, 1)
}

/// `v` split on `/` into between `min` and `max` segments, each a safe path component.
fn path(v: &str, min: usize, max: usize) -> Option<&str> {
    if v.is_empty() || v.len() > 100 {
        return None;
    }
    let segments: Vec<&str> = v.split('/').collect();
    let ok = (min..=max).contains(&segments.len())
        && segments.iter().all(|s| {
            !s.is_empty()
                && *s != "."
                && *s != ".."
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@'))
        });
    ok.then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOCS_EXAMPLE: &str = r#"
# These are supported funding model platforms
github: [octocat, cat]  # Replace with up to 4 GitHub Sponsors-enabled usernames
patreon: octocat
open_collective: octocat
ko_fi: octocat
tidelift: npm/octocat
community_bridge: octocat
liberapay: octocat
issuehunt: octocat
otechie: octocat
lfx_crowdfunding: octocat
custom: ["https://www.paypal.me/octocat", octocat.com]
"#;

    fn urls(links: &[Link]) -> Vec<String> {
        links.iter().map(|(_, u)| u.clone()).collect()
    }

    #[test]
    fn docs_example_maps_every_supported_platform() {
        let links = parse_funding_yml(DOCS_EXAMPLE);
        assert_eq!(
            urls(&links),
            vec![
                "https://github.com/sponsors/octocat",
                "https://github.com/sponsors/cat",
                "https://www.patreon.com/octocat",
                "https://opencollective.com/octocat",
                "https://ko-fi.com/octocat",
                "https://tidelift.com/funding/github/npm/octocat",
                "https://funding.communitybridge.org/projects/octocat",
                "https://liberapay.com/octocat",
                "https://issuehunt.io/r/octocat",
                "https://www.paypal.me/octocat",
                "https://octocat.com/",
            ]
        );
        assert_eq!(links[9].0, FundingPlatform::Paypal);
        assert_eq!(links[10].0, FundingPlatform::Custom);
    }

    #[test]
    fn github_values_are_capped_at_four() {
        let text = "github: [a, b, c, d, e, f]\n";
        assert_eq!(parse_funding_yml(text).len(), 4);
    }

    #[test]
    fn scalar_and_single_item_list_are_equivalent() {
        assert_eq!(
            parse_funding_yml("patreon: octocat\n"),
            parse_funding_yml("patreon: [octocat]\n")
        );
    }

    #[test]
    fn bad_yaml_and_non_mapping_yield_nothing() {
        assert!(parse_funding_yml("github: [unclosed").is_empty());
        assert!(parse_funding_yml("- just\n- a list\n").is_empty());
        assert!(parse_funding_yml("").is_empty());
    }

    #[test]
    fn unknown_keys_and_bad_values_are_skipped() {
        let text = "otechie: x\nko_fi: [\"ok\", \"bad/slash/../x\", \"\"]\npatreon: 42\n";
        assert_eq!(urls(&parse_funding_yml(text)), vec!["https://ko-fi.com/ok"]);
    }

    #[test]
    fn handles_reject_path_tricks() {
        assert_eq!(funding_url(FundingPlatform::Patreon, "../admin"), None);
        assert_eq!(funding_url(FundingPlatform::Patreon, "a b"), None);
        assert_eq!(funding_url(FundingPlatform::Patreon, "a/b"), None);
        assert_eq!(funding_url(FundingPlatform::Tidelift, "npm"), None);
        assert_eq!(
            funding_url(FundingPlatform::Tidelift, "npm/@scope/pkg").as_deref(),
            Some("https://tidelift.com/funding/github/npm/@scope/pkg")
        );
        assert_eq!(
            funding_url(FundingPlatform::ThanksDev, "u/gh/octocat").as_deref(),
            Some("https://thanks.dev/u/gh/octocat")
        );
        assert_eq!(funding_url(FundingPlatform::Paypal, "x"), None);
        assert_eq!(funding_url(FundingPlatform::Custom, "x"), None);
    }

    #[test]
    fn custom_links_are_classified_and_validated() {
        assert_eq!(
            custom_link("https://paypal.com/donate"),
            Some((FundingPlatform::Paypal, "https://paypal.com/donate".into()))
        );
        assert_eq!(
            custom_link("octocat.com"),
            Some((FundingPlatform::Custom, "https://octocat.com/".into()))
        );
        assert_eq!(custom_link("ftp://example.com"), None);
        assert_eq!(custom_link("javascript:alert(1)"), None);
        assert_eq!(custom_link("https://user:pw@example.com"), None);
        assert_eq!(custom_link("https://exa mple.com"), None);
        assert_eq!(custom_link(""), None);
        assert_eq!(
            custom_link(&format!("https://e.com/{}", "a".repeat(600))),
            None
        );
    }

    #[test]
    fn logins_are_validated() {
        assert!(valid_login("octocat"));
        assert!(valid_login("a-b-1"));
        assert!(!valid_login(""));
        assert!(!valid_login("a.b"));
        assert!(!valid_login("a\"b"));
        assert!(!valid_login(&"a".repeat(40)));
    }

    #[test]
    fn sponsors_query_has_one_alias_per_login() {
        let q = sponsors_query(&["alpha".to_string(), "beta".to_string()]);
        assert_eq!(
            q,
            "{ u0: repositoryOwner(login: \"alpha\") { ... on Sponsorable { hasSponsorsListing } } u1: repositoryOwner(login: \"beta\") { ... on Sponsorable { hasSponsorsListing } } }"
        );
    }

    #[test]
    fn donation_hosts_are_kept_and_others_dropped() {
        let kept = [
            ("https://www.patreon.com/x", FundingPlatform::Patreon),
            ("https://ko-fi.com/x", FundingPlatform::KoFi),
            ("buymeacoffee.com/x", FundingPlatform::BuyMeACoffee),
            ("https://liberapay.com/x", FundingPlatform::Liberapay),
            (
                "https://opencollective.com/x",
                FundingPlatform::OpenCollective,
            ),
            ("https://paypal.me/x", FundingPlatform::Paypal),
            ("https://paypal.com/donate/?x=1", FundingPlatform::Paypal),
            ("https://github.com/sponsors/x", FundingPlatform::Github),
            ("https://polar.sh/x", FundingPlatform::Polar),
            ("https://thanks.dev/x", FundingPlatform::ThanksDev),
            ("https://boosty.to/x", FundingPlatform::Boosty),
            ("https://fanbox.cc/@x", FundingPlatform::Fanbox),
            ("https://creator.fanbox.cc/", FundingPlatform::Fanbox),
        ];
        for (raw, platform) in kept {
            assert_eq!(donation_link(raw).map(|(p, _)| p), Some(platform), "{raw}");
        }
        for raw in [
            "https://itch.io/profile/x",
            "https://github.com/x",
            "https://github.com/sponsors/",
            "https://paypal.com/shop",
            "https://evil-patreon.com/x",
            "https://patreon.com.evil.net/x",
            "https://example.com",
            "hello",
            "mailto:x@patreon.com",
            "https://patreon.com",
            "https://www.ko-fi.com",
            "https://ko-fi.com/",
            "https://paypal.me/",
            "https://fanbox.cc/",
            "https://evilfanbox.cc/x",
            "javascript:alert(1)",
        ] {
            assert_eq!(donation_link(raw), None, "{raw}");
        }
    }

    #[test]
    fn bio_punctuation_is_trimmed_and_handles_keep_their_ends() {
        assert_eq!(
            bio_urls("(https://ko-fi.com/x),").collect::<Vec<_>>(),
            vec!["https://ko-fi.com/x"]
        );
        assert_eq!(
            donation_link("ko-fi.com/foo_").map(|(_, u)| u),
            Some("https://ko-fi.com/foo_".to_string())
        );
        assert_eq!(
            custom_link("ko-fi.com/foo-").map(|(_, u)| u),
            Some("https://ko-fi.com/foo-".to_string())
        );
        assert!(
            bio_urls("find me on patreon.com! or ko-fi.com.")
                .filter_map(donation_link)
                .next()
                .is_none()
        );
    }

    #[test]
    fn mixed_value_shapes_parse_per_key() {
        let text =
            "github:\npatreon:\nko_fi: [octocat]\ncustom: octocat.com\nopen_collective: [\"oc\"]\n";
        assert_eq!(
            urls(&parse_funding_yml(text)),
            vec![
                "https://ko-fi.com/octocat",
                "https://octocat.com/",
                "https://opencollective.com/oc",
            ]
        );
    }

    #[test]
    fn custom_values_with_other_schemes_are_dropped() {
        let text = "custom: [\"mailto:a@b.com\", \"javascript:x\", \"ftp://x.com\"]\n";
        assert!(parse_funding_yml(text).is_empty());
    }

    #[test]
    fn only_rate_limits_outages_and_transport_stop_the_run() {
        let err = |m: &str| anyhow::anyhow!("{m}");
        assert!(is_transient(&err(
            "GET /x returned HTTP 429 Too Many Requests: slow down"
        )));
        assert!(is_transient(&err(
            "GET /x returned HTTP 503 Service Unavailable: x"
        )));
        assert!(is_transient(&err(
            "GET /x returned HTTP 403 Forbidden: API rate limit exceeded"
        )));
        assert!(!is_transient(&err(
            "GET /x returned HTTP 403 Forbidden: Repository access blocked"
        )));
        assert!(!is_transient(&err("GET /x returned HTTP 404 Not Found: x")));
        assert!(!is_transient(&err(
            "GET /x returned HTTP 451 Unavailable For Legal Reasons: x"
        )));
        assert!(!is_transient(&err("parsing GET /x")));
    }

    #[test]
    fn vue_funding_file_parses_to_its_live_platforms() {
        let text = "# These are supported funding model platforms\n\ngithub: [yyx990803, posva]\npatreon: evanyou\nopen_collective: vuejs\nko_fi: # Replace with a single Ko-fi username\ntidelift: npm/vue\ncustom: # Replace with a single custom sponsorship URL\n";
        let platforms: Vec<FundingPlatform> = parse_funding_yml(text)
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        assert_eq!(
            platforms,
            vec![
                FundingPlatform::Github,
                FundingPlatform::Github,
                FundingPlatform::Patreon,
                FundingPlatform::OpenCollective,
                FundingPlatform::Tidelift,
            ]
        );
    }

    #[test]
    fn merge_takes_first_source_per_platform_and_keeps_all_its_links() {
        let sources = vec![
            (
                FundingSource::Repo,
                vec![(
                    FundingPlatform::Github,
                    "https://github.com/sponsors/a".to_string(),
                )],
            ),
            (
                FundingSource::Profile,
                vec![
                    (
                        FundingPlatform::Github,
                        "https://github.com/sponsors/b".to_string(),
                    ),
                    (
                        FundingPlatform::Patreon,
                        "https://www.patreon.com/a".to_string(),
                    ),
                ],
            ),
            (
                FundingSource::Sponsors,
                vec![(FundingPlatform::Github, sponsors_url("a"))],
            ),
        ];
        let merged = merge(&sources);
        assert_eq!(
            merged,
            vec![
                FundingRecord {
                    platform: FundingPlatform::Github,
                    url: "https://github.com/sponsors/a".into(),
                    source: FundingSource::Repo,
                },
                FundingRecord {
                    platform: FundingPlatform::Patreon,
                    url: "https://www.patreon.com/a".into(),
                    source: FundingSource::Profile,
                },
            ]
        );
    }

    #[test]
    fn merge_keeps_all_links_from_the_winning_source_without_duplicates() {
        let sources = vec![(
            FundingSource::Repo,
            vec![
                (FundingPlatform::Custom, "https://a.example/".to_string()),
                (FundingPlatform::Custom, "https://b.example/".to_string()),
                (FundingPlatform::Custom, "https://a.example/".to_string()),
            ],
        )];
        assert_eq!(merge(&sources).len(), 2);
    }
}
