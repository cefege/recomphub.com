//! Human display names for ports whose name is still the repo slug.
//!
//! Order: the README's first H1 (cut at the first subtitle separator), then the first short
//! clause of the repo description, then the repo name prettified ("recomp-zelda" -> "Recomp Zelda").

const TITLE_MAX: usize = 80;
const DESCRIPTION_MAX: usize = 50;

/// Headings that say nothing about the game.
const GENERIC: &[&str] = &[
    "readme",
    "building",
    "build",
    "installation",
    "install",
    "overview",
    "introduction",
    "getting started",
    "welcome",
    "documentation",
    "docs",
    "usage",
    "setup",
    "project",
];

/// Words that keep their canonical spelling wherever they appear.
const KEEP: &[&str] = &["Recomp", "PC", "2D", "3D", "HD", "VR", "AI", "OS", "DOS"];

/// Name for a port from its README, description and repo name. Always returns something.
pub fn derive_display_name(readme: Option<&str>, description: Option<&str>, repo: &str) -> String {
    readme
        .and_then(readme_title)
        .or_else(|| description.and_then(description_clause))
        .unwrap_or_else(|| prettify(repo))
}

fn readme_title(readme: &str) -> Option<String> {
    let mut in_code = false;
    for line in readme.lines() {
        let line = line.trim();
        if line.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }
        if let Some(heading) = line.strip_prefix("# ") {
            return clean_title(&strip_markup(heading));
        }
    }
    None
}

fn description_clause(description: &str) -> Option<String> {
    let sentence = description.split(". ").next()?.trim();
    let clause = cut_subtitle(sentence).trim_end_matches('.');
    let clause = clause.trim();
    (clause.chars().count() <= DESCRIPTION_MAX)
        .then(|| clean_title(clause))
        .flatten()
}

/// Keeps the part before a subtitle separator and rejects empty, generic or overlong results.
fn clean_title(raw: &str) -> Option<String> {
    let title = cut_subtitle(raw)
        .trim()
        .trim_end_matches(['-', '|', ','])
        .trim();
    let len = title.chars().count();
    if !(3..=TITLE_MAX).contains(&len) || GENERIC.contains(&title.to_lowercase().as_str()) {
        return None;
    }
    Some(title.to_string())
}

/// Subtitle separators, tried in order.
const SEPARATORS: [&str; 5] = [":", "|", "—", "–", " - "];

/// Words that mark a suffix as platform or port wording rather than part of the title.
const PLATFORM_WORDS: &[&str] = &[
    "pc",
    "mac",
    "macos",
    "linux",
    "windows",
    "web",
    "native",
    "port",
    "recomp",
    "recompiled",
    "switch",
    "android",
    "ios",
    "steam",
    "deck",
    "raspberry",
    "browser",
    "wii",
    "3ds",
    "xbox",
];

/// Cuts at the first separator whose following segment (up to the next separator) is platform or
/// port wording. "Need for Speed: Most Wanted" stays whole; "Game PC: Mac/Linux Native" is cut;
/// "The Legend of Zelda: Majora's Mask - PC Port" is cut at " - ", keeping the subtitle.
fn cut_subtitle(s: &str) -> &str {
    let mut cuts: Vec<(usize, usize)> = SEPARATORS
        .iter()
        .flat_map(|sep| s.match_indices(sep).map(move |(at, _)| (at, sep.len())))
        .collect();
    cuts.sort_unstable();
    for (i, &(at, len)) in cuts.iter().enumerate() {
        let end = cuts.get(i + 1).map_or(s.len(), |next| next.0);
        let segment = &s[at + len..end];
        let platformy = segment
            .split(|c: char| !c.is_alphanumeric())
            .any(|w| PLATFORM_WORDS.contains(&w.to_lowercase().as_str()));
        if platformy {
            return &s[..at];
        }
    }
    s
}

/// Drops HTML tags, link targets, emphasis marks, images and symbols (emoji, ™).
fn strip_markup(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '<' => {
                for d in it.by_ref() {
                    if d == '>' {
                        break;
                    }
                }
            }
            ']' if it.peek() == Some(&'(') => {
                it.next();
                let mut depth = 1;
                for d in it.by_ref() {
                    match d {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
            '!' if it.peek() == Some(&'[') => {}
            '[' | ']' | '*' | '_' | '`' | '\\' => {}
            c if (c as u32) >= 0x2000 && !c.is_alphanumeric() => {}
            c => out.push(c),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// "recomp-zelda_ocarina" -> "Recomp Zelda Ocarina"; "PcPortGame" -> "Pc Port Game".
fn prettify(repo: &str) -> String {
    let mut spaced = String::new();
    let mut prev_lower = false;
    for c in repo.chars() {
        if c == '-' || c == '_' || c == '.' {
            spaced.push(' ');
            prev_lower = false;
        } else {
            if c.is_uppercase() && prev_lower {
                spaced.push(' ');
            }
            prev_lower = c.is_lowercase();
            spaced.push(c);
        }
    }
    spaced
        .split_whitespace()
        .map(word_case)
        .collect::<Vec<_>>()
        .join(" ")
}

fn word_case(word: &str) -> String {
    if let Some(known) = KEEP.iter().find(|k| k.eq_ignore_ascii_case(word)) {
        return (*known).to_string();
    }
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HARRY_POTTER_README: &str = "\
---

# Harry Potter and the Chamber of Secrets PC: Mac/Linux/Windows/Web Native

Play the 2002 game with a modern launcher on macOS, Windows, Linux, and Raspberry Pi.
";

    #[test]
    fn harry_potter_readme_h1_becomes_the_name() {
        let name = derive_display_name(
            Some(HARRY_POTTER_README),
            Some("Play the 2002 game with a modern launcher."),
            "harry-potter-2-chamber-of-secrets-pc",
        );
        assert_eq!(name, "Harry Potter and the Chamber of Secrets PC");
    }

    #[test]
    fn markdown_links_and_emoji_are_stripped() {
        let name = derive_display_name(
            Some("# [MyGame](https://example.com) Remaster 🎮 PC: Windows Native"),
            None,
            "mygame-remaster",
        );
        assert_eq!(name, "MyGame Remaster PC");
    }

    #[test]
    fn colon_titles_are_kept_whole() {
        let nfs = derive_display_name(Some("# Need for Speed: Most Wanted"), None, "nfsmw");
        assert_eq!(nfs, "Need for Speed: Most Wanted");
        let zelda = derive_display_name(
            Some("# The Legend of Zelda: Majora's Mask"),
            None,
            "zelda-mm",
        );
        assert_eq!(zelda, "The Legend of Zelda: Majora's Mask");
        let suffixed = derive_display_name(
            Some("# The Legend of Zelda: Majora's Mask - PC Port"),
            None,
            "zelda-mm-pc",
        );
        assert_eq!(suffixed, "The Legend of Zelda: Majora's Mask");
    }

    #[test]
    fn generic_heading_falls_back_to_description() {
        let name = derive_display_name(
            Some("# Building\n\nSteps..."),
            Some("Fast 2D platformer engine. Works everywhere."),
            "fast-engine",
        );
        assert_eq!(name, "Fast 2D platformer engine");
    }

    #[test]
    fn overlong_heading_and_long_description_fall_back_to_repo_name() {
        let long = format!("# {}", "word ".repeat(20));
        let name = derive_display_name(
            Some(&long),
            Some("A very long description that goes on and on past the limit of fifty characters."),
            "recomp-zelda-ocarina",
        );
        assert_eq!(name, "Recomp Zelda Ocarina");
    }

    #[test]
    fn no_sources_prettifies_repo_name() {
        assert_eq!(
            derive_display_name(None, None, "pc-port-camelCaseGame"),
            "PC Port Camel Case Game"
        );
        assert_eq!(
            derive_display_name(None, None, "nfsmw_recompiled"),
            "Nfsmw Recompiled"
        );
    }

    #[test]
    fn heading_inside_code_block_is_ignored() {
        let readme = "```sh\n# install deps\n```\n# Real Title\n";
        assert_eq!(derive_display_name(Some(readme), None, "x-y"), "Real Title");
    }
}
