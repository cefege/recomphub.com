//! Finds YouTube video IDs in README markdown or HTML. Pure; no network.

use regex::Regex;
use std::sync::LazyLock;

/// Watch, short, embed, and thumbnail links. These are trusted as gameplay.
static PRIMARY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?:youtube(?:-nocookie)?\.com/watch\?(?:[^\s"'<>)\]]*?&)?v=|youtu\.be/|youtube(?:-nocookie)?\.com/(?:embed|v)/|(?:img\.youtube\.com|i\.ytimg\.com)/vi(?:_webp)?/)([A-Za-z0-9_-]{11})"#,
    )
    .expect("static regex")
});

/// Shorts links. Used only when the text has no primary link.
static SHORTS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"youtube(?:-nocookie)?\.com/shorts/([A-Za-z0-9_-]{11})").expect("static regex")
});

fn is_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// True when `s` has the shape of a YouTube video ID: exactly eleven URL-safe characters.
pub fn is_youtube_id(s: &str) -> bool {
    s.len() == 11 && s.chars().all(is_id_char)
}

fn collect(re: &Regex, text: &str, out: &mut Vec<(usize, String)>) {
    for caps in re.captures_iter(text) {
        let id = caps.get(1).expect("group 1 is mandatory");
        // An 11-character match inside a longer run is not a video ID.
        if text[id.end()..].chars().next().is_some_and(is_id_char) {
            continue;
        }
        out.push((id.start(), id.as_str().to_string()));
    }
}

fn in_order(mut hits: Vec<(usize, String)>) -> Vec<String> {
    hits.sort_by_key(|(pos, _)| *pos);
    let mut ids: Vec<String> = Vec::new();
    for (_, id) in hits {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

/// YouTube video IDs linked from `text`, in order of first appearance and without duplicates.
/// `/shorts/` links are returned only when the text has no other YouTube video link.
pub fn youtube_ids_in_markdown(text: &str) -> Vec<String> {
    let mut primary = Vec::new();
    collect(&PRIMARY_RE, text, &mut primary);
    if !primary.is_empty() {
        return in_order(primary);
    }
    let mut shorts = Vec::new();
    collect(&SHORTS_RE, text, &mut shorts);
    in_order(shorts)
}

#[cfg(test)]
mod tests {
    use super::youtube_ids_in_markdown;

    #[test]
    fn watch_short_embed_and_thumbnail_links_are_found() {
        let text = "Watch [here](https://www.youtube.com/watch?v=AAAAAAAAAAA) \
                    or https://youtu.be/BBBBBBBBBBB and \
                    <iframe src=\"https://www.youtube-nocookie.com/embed/CCCCCCCCCCC\"></iframe> \
                    [![x](https://img.youtube.com/vi/DDDDDDDDDDD/hqdefault.jpg)](https://youtu.be/DDDDDDDDDDD)";
        assert_eq!(
            youtube_ids_in_markdown(text),
            vec!["AAAAAAAAAAA", "BBBBBBBBBBB", "CCCCCCCCCCC", "DDDDDDDDDDD"]
        );
    }

    #[test]
    fn watch_links_with_other_query_parameters_are_found() {
        assert_eq!(
            youtube_ids_in_markdown("https://m.youtube.com/watch?list=PL123&v=EEEEEEEEEEE&t=10s"),
            vec!["EEEEEEEEEEE"]
        );
        assert_eq!(
            youtube_ids_in_markdown("https://www.youtube.com/watch?feature=share&v=FFFFFFFFFFF"),
            vec!["FFFFFFFFFFF"]
        );
    }

    #[test]
    fn duplicates_are_dropped_and_first_appearance_wins() {
        let text = "https://youtu.be/GGGGGGGGGGG then https://youtu.be/HHHHHHHHHHH then \
                    https://www.youtube.com/watch?v=GGGGGGGGGGG";
        assert_eq!(
            youtube_ids_in_markdown(text),
            vec!["GGGGGGGGGGG", "HHHHHHHHHHH"]
        );
    }

    #[test]
    fn shorts_are_a_fallback_only() {
        assert_eq!(
            youtube_ids_in_markdown("https://www.youtube.com/shorts/IIIIIIIIIII"),
            vec!["IIIIIIIIIII"]
        );
        let mixed = "https://www.youtube.com/shorts/IIIIIIIIIII \
                     https://youtu.be/JJJJJJJJJJJ";
        assert_eq!(youtube_ids_in_markdown(mixed), vec!["JJJJJJJJJJJ"]);
    }

    #[test]
    fn ids_must_be_exactly_eleven_characters() {
        assert!(youtube_ids_in_markdown("https://youtu.be/KKKKKKKKKKKK").is_empty());
        assert!(youtube_ids_in_markdown("https://youtu.be/KKKKKKKKK").is_empty());
    }

    #[test]
    fn channel_and_playlist_links_are_not_videos() {
        let text = "https://www.youtube.com/@SomeChannel \
                    https://www.youtube.com/playlist?list=PLxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
        assert!(youtube_ids_in_markdown(text).is_empty());
    }

    #[test]
    fn plain_text_without_links_yields_nothing() {
        assert!(youtube_ids_in_markdown("A port of the game. Check youtube for more.").is_empty());
        assert!(youtube_ids_in_markdown("").is_empty());
    }
}
