//! Questions and answers for a game page. The same list renders on the page and as `FAQPage`
//! JSON-LD, so the visible text and the structured data never drift apart.

use recomphub_shared::{GameDetail, PortCard};

use crate::components::listing::PRIMARY_TARGETS;
use crate::components::ui::{target_label, version_label};

#[derive(Debug, Clone, PartialEq)]
pub struct Faq {
    pub question: String,
    pub answer: String,
}

/// FAQ for a game: one "Can I play … on …?" per primary target, then ownership, which port to
/// pick, and decompilation status where they apply.
pub fn game_faq(detail: &GameDetail) -> Vec<Faq> {
    let game = &detail.game.name;
    let origin = origin(detail);
    let ports = ranked(&detail.ports);
    let mut out: Vec<Faq> = PRIMARY_TARGETS
        .iter()
        .map(|t| target_faq(detail, &ports, t))
        .collect();

    if !ports.is_empty() {
        out.push(Faq {
            question: format!("Do I need to own {game} to play these ports?"),
            answer: format!(
                "Yes. Recompilations and native ports rebuild the game's code, not its art, audio or levels. Each project reads those from your own copy of {game}{origin}. RecompHub's ground rules exclude any project that ships game data, ROMs, firmware or keys."
            ),
        });
    }

    if ports.len() > 1 {
        let best = ports[0];
        let others: Vec<String> = ports[1..].iter().map(|p| short(p)).collect();
        out.push(Faq {
            question: format!("Which {game} port should I use?"),
            answer: format!(
                "Start with {}: {}, and it runs on {}. {} Other options: {}.",
                best.name,
                status(best),
                targets_text(best),
                stars_sentence(best),
                join_and(&others),
            )
            .replace("  ", " "),
        });
    }

    if !detail.decompilations.is_empty() {
        let names: Vec<String> = detail
            .decompilations
            .iter()
            .map(|p| p.name.clone())
            .collect();
        out.push(Faq {
            question: format!("Is there a decompilation of {game}?"),
            answer: format!(
                "Yes: {}. A decompilation turns the original machine code back into readable source code. It is the groundwork most native ports are built on, but on its own it is not something you play.",
                join_and(&names)
            ),
        });
    }
    out
}

fn target_faq(detail: &GameDetail, ports: &[&PortCard], target: &str) -> Faq {
    let game = &detail.game.name;
    let label = target_label(target);
    let place = place(target, &label);
    let question = format!("Can I play {game} {place}?");
    let via: Vec<&PortCard> = ports
        .iter()
        .copied()
        .filter(|p| p.targets.iter().any(|t| t == target))
        .collect();

    let answer = match via.as_slice() {
        [] if ports.is_empty() => {
            let wants = match detail.game.want_count {
                0 => String::new(),
                1 => " One person has asked for one.".to_string(),
                n => format!(" {n} people have asked for one."),
            };
            format!(
                "Not yet. There is no known recompilation or native port of {game}{} for {label} or any other platform.{wants} If a project exists that we have missed, you can submit it.",
                origin(detail)
            )
        }
        [] => {
            let elsewhere = covered_targets(ports);
            let layer = match target {
                "macos" if elsewhere.iter().any(|t| t == "Windows") => {
                    " The Windows build may run through a compatibility layer such as CrossOver, but that is not a native port."
                }
                "linux" if elsewhere.iter().any(|t| t == "Windows") => {
                    " The Windows build may run through Wine or Proton, but that is not a native port."
                }
                _ => "",
            };
            let count = if ports.len() == 1 {
                "the one known port".to_string()
            } else {
                format!("the {} known ports", ports.len())
            };
            format!(
                "Not natively yet. None of {count} of {game} has a {label} build; they cover {}.{layer} If you know of a {label} version, you can submit it.",
                join_and(&elsewhere)
            )
        }
        [one] => format!(
            "Yes. {game} runs natively {place} through {}, {} ({}). {}{}",
            one.name,
            article_kind(one),
            status(one),
            release_sentence(one),
            also_runs(one, target),
        ),
        many => {
            let list: Vec<String> = many.iter().map(|p| short(p)).collect();
            format!(
                "Yes. {} projects run {game} natively {place}: {}. If you are not sure which to pick, start with {}, the most complete and most popular of them ({}).",
                many.len(),
                join_and(&list),
                many[0].name,
                status(many[0]),
            )
        }
    };
    Faq { question, answer }
}

/// Ports ordered best first: playability, then GitHub stars, then name.
fn ranked(ports: &[PortCard]) -> Vec<&PortCard> {
    let mut out: Vec<&PortCard> = ports.iter().collect();
    out.sort_by(|a, b| {
        playability_rank(&b.playability)
            .cmp(&playability_rank(&a.playability))
            .then(b.github_stars.cmp(&a.github_stars))
            .then(a.name.cmp(&b.name))
    });
    out
}

fn playability_rank(p: &str) -> u8 {
    match p {
        "complete" => 3,
        "playable" => 2,
        "in_progress" => 1,
        _ => 0,
    }
}

fn playability_text(p: &str) -> &'static str {
    match p {
        "complete" => "fully playable",
        "playable" => "playable",
        "in_progress" => "still in progress",
        _ => "playability not yet rated",
    }
}

/// `" (PlayStation 2, 2005)"`, or empty when neither is known.
fn origin(detail: &GameDetail) -> String {
    let platform = detail.platforms.first().map(|p| p.name.clone());
    match (platform, detail.game.release_year) {
        (Some(p), Some(y)) => format!(" ({p}, {y})"),
        (Some(p), None) => format!(" ({p})"),
        (None, Some(y)) => format!(" ({y})"),
        (None, None) => String::new(),
    }
}

/// "on Windows", or "in a web browser" for the web target.
fn place(target: &str, label: &str) -> String {
    if target == "web" {
        "in a web browser".to_string()
    } else {
        format!("on {label}")
    }
}

/// "a static recompilation", "an unofficial port".
fn article_kind(p: &PortCard) -> String {
    let kind = p.kind_label.to_lowercase();
    let article = match kind.chars().next() {
        Some('a' | 'e' | 'i' | 'o' | 'u') => "an",
        _ => "a",
    };
    format!("{article} {kind}")
}

/// "fully playable, latest release v1.2", or the release state when there is no version.
fn status(p: &PortCard) -> String {
    let play = playability_text(&p.playability);
    match p.latest_version.as_deref().map(version_label) {
        Some(v) if v.starts_with('v') => format!("{play}, latest release {v}"),
        Some(v) => format!("{play}, builds tagged \"{v}\""),
        None => format!("{play}, {}", p.release_state_label.to_lowercase()),
    }
}

/// "NFSMW-Recompiled (static recompilation, playable)".
fn short(p: &PortCard) -> String {
    format!(
        "{} ({}, {})",
        p.name,
        p.kind_label.to_lowercase(),
        playability_text(&p.playability)
    )
}

fn release_sentence(p: &PortCard) -> String {
    match &p.latest_version {
        Some(_) => "Builds are published on the project's release page.".to_string(),
        None => {
            "There is no tagged release yet, so you may need to build it from source.".to_string()
        }
    }
}

/// `" It also runs on Linux and Android."` for the port's other targets, or empty.
fn also_runs(p: &PortCard, target: &str) -> String {
    let others: Vec<String> = p
        .targets
        .iter()
        .filter(|t| t.as_str() != target)
        .map(|t| target_label(t))
        .collect();
    if others.is_empty() {
        String::new()
    } else {
        format!(" It also runs on {}.", join_and(&others))
    }
}

fn stars_sentence(p: &PortCard) -> String {
    match p.github_stars {
        0 => String::new(),
        1 => "It has 1 GitHub star.".to_string(),
        n => format!("It has {n} GitHub stars."),
    }
}

fn targets_text(p: &PortCard) -> String {
    let labels: Vec<String> = p.targets.iter().map(|t| target_label(t)).collect();
    if labels.is_empty() {
        "no listed platforms yet".to_string()
    } else {
        join_and(&labels)
    }
}

/// Labels of every target any port covers, primary targets first, without duplicates.
fn covered_targets(ports: &[&PortCard]) -> Vec<String> {
    let mut slugs: Vec<&str> = Vec::new();
    for p in ports {
        for t in &p.targets {
            if !slugs.contains(&t.as_str()) {
                slugs.push(t);
            }
        }
    }
    slugs.sort_by_key(|s| {
        PRIMARY_TARGETS
            .iter()
            .position(|p| p == s)
            .unwrap_or(usize::MAX)
    });
    slugs.into_iter().map(target_label).collect()
}

/// "a", "a and b", "a, b and c".
fn join_and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use recomphub_shared::{GameCard, PlatformRef};

    fn port(name: &str, targets: &[&str], playability: &str, stars: i32) -> PortCard {
        PortCard {
            id: 1,
            slug: name.to_lowercase(),
            game_id: 1,
            game_slug: "nfsmw".into(),
            game_name: "Need for Speed: Most Wanted".into(),
            name: name.into(),
            kind: "recomp".into(),
            kind_label: "Static recompilation".into(),
            targets: targets.iter().map(|t| t.to_string()).collect(),
            authors: vec![],
            ai_usage: "none".into(),
            ai_tools: vec![],
            ai_evidence: vec![],
            playability: playability.into(),
            latest_version: Some("0.3".into()),
            latest_release_url: None,
            latest_release_at: None,
            release_state: "released".into(),
            release_state_label: "Released".into(),
            github_stars: stars,
            star_count: 0,
            repo_url: String::new(),
            cover: None,
            maintenance: None,
        }
    }

    fn detail(ports: Vec<PortCard>, want_count: i32) -> GameDetail {
        GameDetail {
            game: GameCard {
                id: 1,
                slug: "nfsmw".into(),
                name: "Need for Speed: Most Wanted".into(),
                release_year: Some(2005),
                cover: None,
                port_count: ports.len() as i32,
                want_count,
                targets: vec![],
                best_playability: None,
                popularity: 0,
            },
            summary: None,
            alt_names: vec![],
            developer: None,
            publisher: None,
            genres: vec![],
            first_release_date: None,
            platforms: vec![PlatformRef {
                id: 1,
                slug: Some("ps2".into()),
                name: "PlayStation 2".into(),
                abbreviation: None,
                family: None,
                game_count: 0,
                ported_count: 0,
            }],
            screenshots: vec![],
            og: None,
            ports,
            decompilations: vec![],
            videos: vec![],
            related: vec![],
            total_github_stars: 0,
            live_viewers: 0,
        }
    }

    fn answer<'a>(faq: &'a [Faq], question_part: &str) -> &'a str {
        &faq.iter()
            .find(|f| f.question.contains(question_part))
            .unwrap()
            .answer
    }

    #[test]
    fn single_port_answer_names_kind_status_and_other_targets() {
        let faq = game_faq(&detail(
            vec![port("speedbreaker", &["macos", "linux"], "playable", 40)],
            0,
        ));
        let a = answer(&faq, "on macOS");
        assert!(a.starts_with("Yes. Need for Speed: Most Wanted runs natively on macOS through speedbreaker, a static recompilation (playable, latest release v0.3)."), "{a}");
        assert!(a.ends_with("It also runs on Linux."), "{a}");
        assert!(answer(&faq, "Do I need to own").contains("your own copy"));
    }

    #[test]
    fn several_ports_recommend_the_most_complete_first() {
        let faq = game_faq(&detail(
            vec![
                port("NFSMW-Recompiled", &["windows"], "playable", 500),
                port("speedbreaker", &["windows", "macos"], "complete", 40),
            ],
            0,
        ));
        let a = answer(&faq, "on Windows");
        assert!(a.starts_with("Yes. 2 projects run"), "{a}");
        assert!(a.contains("start with speedbreaker"), "{a}");
        assert!(answer(&faq, "Which").starts_with("Start with speedbreaker"));
    }

    #[test]
    fn missing_target_lists_covered_platforms_and_compatibility_layer() {
        let faq = game_faq(&detail(
            vec![port("NFSMW-Recompiled", &["windows"], "playable", 5)],
            0,
        ));
        let linux = answer(&faq, "on Linux");
        assert!(
            linux.starts_with("Not natively yet. None of the one known port"),
            "{linux}"
        );
        assert!(linux.contains("they cover Windows"), "{linux}");
        assert!(linux.contains("Wine or Proton"), "{linux}");
        let web = answer(&faq, "in a web browser");
        assert!(!web.contains("Wine"), "{web}");
    }

    #[test]
    fn no_ports_mentions_demand_and_skips_ownership_question() {
        let faq = game_faq(&detail(vec![], 12));
        let a = answer(&faq, "on Windows");
        assert!(a.contains("(PlayStation 2, 2005)"), "{a}");
        assert!(a.contains("12 people have asked for one."), "{a}");
        assert!(!faq.iter().any(|f| f.question.starts_with("Do I need")));
    }

    #[test]
    fn join_and_uses_oxford_free_list() {
        let items: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        assert_eq!(join_and(&items), "a, b and c");
    }
}
