use dioxus::prelude::*;
use recomphub_shared::seo::{meta_description, port_title};
use recomphub_shared::{PortDetail, TargetPlatform};

use super::jsonld;
use crate::api;
use crate::components::live::{LiveCount, LiveStore, use_live_watch};
use crate::components::picture::{Picture, avif_url};
use crate::components::report::ReportDialog;
use crate::components::seo::{Seo, SiteUrl};
use crate::components::time::RelativeTime;
use crate::components::ui::{
    AiBadge, Breadcrumb, PlayabilityMeter, PortCardView, ReleaseChip, Section, TargetIcon,
    VideoTabs, initials_of, keyed, target_label,
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
    rsx! { PortBody { detail } }
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
        div { class: "rounded-2xl border border-border bg-surface p-4",
            dt { class: "text-xs font-bold uppercase tracking-wide text-muted", "{label}" }
            dd { class: "mt-2 text-sm", {children} }
        }
    }
}

#[component]
fn PortBody(detail: PortDetail) -> Element {
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
    let description = meta_description(&format!(
        "{} is a {} of {}. It runs on {}. Compare GitHub stars, releases, AI usage and gameplay videos.",
        port.name,
        port.kind_label,
        game.name,
        targets_text.join(", ")
    ));
    let crumbs = port_crumbs(&d);
    let json_ld = jsonld::port_json_ld(&site, &d, &crumbs);
    let path = format!("/games/{}/{}", game.slug, port.slug);
    let h1 = format!("{}: {} {}", port.name, game.name, port.kind_label);
    let initials = initials_of(&game.name);
    let bg_src = port
        .cover
        .as_ref()
        .filter(|m| m.ready)
        .and_then(|m| m.widths.iter().max().map(|w| avif_url(m, *w)));
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
    let last_commit = d.last_commit_at;
    let ai_tools = port.ai_tools.clone();
    let ai_evidence = port.ai_evidence.clone();

    rsx! {
        Seo { title, description, path, json_ld }
        section { class: "relative -mx-4 mb-12 overflow-hidden rounded-3xl border border-border/60",
            if let Some(src) = bg_src {
                img {
                    class: "absolute inset-0 -z-10 h-full w-full scale-110 object-cover opacity-30 blur-2xl saturate-[1.4]",
                    src: "{src}",
                    alt: "",
                    "aria-hidden": "true",
                    "loading": "lazy",
                    "decoding": "async",
                }
            }
            div { class: "relative grid gap-8 p-6 md:grid-cols-[220px_1fr] md:p-10",
                div { class: "perspective mx-auto w-40 md:w-full",
                    div { class: "cover-tilt aspect-[3/4] overflow-hidden rounded-2xl shadow-2xl",
                        Picture {
                            media: port.cover.clone(),
                            alt: format!("Cover art for {}", game.name),
                            initials: initials.clone(),
                            sizes: "(min-width: 768px) 220px, 50vw".to_string(),
                            eager: true,
                            class: "h-full w-full object-cover".to_string(),
                        }
                    }
                }
                div { class: "min-w-0",
                    Breadcrumb { items: crumbs.clone() }
                    h1 { class: "mt-3 text-4xl font-extrabold tracking-tight md:text-5xl", "{h1}" }
                    div { class: "mt-4 flex flex-wrap items-center gap-4",
                        LiveCount { game_id }
                        {keyed(rsx! { StarButton { key: "{star_key}", port_id, count: star_count } })}
                    }
                    div { class: "mt-6 flex flex-wrap gap-3",
                        if let Some(url) = download.clone() {
                            a { href: "{url}", target: "_blank", rel: "noopener", class: "rounded-full bg-accent px-5 py-2.5 text-sm font-bold text-white hover:opacity-90", "Download latest release" }
                        }
                        a { href: "{port.repo_url}", target: "_blank", rel: "noopener", class: "rounded-full border border-border px-5 py-2.5 text-sm font-bold hover:bg-surface-2", "Repository" }
                        if let Some(url) = homepage.clone() {
                            a { href: "{url}", target: "_blank", rel: "noopener", class: "rounded-full border border-border px-5 py-2.5 text-sm font-bold hover:bg-surface-2", "Website" }
                        }
                        a { href: "{edit_href}", class: "rounded-full border border-border px-5 py-2.5 text-sm font-bold hover:bg-surface-2", "Suggest an edit" }
                    }
                }
            }
        }

        Section { heading: "Facts".to_string(),
            dl { class: "grid gap-4 sm:grid-cols-2 lg:grid-cols-3",
                Fact { label: "Kind".to_string(), "{port.kind_label}" }
                Fact { label: "Original version".to_string(), "{source_name}" }
                Fact { label: "Runs on".to_string(),
                    div { class: "flex flex-wrap gap-2",
                        for (target, label) in runs.clone() {
                            span { class: "inline-flex items-center gap-1.5 rounded-full border border-border px-2.5 py-1",
                                TargetIcon { target }
                                "{label}"
                            }
                        }
                    }
                }
                Fact { label: "Author(s)".to_string(),
                    ul { class: "flex flex-col gap-2",
                        for (href, name, avatar) in authors.clone() {
                            li { class: "flex items-center gap-2",
                                Picture {
                                    media: avatar,
                                    alt: name.clone(),
                                    initials: initials_of(&name),
                                    sizes: "24px".to_string(),
                                    class: "h-6 w-6 rounded-full".to_string(),
                                }
                                if let Some(href) = href {
                                    a { href: "{href}", class: "font-semibold hover:text-accent", "{name}" }
                                } else {
                                    span { class: "font-semibold", "{name}" }
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

        if let Some(text) = requirements {
            Section { heading: "What you need".to_string(),
                p { class: "whitespace-pre-line leading-relaxed text-muted", "{text}" }
            }
        }

        if let Some(text) = description_text {
            Section { heading: "About this port".to_string(),
                p { class: "whitespace-pre-line leading-relaxed", "{text}" }
            }
        }

        if !videos.is_empty() {
            Section { heading: "Gameplay videos".to_string(),
                VideoTabs { videos }
            }
        }

        if !other_ports.is_empty() {
            Section { heading: other_title,
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

        div { class: "mt-10 flex justify-end",
            ReportDialog { game_id, port_id }
        }
    }
}
