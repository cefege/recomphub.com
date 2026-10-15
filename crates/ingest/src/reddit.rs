//! r/recomps Atom feed parser: extracts GitHub repositories linked from post content (plan §6).

use crate::github_links::canonical_repo_url;
use anyhow::Result;
use regex::Regex;
use std::collections::HashSet;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedditRepo {
    /// Canonical lowercase `https://github.com/{owner}/{repo}`.
    pub repo_url: String,
    /// Post link, or the entry id when no link is present.
    pub source_url: String,
    pub source_title: String,
}

/// Parses an Atom feed and returns each distinct repository once, at its first mention.
pub fn parse_feed(xml: &[u8]) -> Result<Vec<RedditRepo>> {
    let feed = feed_rs::parser::parse(xml).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for entry in &feed.entries {
        let body = entry
            .content
            .as_ref()
            .and_then(|c| c.body.clone())
            .unwrap_or_default();
        let summary = entry
            .summary
            .as_ref()
            .map(|s| s.content.clone())
            .unwrap_or_default();
        let source_url = entry
            .links
            .first()
            .map(|l| l.href.clone())
            .unwrap_or_else(|| entry.id.clone());
        let source_title = entry
            .title
            .as_ref()
            .map(|t| t.content.clone())
            .unwrap_or_default();
        for text in [body.as_str(), summary.as_str()] {
            for m in github_repo_re().find_iter(text) {
                let Some(repo_url) = canonical_repo_url(m.as_str()) else {
                    continue;
                };
                if seen.insert(repo_url.clone()) {
                    out.push(RedditRepo {
                        repo_url,
                        source_url: source_url.clone(),
                        source_title: source_title.clone(),
                    });
                }
            }
        }
    }
    Ok(out)
}

/// `https://github.com/{owner}/{repo}`, owner and repo required. Owner-only profile links do not match.
fn github_repo_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+").expect("static regex")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/recomps.atom");

    #[test]
    fn fixture_yields_github_repos() {
        let repos = parse_feed(FIXTURE).unwrap();
        assert!(
            !repos.is_empty(),
            "expected at least one repo from the live feed"
        );
        let distinct: HashSet<&str> = repos.iter().map(|r| r.repo_url.as_str()).collect();
        assert_eq!(distinct.len(), repos.len());
        for r in &repos {
            let path = r
                .repo_url
                .strip_prefix("https://github.com/")
                .expect("github prefix");
            assert_eq!(path.split('/').count(), 2, "{}", r.repo_url);
            assert_eq!(r.repo_url, r.repo_url.to_lowercase());
            assert!(!r.repo_url.ends_with(".git"));
        }
    }

    #[test]
    fn synthetic_entry_skips_profiles_strips_git_and_dedupes() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom"><title>t</title><id>x</id>
<updated>2026-01-01T00:00:00Z</updated>
<entry><id>e1</id><title>Post A</title>
<link href="https://www.reddit.com/r/recomps/comments/1/a/"/>
<updated>2026-01-01T00:00:00Z</updated>
<content type="html">&lt;a href="https://github.com/Foo/Bar.git"&gt;x&lt;/a&gt;
&lt;a href="https://github.com/Pulpparty"&gt;u&lt;/a&gt;
&lt;a href="https://github.com/foo/bar/releases"&gt;again&lt;/a&gt;</content></entry></feed>"#;
        let repos = parse_feed(xml.as_bytes()).unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].repo_url, "https://github.com/foo/bar");
        assert_eq!(repos[0].source_title, "Post A");
        assert_eq!(
            repos[0].source_url,
            "https://www.reddit.com/r/recomps/comments/1/a/"
        );
    }
}
