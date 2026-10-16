/// Maximum length of a meta description before it is cut on a word boundary.
pub const DESCRIPTION_MAX: usize = 155;

/// Cut `text` to at most `max` characters, on a word boundary, appending `…` when cut.
/// The result, including the ellipsis, never exceeds `max` characters.
pub fn truncate_description(text: &str, max: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max {
        return collapsed;
    }
    let budget = max.saturating_sub(1); // room for the ellipsis
    let cut: String = collapsed.chars().take(budget).collect();
    let trimmed = match cut.rfind(' ') {
        Some(idx) => cut[..idx].trim_end(),
        None => cut.trim_end(),
    };
    format!("{trimmed}…")
}

/// Meta description, cut to `DESCRIPTION_MAX`.
pub fn meta_description(text: &str) -> String {
    truncate_description(text, DESCRIPTION_MAX)
}

/// Title for a game page with at least one port: `{Game} Native Port & Recomp – {targets} | RecompHub`.
/// `targets` holds up to three primary target labels.
pub fn game_with_ports_title(game: &str, targets: &[&str]) -> String {
    let shown: Vec<&str> = targets.iter().copied().take(3).collect();
    if shown.is_empty() {
        format!("{game} Native Port & Recomp | RecompHub")
    } else {
        format!(
            "{game} Native Port & Recomp – {} | RecompHub",
            shown.join(", ")
        )
    }
}

/// Title for a game page with no port.
pub fn game_without_port_title(game: &str) -> String {
    format!("{game} Native Port? Recomp & Decomp Status | RecompHub")
}

/// Title for a port page.
pub fn port_title(port: &str, game: &str, kind_label: &str, targets: &[&str]) -> String {
    let targets = if targets.is_empty() {
        "PC".to_string()
    } else {
        targets.join(", ")
    };
    format!("{port} – {game} {kind_label} for {targets} | RecompHub")
}

/// Title for a platform hub.
pub fn platform_title(platform: &str, games: i64) -> String {
    format!("{platform} Game Ports & Recompilations ({games} games) | RecompHub")
}

/// Title for a platform-target hub.
pub fn platform_target_title(platform: &str, target: &str) -> String {
    format!("{platform} Games on {target}: Native Ports | RecompHub")
}

/// Title for a runs-on hub.
pub fn runs_on_title(target: &str) -> String {
    format!("Games Running Natively on {target} | RecompHub")
}

/// Title for an author page.
pub fn author_title(name: &str, login: &str) -> String {
    format!("{name} (@{login}) – Game Ports & Recomps | RecompHub")
}

/// Title for the home page.
pub fn home_title() -> &'static str {
    "RecompHub – Every Game Recompilation & Native Port"
}

/// Description for a game that has ports.
pub fn game_with_ports_description(
    game: &str,
    platform: &str,
    year: Option<i32>,
    port_names: &[&str],
    targets: &[&str],
) -> String {
    let year = year.map(|y| format!(", {y}")).unwrap_or_default();
    let n = port_names.len();
    let plural = if n == 1 { "port" } else { "ports" };
    let names = port_names.join(", ");
    let runs_on = if targets.is_empty() {
        String::new()
    } else {
        format!(" Runs on {}.", targets.join(", "))
    };
    let text = format!(
        "{game} ({platform}{year}) has {n} native {plural}: {names}.{runs_on} Compare GitHub stars, releases, AI usage and gameplay videos."
    );
    meta_description(&text)
}

/// Description for a game that has no port.
pub fn game_without_port_description(
    game: &str,
    platform: &str,
    year: Option<i32>,
    wants: i64,
) -> String {
    let year = year.map(|y| format!(", {y}")).unwrap_or_default();
    let text = format!(
        "No native port of {game} ({platform}{year}) exists yet. {wants} people want one. See decompilation status or submit a port."
    );
    meta_description(&text)
}

/// Escape a string for XML text and attribute values.
pub fn xml_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// Escape a JSON-LD string for embedding in a `<script>` element: `<` becomes `\u003c`.
pub fn json_ld_escape(json: &str) -> String {
    json.replace('<', "\\u003c")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_description_is_unchanged() {
        assert_eq!(meta_description("Short text."), "Short text.");
    }

    #[test]
    fn long_description_is_cut_to_155_on_word_boundary() {
        let text = "word ".repeat(60);
        let out = meta_description(&text);
        assert!(
            out.chars().count() <= DESCRIPTION_MAX,
            "len {}",
            out.chars().count()
        );
        assert!(out.ends_with('…'));
        assert!(!out.trim_end_matches('…').ends_with(' '));
        assert!(
            out.trim_end_matches('…')
                .split(' ')
                .all(|w| w == "word" || w.is_empty())
        );
    }

    #[test]
    fn exactly_max_length_is_not_cut() {
        let text = "a".repeat(DESCRIPTION_MAX);
        assert_eq!(meta_description(&text), text);
    }

    #[test]
    fn json_ld_escapes_script_terminators() {
        assert_eq!(json_ld_escape("</script>"), "\\u003c/script>");
    }

    #[test]
    fn xml_escape_handles_specials() {
        assert_eq!(
            xml_escape("a&b<\"c\">'"),
            "a&amp;b&lt;&quot;c&quot;&gt;&apos;"
        );
    }

    #[test]
    fn game_with_ports_title_caps_targets_at_three() {
        let t = game_with_ports_title(
            "Majora's Mask",
            &["Windows", "macOS", "Linux", "Web browser"],
        );
        assert_eq!(
            t,
            "Majora's Mask Native Port & Recomp – Windows, macOS, Linux | RecompHub"
        );
    }

    #[test]
    fn port_description_uses_plural_correctly() {
        let d = game_with_ports_description("Zelda", "N64", Some(2000), &["A"], &["Windows"]);
        assert!(d.starts_with("Zelda (N64, 2000) has 1 native port: A."));
    }
}
