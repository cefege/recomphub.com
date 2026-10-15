//! PortsDR import: parses `article.port-card` elements from the portsdr.com home page (plan §4).

use crate::github_links::canonical_repo_url;
use recomphub_shared::{PortKind, TargetPlatform};
use scraper::{ElementRef, Html, Selector};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortsDrCard {
    /// `h3` text: the game title as PortsDR shows it.
    pub game_title: String,
    /// `p.project-name`, or the repo name when missing.
    pub port_name: String,
    /// Canonical lowercase `https://github.com/{owner}/{repo}`.
    pub repo_url: String,
    /// `data-original-platform`, e.g. "Xbox 360".
    pub original_platform: Option<String>,
    /// Parsed from `data-platforms`, unknown labels dropped, deduplicated.
    pub declared_platforms: Vec<TargetPlatform>,
    /// `data-ai-assisted="true"`.
    pub ai_assisted: bool,
    pub kind: PortKind,
}

/// Parses every card that has a GitHub "Repository" link. Cards without one are skipped.
pub fn parse_cards(html: &str) -> Vec<PortsDrCard> {
    let doc = Html::parse_document(html);
    let card_sel = Selector::parse("article.port-card").expect("static selector");
    let h3_sel = Selector::parse("h3").expect("static selector");
    let name_sel = Selector::parse("p.project-name").expect("static selector");
    let a_sel = Selector::parse("a").expect("static selector");

    doc.select(&card_sel)
        .filter_map(|card| {
            let repo_url = card
                .select(&a_sel)
                .find(|a| text_of(*a) == "Repository")
                .and_then(|a| a.value().attr("href"))
                .and_then(canonical_repo_url)?;
            let game_title = card
                .select(&h3_sel)
                .next()
                .map(text_of)
                .filter(|s| !s.is_empty())?;
            let port_name = card
                .select(&name_sel)
                .next()
                .map(text_of)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| repo_url.rsplit('/').next().unwrap_or_default().to_string());
            let original_platform = card
                .value()
                .attr("data-original-platform")
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let declared_platforms =
                parse_platforms(card.value().attr("data-platforms").unwrap_or(""));
            let ai_assisted = card.value().attr("data-ai-assisted") == Some("true");
            let kind = kind_for(&port_name, &repo_url);
            Some(PortsDrCard {
                game_title,
                port_name,
                repo_url,
                original_platform,
                declared_platforms,
                ai_assisted,
                kind,
            })
        })
        .collect()
}

fn text_of(el: ElementRef<'_>) -> String {
    el.text().collect::<String>().trim().to_string()
}

/// `static_recomp` when the name or repo contains "recomp" (case-insensitive), else `decomp_port`.
fn kind_for(port_name: &str, repo_url: &str) -> PortKind {
    let haystack = format!("{port_name} {repo_url}").to_lowercase();
    if haystack.contains("recomp") {
        PortKind::StaticRecomp
    } else {
        PortKind::DecompPort
    }
}

fn parse_platforms(raw: &str) -> Vec<TargetPlatform> {
    let mut out: Vec<TargetPlatform> = Vec::new();
    for label in raw.split('|') {
        if let Some(p) = platform_for_label(label)
            && !out.contains(&p)
        {
            out.push(p);
        }
    }
    out
}

fn platform_for_label(label: &str) -> Option<TargetPlatform> {
    match label.trim().to_ascii_lowercase().as_str() {
        "windows" => Some(TargetPlatform::Windows),
        "linux" => Some(TargetPlatform::Linux),
        "macos" => Some(TargetPlatform::MacOs),
        "android" => Some(TargetPlatform::Android),
        "ios" => Some(TargetPlatform::Ios),
        "switch" => Some(TargetPlatform::Switch),
        "ps vita" => Some(TargetPlatform::PsVita),
        "psp" => Some(TargetPlatform::Psp),
        "3ds" => Some(TargetPlatform::N3ds),
        "wii u" => Some(TargetPlatform::WiiU),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/portsdr_card.html");

    #[test]
    fn parses_ace_combat_card() {
        let cards = parse_cards(FIXTURE);
        assert_eq!(cards.len(), 1);
        let c = &cards[0];
        assert_eq!(c.game_title, "Ace Combat 6: Fires of Liberation");
        assert_eq!(c.port_name, "AC6_recomp");
        assert_eq!(c.repo_url, "https://github.com/sal063/ac6_recomp");
        assert_eq!(c.declared_platforms, vec![TargetPlatform::Windows]);
        assert!(!c.ai_assisted);
        assert_eq!(c.original_platform.as_deref(), Some("Xbox 360"));
        assert_eq!(c.kind, PortKind::StaticRecomp);
    }

    #[test]
    fn skips_cards_without_github_repository_link() {
        let html = r#"<article class="port-card" data-platforms="Windows">
            <h3>Some Game</h3><p class="project-name">X</p>
            <a href="https://example.com/x">Repository</a></article>
            <article class="port-card" data-ai-assisted="true" data-platforms="Windows|PS Vita|Windows|Nope">
            <h3>Other Game</h3><p class="project-name">decomp_thing</p>
            <a href="https://github.com/a/b">Repository</a></article>"#;
        let cards = parse_cards(html);
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].game_title, "Other Game");
        assert_eq!(cards[0].kind, PortKind::DecompPort);
        assert!(cards[0].ai_assisted);
        assert_eq!(
            cards[0].declared_platforms,
            vec![TargetPlatform::Windows, TargetPlatform::PsVita]
        );
    }
}
