use dioxus::prelude::*;
use recomphub_shared::dto::{PortAbout, RatingSummary};
use recomphub_shared::seo::{meta_description, port_title};
use recomphub_shared::{PortDetail, TargetPlatform};

use super::jsonld;
use crate::api;
use crate::components::comments::PortComments;
use crate::components::live::{LiveCount, LiveStore, use_live_watch};
use crate::components::maintenance::MaintenancePanel;
use crate::components::picture::Picture;
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
        div { class: "{CARD_SURFACE} p-4",
            dt { class: "text-xs font-medium text-muted-foreground", "{label}" }
            dd { class: "mt-2 text-sm", {children} }
        }
    }
}

/// Generated description of the port, from its GitHub README and release notes.
#[component]
fn AboutPort(about: PortAbout, repo_url: String) -> Element {
    let has_lists = !(about.features.is_empty()
        && about.requirements.is_empty()
        && about.limitations.is_empty()
        && about.setup.is_empty());
    rsx! {
        Section { heading: "About this port".to_string(), class: "mt-12".to_string(),
            div { class: "flex flex-col gap-6",
                p { class: "text-lg leading-relaxed", "{about.summary}" }
                div { class: "flex flex-col gap-2",
                    span { class: "{BADGE} {BADGE_OUTLINE}", "{about.technique_label}" }
                    p { class: "leading-relaxed text-muted-foreground", "{about.technique_note}" }
                }
                if has_lists {
                    div { class: "grid gap-4 md:grid-cols-2",
                        if !about.features.is_empty() {
                            AboutList { heading: "Features".to_string(), items: about.features.clone(), numbered: false }
                        }
                        if !about.requirements.is_empty() {
                            AboutList { heading: "Requirements".to_string(), items: about.requirements.clone(), numbered: false }
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
    use_live_watch(Some(game_id), vec![game_id]);

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
        spawn(async move {
            let _ = api::record_hit(game_id).await;
        });
    });

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
    let primary: Vec<&str> = port
        .targets
        .iter()
        .filter_map(|t| t.parse::<TargetPlatform>().ok())
        .filter(|p| p.is_primary())
        .map(TargetPlatform::label)
        .collect();
    let targets_text: Vec<String> = port.targets.iter().map(|t| target_label(t)).collect();
    let title = port_title(&port.name, &game.name, &port.kind_label, &primary);
    let description = match d.about.as_ref() {
        Some(about) => meta_description(&about.summary),
        None => meta_description(&format!(
            "{} is a {} of {}. It runs on {}. Compare GitHub stars, releases, AI usage and gameplay videos.",
            port.name,
            port.kind_label,
            game.name,
            targets_text.join(", ")
        )),
    };
    let crumbs = port_crumbs(&d);
    let json_ld = jsonld::port_json_ld(&site, &d, &crumbs, &ratings);
    let path = format!("/games/{}/{}", game.slug, port.slug);
    let subtitle = format!("{} of", article_kind(&port.kind_label));
    let game_href = format!("/games/{}", game.slug);
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
        div { class: "mt-6",
            PortHero {
                cover: port.cover.clone(),
                screenshots: screenshots.clone(),
                videos: videos.clone(),
                alt: format!("{} gameplay", port.name),
                initials: initials.clone(),
            }
        }
        div { class: "mt-6 flex flex-wrap items-center justify-between gap-4",
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

        Section { heading: "Facts".to_string(), class: "mt-12".to_string(),
            dl { class: "grid gap-4 sm:grid-cols-2 lg:grid-cols-3",
                Fact { label: "Kind".to_string(), "{port.kind_label}" }
                Fact { label: "Original version".to_string(), "{source_name}" }
                Fact { label: "Runs on".to_string(),
                    div { class: "flex flex-wrap gap-2",
                        for (target, label) in runs.clone() {
                            span { class: "{BADGE} {BADGE_OUTLINE}",
                                TargetIcon { target }
                                "{label}"
                            }
                        }
                    }
                }
                Fact { label: "Authors".to_string(),
                    ul { class: "flex flex-col gap-2",
                        for (href, name, avatar) in authors.clone() {
                            li { class: "flex items-center gap-2",
                                Picture {
                                    media: avatar,
                                    alt: name.clone(),
                                    initials: initials_of(&name),
                                    sizes: "24px".to_string(),
                                    class: "size-6 rounded-full text-[9px]".to_string(),
                                }
                                if let Some(href) = href {
                                    a { href: "{href}", class: "font-medium hover:underline", "{name}" }
                                } else {
                                    span { class: "font-medium", "{name}" }
                                }
                            }
                        }
                    }
                }
                Fact { label: "AI use".to_string(),
                    AiBadge { usage: port.ai_usage.clone(), tools: ai_tools.clone(), evidence: ai_evidence.clone() }
                }
                Fact { label: "Playability".to_string(),
                    PlayabilityMeter { playability: port.playability.clone() }
                }
                Fact { label: "Release".to_string(),
                    ReleaseChip {
                        version: port.latest_version.clone(),
                        at: port.latest_release_at,
                        state_label: port.release_state_label.clone(),
                    }
                }
                Fact { label: "License".to_string(), "{license}" }
                Fact { label: "GitHub stars".to_string(), "{stars}" }
                Fact { label: "Last commit".to_string(),
                    if let Some(at) = last_commit {
                        RelativeTime { at }
                    } else {
                        "Unknown"
                    }
                }
                Fact { label: "Toolchain".to_string(), "{toolchain}" }
            }
        }

        if let Some(m) = port.maintenance {
            Section { heading: "Maintenance".to_string(), class: "mt-12".to_string(),
                MaintenancePanel { maintenance: m, weekly_commits: d.weekly_commits.clone() }
            }
        }

        if let Some(text) = requirements {
            Section { heading: "What you need".to_string(), class: "mt-12".to_string(),
                p { class: "whitespace-pre-line leading-relaxed text-muted-foreground", "{text}" }
            }
        }

        if let Some(text) = description_text {
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
        div { class: "mt-12",
            PortComments { key: "{port_id}", port_id, port_name: port.name.clone(), ratings }
        }
        div { class: "mt-12 flex justify-end",
            ReportDialog { game_id, port_id }
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
