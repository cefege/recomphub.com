/// Lowercase URL slug: Latin diacritics fold to ASCII (`é` → `e`, `ß` → `ss`), `&` becomes
/// `and`, every other non-alphanumeric run becomes `-`, and dashes are trimmed.
pub fn slugify(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut pending_dash = false;
    for ch in input.chars() {
        let folded = ch.to_lowercase().next().and_then(fold_latin);
        if ch == '&' {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push_str("and");
        } else if let Some(ascii) = folded {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push_str(ascii);
        } else if ch.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(ch.to_ascii_lowercase());
        } else {
            pending_dash = true;
        }
    }
    out
}

/// Matching key for titles. Lowercases, folds Latin diacritics to ASCII, maps `&` to `and`,
/// turns non-alphanumerics into spaces, collapses whitespace and strips a leading `the `.
pub fn normalize_title(input: &str) -> String {
    let mut spaced = String::with_capacity(input.len());
    for ch in input.chars() {
        if ch == '&' {
            spaced.push_str(" and ");
        } else if ch.is_alphanumeric() {
            for lower in ch.to_lowercase() {
                match fold_latin(lower) {
                    Some(ascii) => spaced.push_str(ascii),
                    None => spaced.push(lower),
                }
            }
        } else {
            spaced.push(' ');
        }
    }
    let collapsed = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    match collapsed.strip_prefix("the ") {
        Some(rest) => rest.to_string(),
        None => collapsed,
    }
}

/// ASCII replacement for an already-lowercased Latin letter with a diacritic.
fn fold_latin(ch: char) -> Option<&'static str> {
    Some(match ch {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'č' => "c",
        'ď' | 'đ' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => "e",
        'ğ' => "g",
        'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' | 'ı' => "i",
        'ł' | 'ľ' => "l",
        'ñ' | 'ń' | 'ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => "o",
        'œ' => "oe",
        'ř' => "r",
        'ś' | 'š' | 'ş' => "s",
        'ß' => "ss",
        'ť' | 'ţ' => "t",
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' | 'ų' => "u",
        'ý' | 'ÿ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basics() {
        assert_eq!(
            slugify("The Legend of Zelda: Majora's Mask"),
            "the-legend-of-zelda-majora-s-mask"
        );
        assert_eq!(slugify("Sonic & Knuckles"), "sonic-and-knuckles");
        assert_eq!(slugify("  --Hello   World--  "), "hello-world");
        assert_eq!(slugify("Pokémon"), "pokemon");
        assert_eq!(slugify("Ōkami HD"), "okami-hd");
    }

    #[test]
    fn normalize_title_folds_diacritics() {
        assert_eq!(normalize_title("Pokémon Snap"), "pokemon snap");
        assert_eq!(normalize_title("Pokemon Snap"), "pokemon snap");
    }

    #[test]
    fn normalize_title_matches_spec_example() {
        assert_eq!(
            normalize_title("The Legend of Zelda: Majora's Mask"),
            "legend of zelda majora s mask"
        );
    }

    #[test]
    fn normalize_title_maps_ampersand_and_strips_article_once() {
        assert_eq!(normalize_title("Sonic & Knuckles"), "sonic and knuckles");
        assert_eq!(normalize_title("The The Division"), "the division");
    }

    #[test]
    fn normalize_title_treats_punctuation_as_space() {
        assert_eq!(
            normalize_title("Ace Combat 6: Fires of Liberation"),
            "ace combat 6 fires of liberation"
        );
        assert_eq!(normalize_title("Mario-Kart"), "mario kart");
    }
}
