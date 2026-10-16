//! Human display names for ports whose name is still the repo slug.
//!
//! Order: the first README H1 that is a real title for this game, then a short clause of the repo
//! description, then the repo name when it is a project brand ("speedbreaker", "dusklight"), then
//! "{game} {kind}" ("Wave Race 64 Decompilation"). A candidate counts only if it names something
//! in the repo or game name, so a template heading copied from another project ("Zelda 64" on a
//! Mario Kart 64 repo) or a section heading ("Dependencies") is skipped. [`disambiguate`] then
//! makes names unique within a game.

use std::collections::HashMap;

const TITLE_MAX: usize = 80;
const DESCRIPTION_MAX: usize = 60;
/// A description clause longer than this reads as a sentence, not a name.
const DESCRIPTION_MAX_WORDS: usize = 6;
/// H1 headings looked at before giving up on the README.
const H1_LIMIT: usize = 4;
/// A repo name needs this many letters, outside port wording, to be used as a brand.
const BRAND_MIN_LETTERS: usize = 5;
/// Words that keep their canonical spelling in a prettified repo name.
const KEEP: &[&str] = &[
    "Recomp", "PC", "2D", "3D", "HD", "VR", "AI", "OS", "DOS", "N64", "PSX",
];

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
    "dependencies",
    "requirements",
    "contributing",
    "contribution guide",
    "contribution guidelines",
    "credits",
    "license",
    "faq",
    "features",
    "about",
    "downloads",
    "download",
    "changelog",
    "status",
    "notes",
    "table of contents",
    "decomp",
    "recomp",
    "decompilation",
    "recompilation",
];

/// Port and platform wording. A subtitle made only of these (plus filler) is cut when it carries
/// at least one platform or tooling word; a bare "Recompiled" names the project and stays.
const PLATFORM_WORDS: &[&str] = &[
    "pc",
    "psx",
    "ps1",
    "ps2",
    "n64",
    "snes",
    "gba",
    "nds",
    "mac",
    "macos",
    "linux",
    "windows",
    "web",
    "native",
    "port",
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
    "static",
    "statique",
    "matching",
    "rexglue",
    "n64recomp",
    "xenonrecomp",
    "xbe",
    "wip",
];
/// What the project is. Allowed in a cut subtitle, but never the reason to cut.
const KIND_WORDS: &[&str] = &[
    "recomp",
    "recompiled",
    "recompilation",
    "recompile",
    "decomp",
    "decompilation",
    "project",
];
/// Words that may sit between platform words in a suffix without making it part of the title.
const SUFFIX_FILLER: &[&str] = &[
    "unofficial",
    "for",
    "and",
    "on",
    "pi",
    "version",
    "build",
    "to",
    "360",
    "rust",
    "bevy",
];
/// Trailing words dropped from a finished name ("… WIP", "… (beta)").
const TRAILING_JUNK: &[&str] = &["wip", "beta", "alpha", "preview"];
/// Words too common to show that a heading is about this game.
const STOP_WORDS: &[&str] = &[
    "the", "of", "and", "for", "a", "an", "on", "in", "to", "with", "is", "game", "games",
    "edition",
];

/// Subtitle separators.
const SEPARATORS: [&str; 5] = [":", "|", "—", "–", " - "];

/// What a name is built from.
pub struct NameSources<'a> {
    pub readme: Option<&'a str>,
    pub description: Option<&'a str>,
    /// Repository name, without the owner.
    pub repo: &'a str,
    pub game: &'a str,
    /// Port kind slug (`static_recomp`, `decompilation`, …).
    pub kind: &'a str,
}

/// Name for a port. Always returns something.
pub fn derive_display_name(src: &NameSources) -> String {
    // A heading that is just the repo slug ("dmc-recomp", "Kirby-Dream-Land-Recomp") says
    // nothing the fallbacks don't; a cased one ("MikuPan", "Kameo: RePowered") is the brand.
    let relevant = |title: &str| {
        let single_plain_word =
            !title.contains([' ', '-', '_']) && !title.chars().skip(1).any(char::is_uppercase);
        let slug_like = !title.chars().any(char::is_uppercase)
            || single_plain_word
            || (!title.contains(' ') && title.matches(['-', '_']).count() >= 2);
        !(slug_like && normalize(title) == normalize(src.repo))
            && is_relevant(title, src.repo, src.game)
    };
    let name = src
        .readme
        .and_then(|r| readme_title(r, &relevant))
        .or_else(|| {
            src.description
                .and_then(|d| description_clause(d, src.game))
                .filter(|t| relevant(t))
        })
        .map(|t| strip_trailing_junk(&t))
        .filter(|t| t.chars().count() >= 3)
        .or_else(|| repo_brand(src.repo, src.game, src.kind))
        .unwrap_or_else(|| src.game.to_string());
    if normalize(&name) == normalize(src.game) {
        return format!("{} {}", src.game, kind_suffix(src.kind));
    }
    // "Need for Speed: High Stakes (PSX) matching decompilation": the game plus port wording
    // with an aside or console/tool noise collapses to "{game} {kind}". A plain tail
    // ("AeroGauge: Recompiled", "… PC") is the project's own name and stays.
    const TAIL_NOISE: &[&str] = &[
        "psx",
        "ps1",
        "ps2",
        "n64",
        "snes",
        "gba",
        "nds",
        "rexglue",
        "matching",
        "static",
        "statique",
        "xbe",
        "n64recomp",
        "xenonrecomp",
        "wip",
    ];
    if let Some(rest) = strip_game_prefix(&name, src.game) {
        let words: Vec<String> = rest
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_lowercase)
            .collect();
        let only_wording = words.iter().all(|w| {
            PLATFORM_WORDS.contains(&w.as_str())
                || KIND_WORDS.contains(&w.as_str())
                || SUFFIX_FILLER.contains(&w.as_str())
        });
        let noisy = rest.contains('(') || words.iter().any(|w| TAIL_NOISE.contains(&w.as_str()));
        if !words.is_empty() && only_wording && noisy {
            return format!("{} {}", src.game, kind_suffix(src.kind));
        }
    }
    name
}

/// What follows the game's name at the start of `name`, compared on letters and digits only.
fn strip_game_prefix<'a>(name: &'a str, game: &str) -> Option<&'a str> {
    let target = normalize(game);
    let mut seen = String::new();
    for (i, c) in name.char_indices() {
        if seen == target {
            return Some(&name[i..]);
        }
        if c.is_alphanumeric() {
            seen.extend(c.to_lowercase());
        }
        if !target.starts_with(&seen) {
            return None;
        }
    }
    (seen == target).then_some("")
}

/// The repo name, prettified, when it is a project brand rather than an abbreviation of the
/// game: "speedbreaker" -> "Speedbreaker", but not "tww", "skate3recomp" or "goldenrecomp".
/// Decompilation repos are nearly always named after the game, so they never count.
fn repo_brand(repo: &str, game: &str, kind: &str) -> Option<String> {
    if kind == "decompilation" {
        return None;
    }
    let base = tokens(repo);
    let letters: usize = base
        .iter()
        .map(|t| t.chars().filter(|c| c.is_alphabetic()).count())
        .sum();
    if letters < BRAND_MIN_LETTERS || base.iter().any(|t| names_the_game(t, game)) {
        return None;
    }
    Some(prettify(repo))
}

/// One port's name within its game, for [`disambiguate`].
pub struct Named {
    pub game_id: i64,
    pub owner: String,
    pub name: String,
    /// GitHub stars; the most-starred port keeps a shared name without the owner suffix.
    pub stars: i32,
}

/// When ports of the same game would share a name, the most-starred one keeps it and the others
/// get " (owner)" appended.
pub fn disambiguate(entries: &mut [Named]) {
    let mut leader: HashMap<(i64, String), (usize, i32, usize)> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        let slot = leader
            .entry((e.game_id, normalize(&e.name)))
            .or_insert((i, e.stars, 0));
        slot.2 += 1;
        if e.stars > slot.1 {
            (slot.0, slot.1) = (i, e.stars);
        }
    }
    for (i, e) in entries.iter_mut().enumerate() {
        let (lead, _, count) = leader[&(e.game_id, normalize(&e.name))];
        if count > 1 && lead != i {
            e.name = format!("{} ({})", e.name, e.owner);
        }
    }
}

/// "Recomp", "Decompilation", … appended when a name would equal the game's.
fn kind_suffix(kind: &str) -> &'static str {
    match kind {
        "static_recomp" => "Recomp",
        "decomp_port" => "Native Port",
        "source_port" => "Source Port",
        "reimplementation" => "Reimplementation",
        "fan_remake" => "Fan Remake",
        "native_layer" => "Native Layer",
        "decompilation" => "Decompilation",
        _ => "Port",
    }
}

fn readme_title(readme: &str, relevant: &dyn Fn(&str) -> bool) -> Option<String> {
    let mut in_code = false;
    let mut seen = 0;
    for line in readme.lines() {
        let line = line.trim();
        if line.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }
        let Some(heading) = line.strip_prefix("# ") else {
            continue;
        };
        seen += 1;
        if seen > H1_LIMIT {
            break;
        }
        let heading = strip_markup(heading);
        // "What is MikuPan?" names the project.
        let heading = heading
            .strip_prefix("What is ")
            .and_then(|rest| rest.strip_suffix('?'))
            .unwrap_or(&heading);
        if heading.ends_with('?') {
            continue;
        }
        if let Some(title) = clean_title(heading).filter(|t| relevant(t)) {
            return Some(title);
        }
    }
    None
}

fn description_clause(description: &str, game: &str) -> Option<String> {
    let sentence = description.split(". ").next()?.trim().trim_end_matches('.');
    if let Some((subject, kind)) = from_kind_phrase(sentence) {
        // "Decompilation of Alice in Wonderland (Wii 2010) by …" -> "Alice in Wonderland".
        let subject = cut_subject(&subject);
        let (s, g) = (normalize(subject), normalize(game));
        let subject = if !s.is_empty() && (g.contains(&s) || s.contains(&g)) {
            game
        } else {
            subject
        };
        return clean_title(&format!("{subject} {kind}"));
    }
    let clause = cut_subtitle(sentence).trim();
    let first = clause.split_whitespace().next()?;
    let starts_like_sentence = first.starts_with(char::is_lowercase)
        || ["a", "an", "the", "this"].contains(&first.to_lowercase().as_str());
    if starts_like_sentence
        || clause.chars().count() > DESCRIPTION_MAX
        || clause.split_whitespace().count() > DESCRIPTION_MAX_WORDS
    {
        return None;
    }
    clean_title(clause)
}

/// "A matching decompilation of Metroid Prime" -> ("Metroid Prime", "Decompilation").
fn from_kind_phrase(sentence: &str) -> Option<(String, &'static str)> {
    let words: Vec<&str> = sentence.split_whitespace().collect();
    let mut i = 0;
    let lower = |w: &str| w.to_lowercase();
    if words
        .get(i)
        .is_some_and(|w| ["a", "an", "the"].contains(&lower(w).as_str()))
    {
        i += 1;
    }
    if words
        .get(i)
        .is_some_and(|w| ["matching", "static"].contains(&lower(w).as_str()))
    {
        i += 1;
    }
    let kind = match words.get(i).map(|w| lower(w)).as_deref() {
        Some("decompilation") => "Decompilation",
        Some("recompilation") => "Recompilation",
        Some("reimplementation") => "Reimplementation",
        _ => return None,
    };
    i += 1;
    if words
        .get(i)
        .is_some_and(|w| ["effort", "project"].contains(&lower(w).as_str()))
    {
        i += 1;
    }
    if !words
        .get(i)
        .is_some_and(|w| ["of", "for"].contains(&lower(w).as_str()))
    {
        return None;
    }
    i += 1;
    if words.get(i).is_some_and(|w| lower(w) == "the") {
        i += 1;
    }
    let subject = words.get(i..)?.join(" ");
    (!subject.is_empty()).then_some((subject, kind))
}

/// Cuts a game subject at the first credit, platform or region aside.
fn cut_subject(subject: &str) -> &str {
    const ASIDES: [&str; 8] = [
        " by ",
        " brought ",
        " using ",
        " with ",
        " for the ",
        " for ",
        " (",
        ",",
    ];
    let lower = subject.to_lowercase();
    let end = ASIDES
        .iter()
        .filter_map(|a| lower.find(a))
        .min()
        .unwrap_or(subject.len());
    subject[..end].trim()
}

/// Keeps the part before a platform subtitle and rejects empty, generic, overlong or notice-like
/// results. A head that is an abbreviation ("TLoZ:TWW HD Recompiled — Android", "SM64: …")
/// belongs to the title, so the cut is applied to what follows it instead. Short real titles
/// ("Doom: Native PC Port") are not abbreviations.
fn clean_title(raw: &str) -> Option<String> {
    let raw = strip_slug_prefix(raw);
    let trim = |s: &str| {
        s.trim()
            .trim_end_matches(['-', '|', ','])
            .trim()
            .to_string()
    };
    let head_raw = cut_subtitle(raw);
    let head = trim(head_raw);
    let title = if is_abbreviation(&head) && head_raw.len() < raw.len() {
        let after_head = &raw[head_raw.len()..];
        let rest_start = SEPARATORS
            .iter()
            .find_map(|sep| after_head.strip_prefix(sep))
            .map_or(head_raw.len(), |rest| raw.len() - rest.len());
        let tail = cut_subtitle(&raw[rest_start..]);
        trim(&raw[..rest_start + tail.len()])
    } else {
        head
    };
    let len = title.chars().count();
    let lower = title.to_lowercase();
    let letters = title.chars().filter(|c| c.is_alphabetic()).count();
    let upper = title.chars().filter(|c| c.is_uppercase()).count();
    // Notices and banners ("NOTE: This is outdated!", "SUPPORT THE OFFICIAL RELEASE…") aren't names.
    let shouting = letters >= 12 && upper * 10 >= letters * 7;
    let notice = lower.contains("http") || lower.starts_with("note") || title.ends_with('!');
    if !(3..=TITLE_MAX).contains(&len) || GENERIC.contains(&lower.as_str()) || shouting || notice {
        return None;
    }
    Some(title)
}

/// "rb2 - Dragon Ball Raging Blast 2" -> "Dragon Ball Raging Blast 2": drops a leading
/// lowercase repo-style word (with a digit, hyphen or underscore) followed by a separator.
fn strip_slug_prefix(raw: &str) -> &str {
    for sep in [" - ", " — ", " – ", ": "] {
        if let Some((head, rest)) = raw.split_once(sep) {
            let slug_like = !head.is_empty()
                && !head.contains(' ')
                && head
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
                && head
                    .chars()
                    .any(|c| c.is_ascii_digit() || c == '-' || c == '_');
            if slug_like && !rest.trim().is_empty() {
                return rest.trim();
            }
        }
    }
    raw
}

/// Cuts at the first separator whose following segment (up to the next separator) is port
/// wording with at least one platform or tooling word. "Need for Speed: Most Wanted" and
/// "Zelda 64: Recompiled" stay whole; "Game PC: Mac/Linux Native" and "… - XBE Recomp" are cut.
fn cut_subtitle(s: &str) -> &str {
    let mut cuts: Vec<(usize, usize)> = SEPARATORS
        .iter()
        .flat_map(|sep| s.match_indices(sep).map(move |(at, _)| (at, sep.len())))
        .collect();
    cuts.sort_unstable();
    for (i, &(at, len)) in cuts.iter().enumerate() {
        let end = cuts.get(i + 1).map_or(s.len(), |next| next.0);
        let words: Vec<String> = s[at + len..end]
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_lowercase)
            .collect();
        let only_port_wording = words.iter().all(|w| {
            PLATFORM_WORDS.contains(&w.as_str())
                || KIND_WORDS.contains(&w.as_str())
                || SUFFIX_FILLER.contains(&w.as_str())
                || w.chars().all(|c| c.is_ascii_digit())
        });
        let has_platform = words.iter().any(|w| PLATFORM_WORDS.contains(&w.as_str()));
        if !words.is_empty() && only_port_wording && has_platform {
            return &s[..at];
        }
    }
    s
}

/// Drops trailing version numbers ("v1.0.3"), "WIP"/"beta", and a final parenthetical made only
/// of port wording ("(Recompilation)", "(beta)").
fn strip_trailing_junk(name: &str) -> String {
    // "Lost Odyssey Recomp using XenonRecomp": the toolchain isn't part of the name.
    let name = match name.to_lowercase().find(" using ") {
        Some(at) if at > 0 => &name[..at],
        _ => name,
    };
    let mut out = name.trim().to_string();
    loop {
        let before = out.clone();
        if out.ends_with(')')
            && let Some(open) = out.rfind('(')
        {
            let inner = out[open + 1..out.len() - 1].to_lowercase();
            let junk = inner.split_whitespace().all(|w| {
                TRAILING_JUNK.contains(&w) || KIND_WORDS.contains(&w) || PLATFORM_WORDS.contains(&w)
            });
            if junk && !inner.trim().is_empty() {
                out.truncate(open);
            }
        }
        let trimmed = out.trim_end();
        if let Some((rest, last)) = trimmed.rsplit_once(' ') {
            let l = last.to_lowercase();
            let version = l.strip_prefix('v').unwrap_or(&l);
            let is_version = !version.is_empty()
                && version.chars().all(|c| c.is_ascii_digit() || c == '.')
                && version.contains('.');
            if TRAILING_JUNK.contains(&l.as_str()) || is_version {
                out = rest.to_string();
            }
        }
        out = out
            .trim()
            .trim_end_matches(['-', '—', '–', ',', ':'])
            .trim()
            .to_string();
        if out == before {
            return out;
        }
    }
}

/// Whether `title` names something in the repo or game name.
fn is_relevant(title: &str, repo: &str, game: &str) -> bool {
    let repo_flat = normalize(repo);
    let repo_tokens = tokens(repo);
    tokens(title).iter().any(|t| {
        names_the_game(t, game)
            || repo_tokens.contains(t)
            || (t.chars().count() >= 3 && repo_flat.contains(t.as_str()))
    })
}

/// Whether a single token stands for the game: one of its words, the start of one ("golden" for
/// "GoldenEye"), a run of its initials ("sot" for "Sands of Time"), or part of its name.
fn names_the_game(token: &str, game: &str) -> bool {
    let game_tokens = tokens(game);
    let initials: String = game
        .split(|c: char| !c.is_alphanumeric())
        .filter_map(|w| w.chars().next())
        .flat_map(char::to_lowercase)
        .collect();
    let long = token.chars().count() >= 3;
    game_tokens
        .iter()
        .any(|g| g == token || (long && g.starts_with(token)))
        || (long && initials.contains(token))
        || (token.chars().count() >= 4 && normalize(game).contains(token))
}

/// Kind words that are often glued to the end of a repo name ("dimensionsrecomp").
const GLUED_SUFFIXES: [&str; 6] = [
    "recompilation",
    "recompiled",
    "recomp",
    "decompilation",
    "decomp",
    "port",
];

/// "dimensionsrecomp" -> ("dimensions", Some("recomp")). The base must keep 3+ characters.
fn split_glued(word: &str) -> (&str, Option<&'static str>) {
    let lower = word.to_lowercase();
    for suffix in GLUED_SUFFIXES {
        if lower.len() >= suffix.len() + 2
            && lower.ends_with(suffix)
            && word.is_char_boundary(word.len() - suffix.len())
        {
            return (&word[..word.len() - suffix.len()], Some(suffix));
        }
    }
    (word, None)
}

/// "speedbreaker" -> "Speedbreaker"; "silly-reencounter" -> "Silly Reencounter";
/// "mirror-of-alessa" -> "Mirror of Alessa"; "sacredfirerecomp" -> "Sacredfire Recomp".
fn prettify(repo: &str) -> String {
    const SMALL: [&str; 7] = ["of", "the", "and", "a", "in", "on", "for"];
    let mut words: Vec<String> = Vec::new();
    for part in repo.split(['-', '_', '.']).filter(|w| !w.is_empty()) {
        let (base, suffix) = split_glued(part);
        words.push(base.to_string());
        if let Some(s) = suffix {
            words.push(s.to_string());
        }
    }
    words
        .iter()
        .enumerate()
        .map(|(i, w)| {
            if let Some(known) = KEEP.iter().find(|k| k.eq_ignore_ascii_case(w)) {
                return (*known).to_string();
            }
            if i > 0 && SMALL.contains(&w.to_lowercase().as_str()) {
                return w.to_lowercase();
            }
            let mut chars = w.chars();
            chars
                .next()
                .map(|f| f.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Significant lowercase words, split at punctuation and at letter/digit and camel-case
/// boundaries: "DK64Recomp" -> ["dk", "recomp"] minus port words -> ["dk"].
fn tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev: Option<char> = None;
    for c in s.chars() {
        let boundary = match prev {
            Some(p) => {
                !c.is_alphanumeric()
                    || (p.is_alphabetic() != c.is_alphabetic() && p.is_alphanumeric())
                    || (p.is_lowercase() && c.is_uppercase())
            }
            None => !c.is_alphanumeric(),
        };
        if boundary && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        if c.is_alphanumeric() {
            cur.extend(c.to_lowercase());
        }
        prev = Some(c);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    let out: Vec<String> = out
        .into_iter()
        .flat_map(|t| {
            let (base, suffix) = split_glued(&t);
            std::iter::once(base.to_string()).chain(suffix.map(str::to_string))
        })
        .collect();
    let mut out = out;
    out.retain(|t| {
        t.chars().count() >= 2
            && !t.chars().all(|c| c.is_ascii_digit())
            && !STOP_WORDS.contains(&t.as_str())
            && !PLATFORM_WORDS.contains(&t.as_str())
            && !KIND_WORDS.contains(&t.as_str())
    });
    out
}

/// Lowercase letters and digits only, for equality checks.
fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
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
            // Em and en dashes stay: they are subtitle separators.
            '—' | '–' => out.push(c),
            c if (c as u32) >= 0x2000 && !c.is_alphanumeric() => {}
            c => out.push(c),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// "TLoZ", "SM64", "OoT", "MGS": one word, at least two capitals, and capitals make up at
/// least half of its letters.
fn is_abbreviation(word: &str) -> bool {
    if word.contains(char::is_whitespace) {
        return false;
    }
    let letters = word.chars().filter(|c| c.is_alphabetic()).count();
    let upper = word.chars().filter(|c| c.is_uppercase()).count();
    upper >= 2 && upper * 2 >= letters
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(readme: Option<&str>, description: Option<&str>, repo: &str, game: &str) -> String {
        derive_display_name(&NameSources {
            readme,
            description,
            repo,
            game,
            kind: "static_recomp",
        })
    }

    #[test]
    fn harry_potter_readme_h1_becomes_the_name() {
        let readme = "---\n\n# Harry Potter and the Chamber of Secrets PC: Mac/Linux/Windows/Web Native\n\nPlay the 2002 game.\n";
        assert_eq!(
            name(
                Some(readme),
                Some("Play the 2002 game with a modern launcher."),
                "harry-potter-2-chamber-of-secrets-pc",
                "Harry Potter and the Chamber of Secrets",
            ),
            "Harry Potter and the Chamber of Secrets PC"
        );
    }

    #[test]
    fn colon_titles_and_bare_kind_subtitles_are_kept_whole() {
        let game = "The Legend of Zelda: Majora's Mask";
        assert_eq!(
            name(
                Some("# The Legend of Zelda: Majora's Mask - PC Port"),
                None,
                "zelda-mm-pc",
                game
            ),
            "The Legend of Zelda: Majora's Mask Recomp"
        );
        assert_eq!(
            name(Some("# Zelda 64: Recompiled"), None, "Zelda64Recomp", game),
            "Zelda 64: Recompiled"
        );
    }

    #[test]
    fn platform_or_tooling_subtitles_are_cut() {
        assert_eq!(
            name(
                Some("# Splinter Cell: Double Agent - XBE Recomp"),
                None,
                "scda-recomp",
                "Tom Clancy's Splinter Cell: Double Agent",
            ),
            "Splinter Cell: Double Agent"
        );
    }

    #[test]
    fn abbreviation_head_and_fork_suffix_are_kept() {
        let game = "The Legend of Zelda: The Wind Waker HD";
        assert_eq!(
            name(
                Some("# TLoZ:TWW HD Recompiled — Android"),
                None,
                "zeldawwhdrecompandroid",
                game
            ),
            "TLoZ:TWW HD Recompiled"
        );
        assert_eq!(
            name(
                Some("# Wind Waker HD Recompiled — Android Enhanced"),
                None,
                "zeldawwhdrecompandroid---enhanced",
                game,
            ),
            "Wind Waker HD Recompiled — Android Enhanced"
        );
        assert_eq!(
            name(Some("# Doom: Native PC Port"), None, "doom-pc", "Doom 64"),
            "Doom"
        );
    }

    #[test]
    fn heading_from_another_project_is_skipped() {
        assert_eq!(
            name(
                Some("# Zelda 64"),
                None,
                "mariokart64recomp",
                "Mario Kart 64"
            ),
            "Mario Kart 64 Recomp"
        );
    }

    #[test]
    fn section_headings_and_questions() {
        assert_eq!(
            name(
                Some("# Dependencies\n# Building"),
                None,
                "sfa-decomp",
                "Star Fox Adventures"
            ),
            "Star Fox Adventures Recomp"
        );
        assert_eq!(
            name(Some("# What is MikuPan?"), None, "mikupan", "Fatal Frame"),
            "MikuPan"
        );
    }

    #[test]
    fn kind_phrases_in_descriptions_become_names() {
        assert_eq!(
            name(
                None,
                Some("A decompilation effort for Super Mario Galaxy 2"),
                "garigari",
                "Super Mario Galaxy 2",
            ),
            "Super Mario Galaxy 2 Decompilation"
        );
        assert_eq!(
            name(
                None,
                Some("Artifact mirror for the Memory of Alessa project"),
                "mirror-of-alessa",
                "Silent Hill 2",
            ),
            "Mirror of Alessa"
        );
    }

    #[test]
    fn trailing_versions_wip_and_kind_parentheticals_are_dropped() {
        assert_eq!(
            name(
                Some("# MOTHER 3 Native Port (Recompilation) v1.0.3"),
                None,
                "mother3-recomp",
                "Mother 3",
            ),
            "MOTHER 3 Native Port"
        );
        assert_eq!(
            name(
                Some("# Castlevania: Legacy of Darkness recompilation WIP"),
                None,
                "cvlod_recomp",
                "Castlevania: Legacy of Darkness",
            ),
            "Castlevania: Legacy of Darkness recompilation"
        );
    }

    #[test]
    fn duplicate_names_within_a_game_get_the_owner_except_the_most_starred() {
        let entry = |game_id, owner: &str, stars| Named {
            game_id,
            owner: owner.into(),
            name: "Banjo: Recompiled".into(),
            stars,
        };
        let mut entries = vec![entry(1, "a", 10), entry(1, "b", 900), entry(2, "c", 0)];
        disambiguate(&mut entries);
        assert_eq!(entries[0].name, "Banjo: Recompiled (a)");
        assert_eq!(entries[1].name, "Banjo: Recompiled");
        assert_eq!(entries[2].name, "Banjo: Recompiled");
    }

    #[test]
    fn heading_inside_code_block_is_ignored() {
        let readme = "```sh\n# install deps\n```\n# Real Title\n";
        assert_eq!(
            name(Some(readme), None, "real-title", "Real Title Game"),
            "Real Title"
        );
    }
}
