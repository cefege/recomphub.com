//! Amazon Associates search links for the "Get the original game" shelf. Pure and
//! deterministic: no Amazon API, prices or images are used.

use serde::{Deserialize, Serialize};

pub const AMAZON_DISCLOSURE: &str = "As an Amazon Associate I earn from qualifying purchases.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OfferKind {
    Original,
    Console,
    Controller,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuyOffer {
    pub kind: OfferKind,
    pub title: String,
    pub subtitle: String,
    pub badge: String,
    pub url: String,
}

struct Retail {
    slug: &'static str,
    label: &'static str,
    search: &'static str,
    console: Option<&'static str>,
    badge: &'static str,
}

const fn r(
    slug: &'static str,
    label: &'static str,
    search: &'static str,
    console: Option<&'static str>,
    badge: &'static str,
) -> Retail {
    Retail {
        slug,
        label,
        search,
        console,
        badge,
    }
}

/// Keyed by IGDB platform slug (see `ingest::platform_map`).
const RETAIL: &[Retail] = &[
    r(
        "n64",
        "Nintendo 64",
        "Nintendo 64",
        Some("Nintendo 64 console"),
        "N64",
    ),
    r("nes", "NES", "NES", Some("NES console"), "NES"),
    r(
        "snes",
        "SNES",
        "SNES",
        Some("Super Nintendo console"),
        "SNES",
    ),
    r(
        "ngc",
        "GameCube",
        "GameCube",
        Some("GameCube console"),
        "GCN",
    ),
    r(
        "wii",
        "Wii",
        "Nintendo Wii",
        Some("Nintendo Wii console"),
        "WII",
    ),
    r(
        "wiiu",
        "Wii U",
        "Wii U",
        Some("Nintendo Wii U console"),
        "WII U",
    ),
    r(
        "switch",
        "Switch",
        "Nintendo Switch",
        Some("Nintendo Switch console"),
        "NSW",
    ),
    r("gb", "Game Boy", "Game Boy", Some("Game Boy console"), "GB"),
    r(
        "gbc",
        "Game Boy Color",
        "Game Boy Color",
        Some("Game Boy Color console"),
        "GBC",
    ),
    r(
        "gba",
        "Game Boy Advance",
        "Game Boy Advance",
        Some("Game Boy Advance console"),
        "GBA",
    ),
    r(
        "nds",
        "Nintendo DS",
        "Nintendo DS",
        Some("Nintendo DS console"),
        "NDS",
    ),
    r(
        "3ds",
        "Nintendo 3DS",
        "Nintendo 3DS",
        Some("Nintendo 3DS console"),
        "3DS",
    ),
    r(
        "ps",
        "PlayStation",
        "PlayStation 1 PS1",
        Some("PlayStation 1 console"),
        "PS1",
    ),
    r(
        "ps2",
        "PlayStation 2",
        "PlayStation 2 PS2",
        Some("PlayStation 2 console"),
        "PS2",
    ),
    r(
        "ps3",
        "PlayStation 3",
        "PlayStation 3 PS3",
        Some("PlayStation 3 console"),
        "PS3",
    ),
    r(
        "ps4--1",
        "PlayStation 4",
        "PlayStation 4 PS4",
        Some("PlayStation 4 console"),
        "PS4",
    ),
    r(
        "ps5",
        "PlayStation 5",
        "PlayStation 5 PS5",
        Some("PlayStation 5 console"),
        "PS5",
    ),
    r("psp", "PSP", "PSP", Some("Sony PSP console"), "PSP"),
    r(
        "psvita",
        "PS Vita",
        "PS Vita",
        Some("PS Vita console"),
        "VITA",
    ),
    r(
        "xbox",
        "Xbox",
        "Original Xbox",
        Some("Original Xbox console"),
        "XBOX",
    ),
    r(
        "xbox360",
        "Xbox 360",
        "Xbox 360",
        Some("Xbox 360 console"),
        "X360",
    ),
    r(
        "xboxone",
        "Xbox One",
        "Xbox One",
        Some("Xbox One console"),
        "XONE",
    ),
    r(
        "series-x-s",
        "Xbox Series X|S",
        "Xbox Series X",
        Some("Xbox Series X console"),
        "XSX",
    ),
    r(
        "genesis-slash-megadrive",
        "Mega Drive",
        "Sega Genesis",
        Some("Sega Genesis console"),
        "GEN",
    ),
    r(
        "saturn",
        "Saturn",
        "Sega Saturn",
        Some("Sega Saturn console"),
        "SAT",
    ),
    r(
        "dc",
        "Dreamcast",
        "Sega Dreamcast",
        Some("Sega Dreamcast console"),
        "DC",
    ),
    r("win", "PC", "PC", None, "PC"),
];

fn valid_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.len() <= 64
        && tag.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn amazon_search_url(tag: &str, query: &str) -> String {
    let mut q = String::new();
    for b in query.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            q.push(b as char);
        } else {
            q.push_str(&format!("%{b:02X}"));
        }
    }
    format!("https://www.amazon.com/s?k={q}&i=videogames&tag={tag}")
}

/// Offers for one game. `platforms` is `(igdb slug, name)`. Empty without a valid tag.
pub fn game_offers(
    tag: Option<&str>,
    game_name: &str,
    platforms: &[(Option<&str>, &str)],
    has_pc_port: bool,
) -> Vec<BuyOffer> {
    let Some(tag) = tag.filter(|t| valid_tag(t)) else {
        return Vec::new();
    };
    let mapped: Vec<&Retail> = platforms
        .iter()
        .filter_map(|(slug, _)| slug.and_then(|s| RETAIL.iter().find(|x| x.slug == s)))
        .collect();

    let mut offers: Vec<BuyOffer> = Vec::new();
    let mut originals = 0;
    for row in &mapped {
        if originals == 3 {
            break;
        }
        let url = amazon_search_url(tag, &format!("{game_name} {}", row.search));
        if offers.iter().any(|o| o.url == url) {
            continue;
        }
        offers.push(BuyOffer {
            kind: OfferKind::Original,
            title: format!("{} edition", row.label),
            subtitle: game_name.to_string(),
            badge: row.badge.to_string(),
            url,
        });
        originals += 1;
    }
    if let Some((row, term)) = mapped.iter().find_map(|x| x.console.map(|c| (x, c))) {
        offers.push(BuyOffer {
            kind: OfferKind::Console,
            title: format!("{} console", row.label),
            subtitle: "Play the original on real hardware".to_string(),
            badge: row.badge.to_string(),
            url: amazon_search_url(tag, term),
        });
    }
    if has_pc_port {
        offers.push(BuyOffer {
            kind: OfferKind::Controller,
            title: "PC game controller".to_string(),
            subtitle: "For PC play; check device and game compatibility".to_string(),
            badge: "PAD".to_string(),
            url: amazon_search_url(tag, "Xbox wireless controller for PC"),
        });
    }
    offers.truncate(5);
    offers
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAG: Option<&str> = Some("funnybirthday-20");

    #[test]
    fn no_or_bad_tag_gives_nothing() {
        let p = [(Some("n64"), "N64")];
        assert!(game_offers(None, "G", &p, true).is_empty());
        assert!(game_offers(Some("a b"), "G", &p, true).is_empty());
        assert!(game_offers(Some("x&y"), "G", &p, true).is_empty());
    }

    #[test]
    fn builds_tagged_encoded_url() {
        let o = game_offers(TAG, "Zelda: OoT", &[(Some("n64"), "N64")], false);
        assert_eq!(o[0].kind, OfferKind::Original);
        assert!(o[0].url.contains("k=Zelda%3A%20OoT%20Nintendo%2064"));
        assert!(o[0].url.contains("tag=funnybirthday-20"));
        assert_eq!(o[1].kind, OfferKind::Console);
    }

    #[test]
    fn skips_unmapped() {
        let p = [(Some("arcade"), "Arcade"), (None, "x")];
        assert!(game_offers(TAG, "G", &p, false).is_empty());
    }

    #[test]
    fn caps_and_controller() {
        let p = [
            (Some("n64"), ""),
            (Some("ps"), ""),
            (Some("ps2"), ""),
            (Some("xbox"), ""),
            (Some("dc"), ""),
        ];
        let o = game_offers(TAG, "G", &p, true);
        assert_eq!(
            o.iter().filter(|x| x.kind == OfferKind::Original).count(),
            3
        );
        assert_eq!(o.len(), 5);
        assert!(
            !game_offers(TAG, "G", &p, false)
                .iter()
                .any(|x| x.kind == OfferKind::Controller)
        );
    }

    #[test]
    fn pc_only_has_no_console() {
        let o = game_offers(TAG, "G", &[(Some("win"), "PC")], false);
        assert_eq!(o.len(), 1);
    }
}
