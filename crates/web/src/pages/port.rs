use dioxus::prelude::*;
use recomphub_shared::dto::{PortAbout, RatingSummary};
use recomphub_shared::seo::{meta_description, port_title};
use recomphub_shared::{PortCard, PortDetail, TargetPlatform};

use super::jsonld;
use crate::api;
use crate::components::comments::PortComments;
use crate::components::live::{LiveCount, LiveStore, use_live_watch};
use crate::components::maintenance::MaintenancePanel;
use crate::components::report::ReportDialog;
use crate::components::seo::{Seo, SiteUrl};
use crate::components::styles::{
    BADGE, BADGE_OUTLINE, BUTTON, BUTTON_GHOST, BUTTON_MD, BUTTON_OUTLINE, BUTTON_PRIMARY,
    CARD_SURFACE, H1, TEXT_LINK,
};
use crate::components::time::RelativeTime;
use crate::components::ui::{
    AiBadge, Breadcrumb, PlayabilityMeter, PortCardView, PortHero, ReleaseChip, Section,
    TargetIcon, initials_of, keyed, target_label,
};
use crate::components::viewer::{StarButton, use_marks};

/// `/games/:game_slug/:port_slug`. Remounts when either slug changes.
#[component]
pub fn PortPage(game_slug: String, port_slug: String) -> Element {
    let key = format!("{game_slug}/{port_slug}");
    keyed(rsx! { PortPageInner { key: "{key}", game_slug, port_slug } })
}

#[component]
fn PortPageInner(game_slug: String, port_slug: String) -> Element {
    let (lookup_game, lookup_port) = (game_slug.clone(), port_slug.clone());
    let data = use_loader(move || api::port_detail(lookup_game.clone(), lookup_port.clone()))?;
    let detail = data();
    let port_id = detail.port.id;
    // The summary is non-essential: fall back to no ratings instead of failing the whole page.
    let ratings_data = use_loader(move || async move {
        Ok::<RatingSummary, ServerFnError>(api::port_ratings(port_id).await.unwrap_or_default())
    })?;
    rsx! { PortBody { detail, ratings: ratings_data() } }
}

fn port_crumbs(detail: &PortDetail) -> Vec<(String, String)> {
    let game = &detail.game;
    vec![
        ("Home".to_string(), "/".to_string()),
        ("Games".to_string(), "/games".to_string()),
        (game.name.clone(), format!("/games/{}", game.slug)),
        (
            detail.port.name.clone(),
            format!("/games/{}/{}", game.slug, detail.port.slug),
        ),
    ]
}

#[component]
fn Fact(label: String, children: Element) -> Element {
    rsx! {
        div { class: "flex items-start justify-between gap-3 border-b pb-2 text-sm last:border-0 last:pb-0",
            dt { class: "shrink-0 text-muted-foreground", "{label}" }
            dd { class: "text-right", {children} }
        }
    }
}

/// Generated description of the port, from its GitHub README and release notes.
#[component]
fn AboutPort(about: PortAbout, repo_url: String) -> Element {
    let has_lists =
        !(about.features.is_empty() && about.limitations.is_empty() && about.setup.is_empty());
    rsx! {
        Section { heading: "About this port".to_string(), class: "mt-12".to_string(),
            div { class: "flex flex-col gap-6",
                div { class: "flex flex-col gap-2",
                    span { class: "{BADGE} {BADGE_OUTLINE}", "{about.technique_label}" }
                    p { class: "leading-relaxed text-muted-foreground", "{about.technique_note}" }
                }
                if has_lists {
                    div { class: "grid gap-4 md:grid-cols-2",
                        if !about.features.is_empty() {
                            AboutList { heading: "Features".to_string(), items: about.features.clone(), numbered: false }
                        }
                        if !about.limitations.is_empty() {
                            AboutList { heading: "Known limitations".to_string(), items: about.limitations.clone(), numbered: false }
                        }
                        if !about.setup.is_empty() {
                            AboutList { heading: "Setup".to_string(), items: about.setup.clone(), numbered: true }
                        }
                    }
                }
                p { class: "text-sm text-muted-foreground",
                    "Generated from the repository's README and release notes "
                    RelativeTime { at: about.generated_at }
                    ". Check the "
                    a { href: "{repo_url}", target: "_blank", rel: "noopener", class: "{TEXT_LINK}", "repository" }
                    " for the latest details."
                }
            }
        }
    }
}

/// One titled list inside the About section. Empty lists are not rendered by the caller.
#[component]
fn AboutList(heading: String, items: Vec<String>, numbered: bool) -> Element {
    rsx! {
        div { class: "{CARD_SURFACE} p-4",
            h3 { class: "text-sm font-medium", "{heading}" }
            if numbered {
                ol { class: "mt-2 flex list-decimal flex-col gap-2 pl-5 text-sm",
                    for item in items { li { "{item}" } }
                }
            } else {
                ul { class: "mt-2 flex list-disc flex-col gap-2 pl-5 text-sm",
                    for item in items { li { "{item}" } }
                }
            }
        }
    }
}

#[component]
fn PortBody(detail: PortDetail, ratings: RatingSummary) -> Element {
    let site = use_context::<SiteUrl>().0;
    let port = detail.port.clone();
    let game = detail.game.clone();
    let game_id = port.game_id;
    let port_id = port.id;
    // While this is the game's only published native port, the game page shows the same
    // content at a shorter URL. No redirect is sent (a browser-cached 301 would survive after a
    // second port lands, and the user wants this to revert on its own): crawlers without
    // JavaScript still get the full page here, with the canonical link pointing at the game
    // page, and a real browser is bounced there client-side. The moment a second native port
    // exists, or this page is itself a decompilation (never merged onto the game page), both
    // effects stop.
    let is_sole_port = is_sole_native_port(&port.kind, &detail.other_ports);
    let primary: Vec<&str> = port
        .targets
        .iter()
        .filter_map(|t| t.parse::<TargetPlatform>().ok())
        .filter(|p| p.is_primary())
        .map(TargetPlatform::label)
        .collect();
    let targets_text: Vec<String> = port.targets.iter().map(|t| target_label(t)).collect();
    let title = port_title(&port.name, &game.name, &port.kind_label, &primary);
    let description = match detail.about.as_ref() {
        Some(about) => meta_description(&about.summary),
        None => meta_description(&format!(
            "{} is a {} of {}. It runs on {}. Compare GitHub stars, releases, AI usage and gameplay videos.",
            port.name,
            port.kind_label,
            game.name,
            targets_text.join(", ")
        )),
    };
    let crumbs = port_crumbs(&detail);
    let json_ld = jsonld::port_json_ld(&site, &detail, &crumbs, &ratings);
    let game_href = format!("/games/{}", game.slug);
    let path = if is_sole_port {
        game_href.clone()
    } else {
        format!("/games/{}/{}", game.slug, port.slug)
    };
    let subtitle = format!("{} of", article_kind(&port.kind_label));

    let redirect_href = game_href.clone();
    use_effect(move || {
        if is_sole_port {
            let _ = document::eval(&format!("location.replace('{redirect_href}')"));
        }
    });
    // Recorded here, not in `PortMainContent`: when that content is embedded on the game page
    // instead, the game page already records its own hit and live-watch for this `game_id`, and
    // a second call here would count every solo-port view twice.
    use_live_watch(Some(game_id), vec![game_id]);
    use_effect(move || {
        spawn(async move {
            let _ = api::record_hit(game_id).await;
        });
    });

    rsx! {
        Seo { title, description, path, json_ld }
        div {
            Breadcrumb { items: crumbs.clone() }
            h1 { class: "mt-4 {H1}", "{port.name}" }
            p { class: "mt-2 text-muted-foreground",
                "{subtitle} "
                a { href: "{game_href}", class: "font-medium text-foreground underline-offset-4 hover:underline", "{game.name}" }
            }
        }
        PortMainContent { detail, ratings: ratings.clone() }
        div { class: "mt-12",
            PortComments { key: "{port_id}", port_id, port_name: port.name.clone(), ratings }
        }
        div { class: "mt-12 flex justify-end",
            ReportDialog { game_id, port_id }
        }
    }
}

/// Everything about one port below its title: media and facts sidebar, the download row,
/// generated description, maintenance, system requirements and other ports. Shared by the
/// port's own page and, when a game has exactly one published port, the game page. The caller
/// renders its own comments/report section once, since where that belongs on a merged game
/// page differs from the port page.
#[component]
pub(crate) fn PortMainContent(
    detail: PortDetail,
    ratings: RatingSummary,
    #[props(default)] embedded: bool,
) -> Element {
    let live = use_context::<LiveStore>();
    let port_id = detail.port.id;
    let game_id = detail.port.game_id;
    let mut current = use_signal(|| detail.clone());
    let mut last_seen = use_signal(|| {
        live.activity
            .peek()
            .front()
            .map(|item| item.id)
            .unwrap_or(0)
    });

    use_marks(vec![port_id], vec![game_id]);

    let refresh = move || {
        let (game_slug, port_slug) = {
            let d = current.peek();
            (d.game.slug.clone(), d.port.slug.clone())
        };
        spawn(async move {
            if let Ok(next) = api::port_detail(game_slug, port_slug).await {
                current.set(next);
            }
        });
    };

    use_effect(move || {
        let front = live.activity.read().front().cloned();
        let Some(item) = front else {
            return;
        };
        if item.id <= *last_seen.peek() {
            return;
        }
        last_seen.set(item.id);
        let for_this_port = item.port_id == Some(port_id);
        let for_this_game = item.game_id == game_id;
        let refresh_now = match item.kind.as_str() {
            "starred" => for_this_port,
            "release" | "port_added" => for_this_game,
            _ => false,
        };
        if refresh_now {
            refresh();
        }
    });

    let d = current();
    let port = d.port.clone();
    let game = d.game.clone();
    let initials = initials_of(&game.name);
    let source_name = d
        .source_platform
        .as_ref()
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "Not stated".to_string());
    let runs: Vec<(String, String)> = port
        .targets
        .iter()
        .map(|t| (t.clone(), target_label(t)))
        .collect();
    let authors: Vec<(Option<String>, String, Option<recomphub_shared::MediaRef>)> = port
        .authors
        .iter()
        .map(|a| {
            let href = a
                .github_login
                .as_ref()
                .map(|login| format!("/authors/{login}"));
            (href, a.name.clone(), a.avatar.clone())
        })
        .collect();
    let license = d
        .license
        .clone()
        .unwrap_or_else(|| "Not stated".to_string());
    let toolchain = d
        .toolchain
        .clone()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "Not stated".to_string());
    let description_text = d.description.clone();
    let requirements = d.requirements.clone();
    let homepage = d.homepage_url.clone();
    let download = port.latest_release_url.clone();
    let stars = port.github_stars;
    let star_key = format!("{}-{}", port.id, port.star_count);
    let star_count = i64::from(port.star_count);
    let edit_href = format!("/submit/edit/{}", port.id);
    let other_title = format!("Other ports of {}", game.name);
    let other_ports = d.other_ports.clone();
    let videos = d.videos.clone();
    let screenshots = d.screenshots.clone();
    let last_commit = d.last_commit_at;
    let ai_tools = port.ai_tools.clone();
    let ai_evidence = port.ai_evidence.clone();
    let about = d.about.clone();
    let has_about = about.is_some();
    let about_requirements: Vec<String> = about
        .as_ref()
        .map(|a| a.requirements.clone())
        .unwrap_or_default();
    let rating_preview = if ratings.count > 0 {
        let avg = ratings
            .average
            .map(|a| (a * 10.0).round() / 10.0)
            .unwrap_or(0.0);
        format!(
            "{avg:.1}★ ({} rating{})",
            ratings.count,
            if ratings.count == 1 { "" } else { "s" }
        )
    } else {
        "No ratings yet".to_string()
    };

    rsx! {
        div { class: "grid gap-6 md:grid-cols-[1fr_300px]",
            PortHero {
                cover: if embedded { None } else { port.cover.clone() },
                screenshots: screenshots.clone(),
                videos: videos.clone(),
                alt: format!("{} gameplay", port.name),
                initials: initials.clone(),
            }
            aside { class: "flex flex-col gap-4",
                if !embedded
                    && let Some(about) = about.as_ref()
                {
                    p { class: "text-sm text-muted-foreground", "{about.summary}" }
                }
                a {
                    href: "#ratings-heading",
                    class: "text-sm font-medium hover:underline",
                    "{rating_preview}"
                }
                dl { class: "flex flex-col gap-2",
                    Fact { label: "Release".to_string(),
                        ReleaseChip {
                            version: port.latest_version.clone(),
                            at: port.latest_release_at,
                            state_label: port.release_state_label.clone(),
                        }
                    }
                    Fact { label: "Developer".to_string(),
                        div { class: "flex flex-wrap justify-end gap-x-1.5",
                            for (index, (href, name, _)) in authors.iter().cloned().enumerate() {
                                span { key: "{index}",
                                    if let Some(href) = href {
                                        a { href: "{href}", class: "font-medium hover:underline", "{name}" }
                                    } else {
                                        span { class: "font-medium", "{name}" }
                                    }
                                    if index + 1 < authors.len() { "," }
                                }
                            }
                        }
                    }
                    Fact { label: "Original version".to_string(), "{source_name}" }
                    Fact { label: "License".to_string(), "{license}" }
                    Fact { label: "Toolchain".to_string(), "{toolchain}" }
                    Fact { label: "GitHub stars".to_string(), "{stars}" }
                    Fact { label: "Last commit".to_string(),
                        if let Some(at) = last_commit {
                            RelativeTime { at }
                        } else {
                            "Unknown"
                        }
                    }
                    Fact { label: "AI use".to_string(),
                        AiBadge { usage: port.ai_usage.clone(), tools: ai_tools.clone(), evidence: ai_evidence.clone() }
                    }
                    Fact { label: "Playability".to_string(),
                        PlayabilityMeter { playability: port.playability.clone() }
                    }
                }
                div { class: "flex flex-wrap gap-1.5 border-t pt-3",
                    span { class: "{BADGE} {BADGE_OUTLINE}", "{port.kind_label}" }
                    for (target, label) in runs.clone() {
                        span { key: "{target}", class: "{BADGE} {BADGE_OUTLINE}",
                            TargetIcon { target }
                            "{label}"
                        }
                    }
                }
            }
        }

        div { class: "{CARD_SURFACE} mt-6 flex flex-wrap items-center justify-between gap-4 p-4",
            div { class: "flex flex-wrap items-center gap-4",
                LiveCount { game_id }
                {keyed(rsx! { StarButton { key: "{star_key}", port_id, count: star_count } })}
            }
            div { class: "flex flex-wrap gap-2",
                if let Some(url) = download.clone() {
                    a { href: "{url}", target: "_blank", rel: "noopener", class: "{BUTTON} {BUTTON_PRIMARY} {BUTTON_MD}", "Download latest release" }
                }
                a { href: "{port.repo_url}", target: "_blank", rel: "noopener", class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD}", "Repository" }
                if let Some(url) = homepage.clone() {
                    a { href: "{url}", target: "_blank", rel: "noopener", class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD}", "Website" }
                }
                a { href: "{edit_href}", class: "{BUTTON} {BUTTON_GHOST} {BUTTON_MD}", "Suggest an edit" }
            }
        }

        if let Some(about) = about {
            AboutPort { about, repo_url: port.repo_url.clone() }
        }

        if let Some(m) = port.maintenance {
            Section { heading: "Maintenance".to_string(), class: "mt-12".to_string(),
                MaintenancePanel { maintenance: m, weekly_commits: d.weekly_commits.clone() }
            }
        }

        if requirements.is_some() || !about_requirements.is_empty() {
            Section { heading: "System requirements".to_string(), class: "mt-12".to_string(),
                div { class: "flex flex-col gap-4",
                    if !about_requirements.is_empty() {
                        ul { class: "flex list-disc flex-col gap-1.5 pl-5 text-sm text-muted-foreground",
                            for item in about_requirements.clone() {
                                li { "{item}" }
                            }
                        }
                    }
                    if let Some(text) = requirements {
                        p { class: "whitespace-pre-line leading-relaxed text-muted-foreground", "{text}" }
                    }
                }
            }
        }

        if let Some(text) = description_text
            && !has_about
        {
            Section { heading: "Repository description".to_string(), class: "mt-12".to_string(),
                p { class: "whitespace-pre-line leading-relaxed", "{text}" }
            }
        }

        if !other_ports.is_empty() {
            Section { heading: other_title, class: "mt-12".to_string(),
                ul { class: "grid gap-4 md:grid-cols-2",
                    for other in other_ports {
                        {
                            let key = format!("{}-{}", other.id, other.star_count);
                            rsx! { li { key: "{key}", PortCardView { port: other } } }
                        }
                    }
                }
            }
        }
    }
}

/// "A static recompilation", "An unofficial port": the kind label as a sentence start.
fn article_kind(kind_label: &str) -> String {
    let lower = kind_label.to_lowercase();
    let article = match lower.chars().next() {
        Some('a' | 'e' | 'i' | 'o' | 'u') => "An",
        _ => "A",
    };
    format!("{article} {lower}")
}

/// Whether this port's game page should show its full content inline instead of a card list:
/// it must itself be a native port (never a decompilation, which always has its own page), and
/// no other published native port of the game may exist.
fn is_sole_native_port(kind: &str, other_ports: &[PortCard]) -> bool {
    kind.parse::<recomphub_shared::PortKind>()
        .is_ok_and(recomphub_shared::PortKind::is_port)
        && other_ports.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lone_native_port_is_sole() {
        assert!(is_sole_native_port("static_recomp", &[]));
    }

    #[test]
    fn a_native_port_with_another_published_port_is_not_sole() {
        let other = PortCard {
            id: 2,
            slug: "other".to_string(),
            game_id: 1,
            game_slug: "game".to_string(),
            game_name: "Game".to_string(),
            name: "Other".to_string(),
            kind: "static_recomp".to_string(),
            kind_label: "Static recompilation".to_string(),
            targets: Vec::new(),
            authors: Vec::new(),
            ai_usage: String::new(),
            ai_tools: Vec::new(),
            ai_evidence: Vec::new(),
            playability: String::new(),
            latest_version: None,
            latest_release_url: None,
            latest_release_at: None,
            release_state: String::new(),
            release_state_label: String::new(),
            github_stars: 0,
            star_count: 0,
            repo_url: String::new(),
            cover: None,
            maintenance: None,
        };
        assert!(!is_sole_native_port("static_recomp", &[other]));
    }

    #[test]
    fn a_decompilation_is_never_sole_even_with_no_other_native_ports() {
        // A decomp page's "other published ports" query only ever returns other native ports,
        // never other decomps, so an empty list here does not mean the decomp is alone: it means
        // the game simply has no native port. The decomp page must keep its own URL and content.
        assert!(!is_sole_native_port("decompilation", &[]));
    }
}
