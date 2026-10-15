//! Console platform names from the import sources mapped to IGDB platform slugs (plan §4).
//! Slugs are confirmed at importer startup against IGDB's `/platforms` endpoint.

/// Source platform value (matched case-insensitively) to IGDB platform slug.
pub const CONSOLE_SLUGS: &[(&str, &str)] = &[
    ("n64", "n64"),
    ("Nintendo 64", "n64"),
    ("ps1", "ps"),
    ("PlayStation", "ps"),
    ("ps2", "ps2"),
    ("PlayStation 2", "ps2"),
    ("ps4", "ps4--1"),
    ("PlayStation 4", "ps4--1"),
    ("ps5", "ps5"),
    ("ps3", "ps3"),
    ("saturn", "saturn"),
    ("GameCube", "ngc"),
    ("wii", "wii"),
    ("wiiu", "wiiu"),
    ("Wii U", "wiiu"),
    ("xbox", "xbox"),
    ("Xbox 360", "xbox360"),
    ("x360", "xbox360"),
    ("xboxone", "xboxone"),
    ("snes", "snes"),
    ("nes", "nes"),
    ("nds", "nds"),
    ("Nintendo DS", "nds"),
    ("3ds", "3ds"),
    ("Nintendo 3DS", "3ds"),
    ("gba", "gba"),
    ("Game Boy Advance", "gba"),
    ("gb", "gb"),
    ("Game Boy", "gb"),
    ("gbc", "gbc"),
    ("Game Boy Color", "gbc"),
    ("megadrive", "genesis-slash-megadrive"),
    ("Mega Drive", "genesis-slash-megadrive"),
    ("dreamcast", "dc"),
    ("psp", "psp"),
    ("PlayStation Portable", "psp"),
    ("switch", "switch"),
    ("pc", "win"),
    ("arcade", "arcade"),
    ("Virtual Boy", "virtualboy"),
];

/// Distinct IGDB platform slugs referenced by [`CONSOLE_SLUGS`], verified at startup.
pub const IGDB_SLUGS: &[&str] = &[
    "n64",
    "ps",
    "ps2",
    "ps4--1",
    "ps5",
    "ps3",
    "saturn",
    "ngc",
    "wii",
    "wiiu",
    "xbox",
    "xbox360",
    "xboxone",
    "snes",
    "nes",
    "nds",
    "3ds",
    "gba",
    "gb",
    "gbc",
    "genesis-slash-megadrive",
    "dc",
    "psp",
    "switch",
    "win",
    "arcade",
    "virtualboy",
];

/// IGDB slug for a source platform value, matched case-insensitively.
pub fn igdb_slug_for(source: &str) -> Option<&'static str> {
    let source = source.trim();
    CONSOLE_SLUGS
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(source))
        .map(|(_, slug)| *slug)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn maps_keys_case_insensitively() {
        assert_eq!(igdb_slug_for("Nintendo 64"), Some("n64"));
        assert_eq!(igdb_slug_for("NINTENDO 64"), Some("n64"));
        assert_eq!(igdb_slug_for("ps1"), Some("ps"));
        assert_eq!(igdb_slug_for("playstation"), Some("ps"));
        assert_eq!(igdb_slug_for("Xbox 360"), Some("xbox360"));
        assert_eq!(igdb_slug_for("pc"), Some("win"));
        assert_eq!(igdb_slug_for("Atari 2600"), None);
    }

    #[test]
    fn igdb_slugs_are_distinct_and_cover_the_map() {
        let distinct: HashSet<&str> = IGDB_SLUGS.iter().copied().collect();
        assert_eq!(distinct.len(), IGDB_SLUGS.len());
        for (_, slug) in CONSOLE_SLUGS {
            assert!(distinct.contains(slug), "{slug} missing from IGDB_SLUGS");
        }
        for slug in IGDB_SLUGS {
            assert!(CONSOLE_SLUGS.iter().any(|(_, s)| s == slug));
        }
    }
}
