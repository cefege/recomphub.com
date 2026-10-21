//! Finds real screenshot images in README markdown or HTML, resolved to absolute URLs.
//! Filters out badges, shields, stat counters and logos, which fill most project READMEs
//! alongside actual screenshots. Pure; no network.

use regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;

/// One accepted README screenshot, retaining the contextual alt text for review and delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadmeImage {
    pub url: String,
    pub alt: String,
}
pub const MAX_IMAGES: usize = 6;

static CODE_FENCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)```.*?```").expect("static regex"));
static HTML_COMMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").expect("static regex"));
static MARKDOWN_IMAGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"!\[([^\]]*)\]\(\s*<?([^)\s>]+)>?(?:\s+"[^"]*")?\s*\)"#).expect("static regex")
});
static HTML_IMG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<img\b([^>]*)>").expect("static regex"));
static HTML_SRC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\bsrc\s*=\s*"([^"]*)"|\bsrc\s*=\s*'([^']*)'"#).expect("static regex")
});
static HTML_ALT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\balt\s*=\s*"([^"]*)"|\balt\s*=\s*'([^']*)'"#).expect("static regex")
});

/// Lowercase substrings in alt text or the resolved URL that mark decoration, not a screenshot.
const NOISE_WORDS: &[&str] = &[
    "badge",
    "shield",
    "license",
    "build status",
    "build-status",
    "ci status",
    "coverage",
    "downloads",
    "discord",
    "stars",
    "forks",
    "contributors",
    "last commit",
    "release",
    "version",
    "npm",
    "pypi",
    "docker pulls",
    "codecov",
    "sponsor",
    "ko-fi",
    "kofi",
    "buy me a coffee",
    "patreon",
    "paypal",
    "donate",
    "workflow",
    "visitors",
    "hits",
    "wakatime",
    "codacy",
    "sonarcloud",
    "snyk",
    "crates.io",
    "logo",
    "icon",
    "banner",
];

/// Extensions accepted as a real screenshot.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "avif"];

/// Hosts serving badges and stat cards, matched by exact hostname.
const BLOCKED_HOSTS: &[&str] = &[
    "shields.io",
    "img.shields.io",
    "badgen.net",
    "forthebadge.com",
    "github-readme-stats.vercel.app",
    "streak-stats.demolab.com",
    "github-profile-trophy.vercel.app",
    "komarev.com",
    "visitor-badge.laobi.icu",
    "visitor-badge.glitch.me",
    "hits.sh",
    "hitscounter.dev",
    "api.star-history.com",
    "codecov.io",
    "coveralls.io",
    "app.codacy.com",
    "sonarcloud.io",
    "snyk.io",
    "deepsource.io",
    "wakatime.com",
    "opencollective.com",
    "ko-fi.com",
    "img.buymeacoffee.com",
];

/// Hosts whose images are kept even without a recognized extension: GitHub's own attachment CDN
/// for files dragged into a README, issue or PR. `github.com` itself is kept only on an
/// `/assets/` path, which is how the newer attachment links are shaped.
const EXTENSIONLESS_HOSTS: &[&str] = &[
    "user-images.githubusercontent.com",
    "private-user-images.githubusercontent.com",
    "github.com",
];

fn host_of(url: &str) -> &str {
    let after_scheme = url.split("://").nth(1).unwrap_or(url);
    after_scheme.split(['/', '?', '#']).next().unwrap_or("")
}

fn path_of(url: &str) -> &str {
    let after_scheme = url.split("://").nth(1).unwrap_or(url);
    match after_scheme.find('/') {
        Some(i) => &after_scheme[i..],
        None => "",
    }
}

/// The last path segment (file name), ignoring any query string.
fn filename_of(url: &str) -> &str {
    let path = path_of(url);
    let path = path.split(['?', '#']).next().unwrap_or(path);
    path.rsplit('/').next().unwrap_or(path)
}

fn extension(url: &str) -> Option<&str> {
    let name = filename_of(url);
    name.contains('.')
        .then(|| name.rsplit('.').next().expect("segment contains a dot"))
}

/// Rewrites a GitHub `/blob/{branch}/{path}` page link to the raw content host. GitHub serves
/// `/blob/` URLs as an HTML page, not the image bytes, so without this a screenshot linked the
/// way GitHub's own "copy permalink" button produces it would fail to download.
fn rewrite_blob_url(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))?;
    let mut parts = rest.splitn(4, '/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    if parts.next()? != "blob" {
        return None;
    }
    let branch_and_path = parts.next()?;
    Some(format!(
        "https://raw.githubusercontent.com/{owner}/{repo}/{branch_and_path}"
    ))
}

/// Resolves a markdown/HTML image reference against the repository, or `None` for references
/// that cannot become a usable absolute URL (anchors, `mailto:`, inline data URIs, parent paths).
/// Strips any `#fragment` first, since a fragment is never part of the asset path, and rewrites
/// a GitHub `/blob/` page link to the raw content host.
fn resolve(raw_url: &str, owner: &str, repo: &str, branch: &str) -> Option<String> {
    let url = raw_url.split('#').next().unwrap_or(raw_url).trim();
    if url.is_empty() || url.starts_with("data:") || url.starts_with("mailto:") {
        return None;
    }
    if let Some(rest) = url.strip_prefix("//") {
        return Some(format!("https://{rest}"));
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        return Some(rewrite_blob_url(url).unwrap_or_else(|| url.to_string()));
    }
    if url.starts_with("..") {
        return None;
    }
    let trimmed = url.trim_start_matches("./").trim_start_matches('/');
    Some(format!(
        "https://raw.githubusercontent.com/{owner}/{repo}/{branch}/{trimmed}"
    ))
}

/// True when `url` (already resolved to absolute) and its alt text look like an actual
/// screenshot rather than a badge, stat card, sponsor button or logo. Noise words are checked
/// against the alt text and the file name only, never the host or repository path, so an owner
/// or repo name like "my-logo-game" does not suppress its own screenshots.
fn is_screenshot(url: &str, alt: &str) -> bool {
    let lower_url = url.to_ascii_lowercase();
    let lower_alt = alt.to_ascii_lowercase();
    let host = host_of(&lower_url);
    if BLOCKED_HOSTS.contains(&host) {
        return false;
    }
    let filename = filename_of(&lower_url);
    if NOISE_WORDS
        .iter()
        .any(|w| lower_alt.contains(w) || filename.contains(w))
    {
        return false;
    }
    match extension(&lower_url) {
        Some(ext) => IMAGE_EXTENSIONS.contains(&ext),
        None => {
            EXTENSIONLESS_HOSTS.contains(&host)
                && (host != "github.com" || lower_url.contains("/assets/"))
        }
    }
}
/// Real README screenshots in order of first appearance, deduped and capped at [`MAX_IMAGES`].
/// Relative paths resolve against `owner/repo` at `branch`; alt text is retained for moderation.
pub fn screenshot_urls_in_readme(
    readme: &str,
    owner: &str,
    repo: &str,
    branch: &str,
) -> Vec<ReadmeImage> {
    let no_comments = HTML_COMMENT.replace_all(readme, "");
    let cleaned = CODE_FENCE.replace_all(&no_comments, "");

    let mut hits: Vec<(usize, String, String)> = Vec::new();
    for caps in MARKDOWN_IMAGE.captures_iter(&cleaned) {
        let pos = caps.get(0).expect("whole match").start();
        let alt = caps
            .get(1)
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();
        let url = caps
            .get(2)
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();
        hits.push((pos, alt, url));
    }
    for caps in HTML_IMG.captures_iter(&cleaned) {
        let pos = caps.get(0).expect("whole match").start();
        let attrs = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
        let Some(src) = HTML_SRC
            .captures(attrs)
            .and_then(|c| c.get(1).or_else(|| c.get(2)))
            .map(|m| m.as_str().to_string())
        else {
            continue;
        };
        let alt = HTML_ALT
            .captures(attrs)
            .and_then(|c| c.get(1).or_else(|| c.get(2)))
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();
        hits.push((pos, alt, src));
    }
    hits.sort_by_key(|(pos, _, _)| *pos);

    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (_, alt, raw_url) in hits {
        if out.len() >= MAX_IMAGES {
            break;
        }
        let Some(url) = resolve(&raw_url, owner, repo, branch) else {
            continue;
        };
        if !is_screenshot(&url, &alt) || !seen.insert(url.clone()) {
            continue;
        }
        out.push(ReadmeImage { url, alt });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{ReadmeImage, screenshot_urls_in_readme};
    fn urls(readme: &str) -> Vec<String> {
        screenshot_urls_in_readme(readme, "octocat", "game-port", "main")
            .into_iter()
            .map(|image| image.url)
            .collect()
    }

    #[test]
    fn relative_markdown_image_resolves_against_the_repo() {
        assert_eq!(
            urls("![Gameplay](docs/screenshot.png)"),
            vec!["https://raw.githubusercontent.com/octocat/game-port/main/docs/screenshot.png"]
        );
    }

    #[test]
    fn absolute_github_asset_links_are_kept() {
        let readme = "![demo](https://user-images.githubusercontent.com/1/abc.png)\n\
                      ![demo2](https://github.com/octocat/game-port/assets/1/def-ghi)";
        assert_eq!(
            urls(readme),
            vec![
                "https://user-images.githubusercontent.com/1/abc.png",
                "https://github.com/octocat/game-port/assets/1/def-ghi",
            ]
        );
    }

    #[test]
    fn shield_badges_are_dropped_by_host_and_extension() {
        let readme = "[![Build](https://img.shields.io/github/actions/workflow/status/x/y/ci.yml)](ci)\n\
                      ![License](https://img.shields.io/badge/license-MIT-blue.svg)";
        assert!(urls(readme).is_empty());
    }

    #[test]
    fn logo_and_sponsor_images_are_dropped_by_alt_text() {
        let readme = "![Project logo](logo.png)\n![Sponsor](https://example.com/kofi.png)";
        assert!(urls(readme).is_empty());
    }

    #[test]
    fn html_img_tags_are_found_and_resolved() {
        let readme = r#"<p align="center"><img src="screenshots/menu.png" alt="Main menu"></p>"#;
        assert_eq!(
            urls(readme),
            vec!["https://raw.githubusercontent.com/octocat/game-port/main/screenshots/menu.png"]
        );
    }

    #[test]
    fn images_inside_fenced_code_blocks_are_ignored() {
        let readme = "```md\n![example](fake.png)\n```\n![real](real.png)";
        assert_eq!(
            urls(readme),
            vec!["https://raw.githubusercontent.com/octocat/game-port/main/real.png"]
        );
    }

    #[test]
    fn duplicates_are_dropped_and_first_appearance_wins() {
        let readme = "![a](shot.png)\n![b](./shot.png)";
        assert_eq!(
            urls(readme),
            vec!["https://raw.githubusercontent.com/octocat/game-port/main/shot.png"]
        );
    }

    #[test]
    fn result_is_capped_at_max_images() {
        let readme: String = (0..10).map(|i| format!("![s{i}](s{i}.png)\n")).collect();
        assert_eq!(urls(&readme).len(), super::MAX_IMAGES);
    }

    #[test]
    fn data_uris_and_anchors_are_skipped() {
        let readme = "![inline](data:image/png;base64,AAAA)\n![anchor](#usage)";
        assert!(urls(readme).is_empty());
    }

    #[test]
    fn extractor_retains_alt_text_with_its_resolved_url() {
        assert_eq!(
            screenshot_urls_in_readme(
                "![Gameplay HUD](docs/shot.png)",
                "my-logo-studio",
                "release-port",
                "main",
            ),
            vec![ReadmeImage {
                url: "https://raw.githubusercontent.com/my-logo-studio/release-port/main/docs/shot.png"
                    .to_string(),
                alt: "Gameplay HUD".to_string(),
            }],
        );
    }

    #[test]
    fn github_blob_links_are_rewritten_to_raw_content() {
        assert_eq!(
            urls("![Gameplay](https://github.com/octocat/game-port/blob/main/docs/shot.png)"),
            vec!["https://raw.githubusercontent.com/octocat/game-port/main/docs/shot.png"]
        );
    }

    #[test]
    fn fragments_are_stripped_before_resolving() {
        assert_eq!(
            urls("![Gameplay](shot.png#gh-dark-mode-only)"),
            vec!["https://raw.githubusercontent.com/octocat/game-port/main/shot.png"]
        );
    }

    #[test]
    fn github_user_attachments_links_are_kept() {
        assert_eq!(
            urls("![demo](https://github.com/user-attachments/assets/1234-abcd)"),
            vec!["https://github.com/user-attachments/assets/1234-abcd"]
        );
    }

    #[test]
    fn plain_text_without_images_yields_nothing() {
        assert!(urls("A port of the game. See the README for setup.").is_empty());
        assert!(urls("").is_empty());
    }
}
