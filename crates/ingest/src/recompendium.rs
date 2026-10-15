//! Recompendium (`nio03/unricopie`) YAML parser (plan §4). Tarball download lives in the worker.

use crate::github_links::normalize_repo_url;
use anyhow::{Result, bail};
use recomphub_shared::{AiSource, AiUsage, Playability, PortKind};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecompDir {
    Recomps,
    Ports,
    Decomps,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecompEntry {
    /// Game title with any trailing " (…)" removed.
    pub game_title: String,
    /// Port name: repo name for `recomps/` and `decomps/`, YAML `name` for `ports/`.
    pub port_name: String,
    pub repo_url: String,
    pub kind: PortKind,
    /// Raw console value (`console`, or the first of `consoles`), mapped by `platform_map`.
    pub source_platform: Option<String>,
    pub playability: Playability,
    pub ai_usage: AiUsage,
    pub ai_source: Option<AiSource>,
    pub authors: Vec<String>,
    pub homepage_url: Option<String>,
    pub toolchain: Option<String>,
    pub requirements: Option<String>,
    pub import_description: Option<String>,
    pub license: Option<String>,
}

#[derive(Deserialize)]
struct Raw {
    name: Option<String>,
    console: Option<String>,
    consoles: Option<Vec<String>>,
    status: Option<String>,
    repo: Option<String>,
    homepage: Option<String>,
    author: Option<String>,
    toolchain: Option<String>,
    category: Option<String>,
    license: Option<String>,
    desc: Option<serde_yaml::Value>,
    requirements: Option<serde_yaml::Value>,
    tags: Option<Vec<String>>,
}

/// Parses one YAML file. `path` is the file path inside the tarball, such as
/// `src/data/recomps/zelda-majoras-mask.yaml`; it determines the kind.
///
/// Returns `Ok(None)` for `mods/` and `tools/` files and for files without a `repo`.
pub fn parse_entry(path: &str, yaml: &str) -> Result<Option<RecompEntry>> {
    let segments: Vec<&str> = path.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    if segments.iter().any(|s| *s == "mods" || *s == "tools") {
        return Ok(None);
    }
    let dir = match segments.len().checked_sub(2).map(|i| segments[i]) {
        Some("recomps") => RecompDir::Recomps,
        Some("ports") => RecompDir::Ports,
        Some("decomps") => RecompDir::Decomps,
        _ => bail!("{path}: not under recomps/, ports/ or decomps/"),
    };

    let raw: Raw = serde_yaml::from_str(yaml).map_err(|e| anyhow::anyhow!("{path}: {e}"))?;
    let Some(repo) = raw.repo.as_deref().map(str::trim).filter(|r| !r.is_empty()) else {
        return Ok(None);
    };
    let Some((owner, repo_name)) = normalize_repo_url(&format!("https://github.com/{repo}")) else {
        bail!("{path}: invalid repo {repo:?}");
    };
    let repo_url = format!(
        "https://github.com/{}/{}",
        owner.to_lowercase(),
        repo_name.to_lowercase()
    );

    let yaml_name = non_empty(raw.name.as_deref());
    let title_source = yaml_name.clone().unwrap_or_else(|| repo_name.clone());
    let game_title = strip_trailing_parenthetical(&title_source);
    let port_name = match dir {
        RecompDir::Recomps | RecompDir::Decomps => repo_name.clone(),
        RecompDir::Ports => yaml_name.unwrap_or_else(|| repo_name.clone()),
    };

    let kind = match dir {
        RecompDir::Recomps => PortKind::StaticRecomp,
        RecompDir::Decomps => PortKind::Decompilation,
        RecompDir::Ports => match raw.category.as_deref().map(str::trim) {
            Some("fan-remake") => PortKind::FanRemake,
            Some("native-layer") => PortKind::NativeLayer,
            _ => PortKind::DecompPort,
        },
    };

    let playability = match raw.status.as_deref().map(str::trim) {
        Some("fully") | Some("stable") => Playability::Complete,
        Some("playable") | Some("usable") | Some("beta") => Playability::Playable,
        Some("experimental") => Playability::InProgress,
        _ => Playability::Unknown,
    };

    let tags = raw.tags.unwrap_or_default();
    let (ai_usage, ai_source) = if tags.iter().any(|t| t == "human-driven") {
        (AiUsage::None, Some(AiSource::Imported))
    } else if tags.iter().any(|t| t == "ai-assisted") {
        (AiUsage::Assisted, Some(AiSource::Imported))
    } else {
        (AiUsage::Unknown, None)
    };

    let source_platform = non_empty(raw.console.as_deref()).or_else(|| {
        raw.consoles
            .unwrap_or_default()
            .into_iter()
            .find_map(|c| non_empty(Some(c.as_str())))
    });

    let authors = raw
        .author
        .as_deref()
        .unwrap_or("")
        .split(" / ")
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(str::to_string)
        .collect();

    Ok(Some(RecompEntry {
        game_title,
        port_name,
        repo_url,
        kind,
        source_platform,
        playability,
        ai_usage,
        ai_source,
        authors,
        homepage_url: non_empty(raw.homepage.as_deref()),
        toolchain: non_empty(raw.toolchain.as_deref()),
        requirements: lang_en(raw.requirements.as_ref()),
        import_description: lang_en(raw.desc.as_ref()),
        license: non_empty(raw.license.as_deref()),
    }))
}

fn non_empty(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Removes a trailing " (…)" group: "Game (2001)" becomes "Game".
fn strip_trailing_parenthetical(title: &str) -> String {
    let t = title.trim();
    if t.ends_with(')')
        && let Some(i) = t.rfind(" (")
    {
        return t[..i].trim().to_string();
    }
    t.to_string()
}

/// `en` text of a language map, or the value itself when it is a plain string.
fn lang_en(v: Option<&serde_yaml::Value>) -> Option<String> {
    match v? {
        serde_yaml::Value::String(s) => non_empty(Some(s.as_str())),
        serde_yaml::Value::Mapping(m) => non_empty(m.get("en").and_then(|x| x.as_str())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZELDA: &str = include_str!("../tests/fixtures/zelda-majoras-mask.yaml");
    const ZELDA_PATH: &str = "src/data/recomps/zelda-majoras-mask.yaml";

    #[test]
    fn parses_majoras_mask_fixture() {
        let e = parse_entry(ZELDA_PATH, ZELDA).unwrap().expect("entry");
        assert_eq!(e.game_title, "The Legend of Zelda: Majora's Mask");
        assert_eq!(e.port_name, "Zelda64Recomp");
        assert_eq!(e.repo_url, "https://github.com/zelda64recomp/zelda64recomp");
        assert_eq!(e.kind, PortKind::StaticRecomp);
        assert_eq!(e.playability, Playability::Complete);
        assert_eq!(e.ai_usage, AiUsage::None);
        assert_eq!(e.ai_source, Some(AiSource::Imported));
        assert_eq!(
            e.authors,
            vec!["Zelda64Recomp".to_string(), "Mr. Wiseguy".to_string()]
        );
        assert_eq!(e.source_platform.as_deref(), Some("n64"));
    }

    #[test]
    fn ports_directory_uses_category_and_yaml_name() {
        let yaml = "name: Foo (Demo)\nconsoles: [gba]\nstatus: experimental\ncategory: fan-remake\n\
                    repo: A/B\ntags: [ai-assisted]\n";
        let e = parse_entry("src/data/ports/foo.yaml", yaml)
            .unwrap()
            .unwrap();
        assert_eq!(e.kind, PortKind::FanRemake);
        assert_eq!(e.port_name, "Foo (Demo)");
        assert_eq!(e.game_title, "Foo");
        assert_eq!(e.playability, Playability::InProgress);
        assert_eq!(e.ai_usage, AiUsage::Assisted);
        assert_eq!(e.source_platform.as_deref(), Some("gba"));
    }

    #[test]
    fn ports_without_category_are_decomp_ports() {
        let e = parse_entry("src/data/ports/x.yaml", "name: X\nrepo: a/x\n")
            .unwrap()
            .unwrap();
        assert_eq!(e.kind, PortKind::DecompPort);
    }

    #[test]
    fn decomps_directory_is_decompilation() {
        let e = parse_entry("src/data/decomps/sm64.yaml", "name: SM64\nrepo: n/sm64\n")
            .unwrap()
            .unwrap();
        assert_eq!(e.kind, PortKind::Decompilation);
        assert_eq!(e.port_name, "sm64");
    }

    #[test]
    fn skips_mods_tools_and_repoless_files() {
        assert_eq!(
            parse_entry("src/data/mods/x.yaml", "repo: a/b\n").unwrap(),
            None
        );
        assert_eq!(
            parse_entry("src/data/tools/x.yaml", "repo: a/b\n").unwrap(),
            None
        );
        assert_eq!(
            parse_entry("src/data/recomps/x.yaml", "name: X\n").unwrap(),
            None
        );
    }

    #[test]
    fn rejects_unknown_directory_and_bad_repo() {
        assert!(parse_entry("src/data/other/x.yaml", "repo: a/b\n").is_err());
        assert!(parse_entry("src/data/recomps/x.yaml", "repo: notarepo\n").is_err());
    }
}
