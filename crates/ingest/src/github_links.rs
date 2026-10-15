//! GitHub repository URL normalization, shared by submissions, discovery and importers.

/// Parses a GitHub repository URL into `(owner, repo)`, keeping the original case.
///
/// Accepts `https://github.com/o/r`, `http://`, `www.`, scheme-less `github.com/o/r`,
/// trailing slashes, a `.git` suffix, query strings, fragments and extra path segments.
/// Returns `None` for other hosts and for URLs without an owner and repo.
pub fn normalize_repo_url(input: &str) -> Option<(String, String)> {
    let trimmed = input.trim();
    let rest = strip_prefix_ci(trimmed, "https://")
        .or_else(|| strip_prefix_ci(trimmed, "http://"))
        .unwrap_or(trimmed);
    let (host, path) = rest.split_once('/')?;
    let host = host.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    if host != "github.com" {
        return None;
    }
    let path = path.split(['?', '#']).next().unwrap_or("");
    let mut segments = path.split('/').filter(|s| !s.is_empty());
    let owner = segments.next()?;
    let repo = segments.next()?;
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    if valid_owner(owner) && valid_repo(repo) {
        Some((owner.to_string(), repo.to_string()))
    } else {
        None
    }
}

/// Canonical `https://github.com/{owner}/{repo}` URL, lowercased, as stored in `ports.repo_url`.
pub fn canonical_repo_url(input: &str) -> Option<String> {
    normalize_repo_url(input).map(|(owner, repo)| {
        format!(
            "https://github.com/{}/{}",
            owner.to_lowercase(),
            repo.to_lowercase()
        )
    })
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len()
        && s.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
    {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

fn valid_owner(owner: &str) -> bool {
    !owner.is_empty() && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn valid_repo(repo: &str) -> bool {
    !repo.is_empty()
        && repo != "."
        && repo != ".."
        && repo
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_common_forms() {
        let want = Some(("sal063".to_string(), "AC6_recomp".to_string()));
        assert_eq!(
            normalize_repo_url("https://github.com/sal063/AC6_recomp"),
            want
        );
        assert_eq!(normalize_repo_url("github.com/sal063/AC6_recomp"), want);
        assert_eq!(
            normalize_repo_url("https://www.github.com/sal063/AC6_recomp/"),
            want
        );
        assert_eq!(
            normalize_repo_url("  https://github.com/sal063/AC6_recomp.git  "),
            want
        );
        assert_eq!(
            normalize_repo_url("https://github.com/sal063/AC6_recomp/tree/main/src?x=1#y"),
            want
        );
    }

    #[test]
    fn rejects_other_hosts_and_incomplete_urls() {
        assert_eq!(normalize_repo_url("https://gitlab.com/o/r"), None);
        assert_eq!(normalize_repo_url("https://github.com.evil.io/o/r"), None);
        assert_eq!(normalize_repo_url("https://github.com/owner"), None);
        assert_eq!(normalize_repo_url("https://github.com/"), None);
        assert_eq!(normalize_repo_url("o/r"), None);
        assert_eq!(normalize_repo_url("https://github.com/o/.."), None);
    }

    #[test]
    fn canonical_is_lowercase_without_suffix() {
        assert_eq!(
            canonical_repo_url("https://GitHub.com/Sal063/AC6_recomp.git/").as_deref(),
            Some("https://github.com/sal063/ac6_recomp")
        );
    }
}
