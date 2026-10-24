use dioxus::prelude::*;
use recomphub_shared::dto::{InstallationGuides, PortAbout, RatingSummary};
use recomphub_shared::seo::{meta_description, port_title};
use recomphub_shared::{PortCard, PortDetail, TargetPlatform};

use super::jsonld;
use crate::api;
use crate::app::DISCORD_INVITE_LISTING;
use crate::components::comments::PortComments;
use crate::components::live::{LiveCount, LiveStore, use_live_watch};
use crate::components::maintenance::{MaintenanceBadge, MaintenancePanel};
use crate::components::report::ReportDialog;
use crate::components::seo::{Seo, SiteUrl};
use crate::components::shop::BuyShelf;
use crate::components::styles::{
    BADGE, BADGE_OUTLINE, BUTTON, BUTTON_DISCORD, BUTTON_MD, BUTTON_PRIMARY, CARD_SURFACE, H1,
    TEXT_LINK,
};
use crate::components::time::RelativeTime;
use crate::components::ui::{
    AiBadge, Breadcrumb, ExternalLink, InternalLink, PlayabilityMeter, PortCardView, PortHero,
    ReleaseChip, Section, TargetIcon, initials_of, keyed, target_label,
};
use crate::components::viewer::{StarButton, use_marks};
use crate::components::{DiscordCount, DiscordLink, FavoriteButton};
use crate::routes::Route;

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
    let has_lists = !(about.features.is_empty() && about.limitations.is_empty());
    rsx! {
        Section { heading: "About this port".to_string(), class: "mt-12".to_string(),
            p { class: "mb-4 max-w-3xl leading-relaxed", "{about.summary}" }
            div { class: "flex flex-col gap-6",
                div { class: "flex flex-col gap-2",
                    span { class: "{BADGE} {BADGE_OUTLINE}", "{about.technique_label}" }
                    p { class: "leading-relaxed text-muted-foreground", "{about.technique_note}" }
                }
                if has_lists {
                    div { class: "grid gap-4 md:grid-cols-2",
                        if !about.features.is_empty() {
                            AboutList { heading: "Features".to_string(), items: about.features.clone() }
                        }
                        if !about.limitations.is_empty() {
                            AboutList { heading: "Known limitations".to_string(), items: about.limitations.clone() }
                        }
                    }
                }
                p { class: "text-sm text-muted-foreground",
                    ExternalLink { url: repo_url, class: TEXT_LINK.to_string(), "Full project documentation ↗" }
                }
            }
        }
    }
}

/// One titled list inside the About section. Empty lists are not rendered by the caller.
#[component]
fn AboutList(heading: String, items: Vec<String>) -> Element {
    rsx! {
        div { class: "{CARD_SURFACE} p-4",
            h3 { class: "text-sm font-medium", "{heading}" }
            ul { class: "mt-2 flex list-disc flex-col gap-2 pl-5 text-sm",
                for item in items { li { "{item}" } }
            }
        }
    }
}

fn is_native_port(kind: &str) -> bool {
    kind.parse::<recomphub_shared::PortKind>()
        .is_ok_and(recomphub_shared::PortKind::is_port)
}

/// Render the stored, OS-specific installation instructions without executing commands.
#[component]
fn InstallationGuide(guide: Option<InstallationGuides>, repo_url: String) -> Element {
    let default_index = guide.as_ref().map_or(0, |g| {
        if g.windows.documented {
            0
        } else if g.macos.documented {
            1
        } else if g.linux.documented {
            2
        } else {
            0
        }
    });
    let mut selected = use_signal(|| default_index);
    let guide_url = format!("{repo_url}#readme");
    let systems = [
        ("windows", "Windows"),
        ("macos", "macOS"),
        ("linux", "Linux"),
    ];
    let current = match selected() {
        0 => guide.as_ref().map(|g| &g.windows),
        1 => guide.as_ref().map(|g| &g.macos),
        _ => guide.as_ref().map(|g| &g.linux),
    };
    let label = systems[selected()].1;
    rsx! {
        div { id: "installation", class: "scroll-mt-6",
            Section { heading: "Step 2: Install the port".to_string(), class: "mt-8".to_string(),
                div { class: "flex max-w-full overflow-x-auto border-b", role: "tablist", "aria-label": "Installation operating system",
                    for (index, (id, name)) in systems.iter().enumerate() {
                        button {
                            id: "installation-tab-{id}",
                            r#type: "button",
                            role: "tab",
                            class: if selected() == index { "shrink-0 border-b-2 border-primary px-4 py-2 text-sm font-medium" } else { "shrink-0 border-b-2 border-transparent px-4 py-2 text-sm text-muted-foreground" },
                            tabindex: if selected() == index { "0" } else { "-1" },
                            "aria-selected": selected() == index,
                            "aria-controls": "installation-panel",
                            onclick: move |_| selected.set(index),
                            onkeydown: move |event| {
                                let active = selected();
                                let next = match event.key() {
                                    Key::ArrowRight => Some((active + 1) % 3),
                                    Key::ArrowLeft => Some((active + 2) % 3),
                                    Key::Home => Some(0),
                                    Key::End => Some(2),
                                    _ => None,
                                };
                                if let Some(next) = next {
                                    event.prevent_default();
                                    selected.set(next);
                                    let id = ["installation-tab-windows", "installation-tab-macos", "installation-tab-linux"][next];
                                    spawn(async move {
                                        let eval = document::eval("document.getElementById(await dioxus.recv())?.focus();");
                                        if eval.send(id).is_ok() {
                                            let _ = eval.join::<()>().await;
                                        }
                                    });
                                }
                            },
                            "{name}"
                        }
                    }
                }
                article { id: "installation-panel", role: "tabpanel", "aria-labelledby": "installation-tab-{systems[selected()].0}", tabindex: "0", class: "{CARD_SURFACE} mt-4 min-w-0 p-5",
                    h3 { class: "font-medium", "{label} installation instructions" }
                    if let Some(os) = current {
                        if os.documented {
                            if !os.requirements.is_empty() {
                                h4 { class: "mt-4 text-sm font-semibold", "For {label}" }
                                ul { class: "mt-2 flex list-disc flex-col gap-2 pl-5 text-sm leading-relaxed",
                                    for requirement in os.requirements.iter() { li { "{requirement}" } }
                                }
                            }
                            if !os.steps.is_empty() {
                                ol { class: "mt-4 flex list-decimal flex-col gap-4 pl-5 text-sm leading-relaxed",
                                    for step in os.steps.iter() {
                                        li {
                                            h4 { class: "font-semibold", "{step.title}" }
                                            p { class: "mt-1 whitespace-pre-line", "{step.detail}" }
                                            if let Some(command) = step.command.as_ref() {
                                                pre { class: "mt-2 max-w-full overflow-x-auto rounded bg-muted p-3 text-xs", code { "{command}" } }
                                            }
                                        }
                                    }
                                }
                            }
                            if !os.notes.is_empty() {
                                h4 { class: "mt-4 text-sm font-semibold", "Notes" }
                                ul { class: "mt-2 flex list-disc flex-col gap-2 pl-5 text-sm leading-relaxed",
                                    for note in os.notes.iter() { li { "{note}" } }
                                }
                            }
                        } else {
                            p { class: "mt-3 text-sm text-muted-foreground", "No documented {label} installation instructions are available for this project." }
                        }
                    } else {
                        p { class: "mt-3 text-sm text-muted-foreground", "Installation instructions have not been recorded yet." }
                    }
                    ExternalLink { url: guide_url.clone(), class: format!("{TEXT_LINK} mt-4 inline-block text-sm"), "Read the project's documentation ↗" }
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

    let navigator = use_navigator();
    let redirect_slug = game.slug.clone();
    use_effect(move || {
        if is_sole_port {
            navigator.replace(Route::GamePage {
                slug: redirect_slug.clone(),
            });
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
            div { class: "mt-4 flex flex-wrap items-center justify-between gap-3",
                h1 { class: H1, "{port.name}" }
                FavoriteButton { game_id, game_name: game.name.clone() }
            }
            p { class: "mt-2 text-muted-foreground",
                "{subtitle} "
                Link { to: crate::routes::Route::GamePage { slug: game.slug.clone() }, class: "font-medium text-foreground underline-offset-4 hover:underline", "{game.name}" }
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

/// Everything about one port below its title: media and facts sidebar, the upstream release/source
/// row, generated description, maintenance, system requirements and other ports. Shared by the
/// port's own page and, when a game has exactly one published port, the game page. The caller
/// renders its own comments/report section once, since where that belongs on a merged game page
/// differs from the port page.
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
    let other_title = format!("Other ports of {}", game.name);
    let other_ports = d.other_ports.clone();
    let videos = d.videos.clone();
    let screenshots = d.screenshots.clone();
    let last_commit = d.last_commit_at;
    let ai_tools = port.ai_tools.clone();
    let ai_evidence = port.ai_evidence.clone();
    let mut about = d.about.clone();
    let has_about = about.is_some();
    let mut installation_requirements = about
        .as_ref()
        .map(|a| a.requirements.clone())
        .unwrap_or_default();
    if let Some(requirements) = requirements.filter(|text| !text.trim().is_empty())
        && !installation_requirements.contains(&requirements)
    {
        installation_requirements.push(requirements);
    }
    let installation = about.as_mut().and_then(|about| about.installation.take());
    let show_installation = is_native_port(&port.kind);
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

    #[cfg(feature = "desktop")]
    let native_actions = rsx! { crate::desktop::install_actions::InstallActions { port_id } };
    #[cfg(not(feature = "desktop"))]
    let native_actions = rsx! {};

    rsx! {
        div { class: "grid grid-cols-1 gap-6 md:grid-cols-[minmax(0,1fr)_300px]",
            PortHero {
                cover: if embedded { None } else { port.cover.clone() },
                screenshots: screenshots.clone(),
                videos: videos.clone(),
                alt: format!("{} gameplay", port.name),
                initials: initials.clone(),
            }
            aside { class: "flex flex-col gap-4",
                div { id: "port-download", class: "flex flex-col gap-3",
                    if show_installation {
                        if let Some(url) = download.clone() {
                            ExternalLink { url, class: format!("{BUTTON} {BUTTON_PRIMARY} {BUTTON_MD} w-full"), "Download" }
                            p { class: "text-xs leading-relaxed text-muted-foreground", "Opens the project's release page. Choose the download for your system; files are not verified by RecompHub." }
                        } else {
                            p { class: "text-sm font-medium", "No download available" }
                            p { class: "text-xs leading-relaxed text-muted-foreground", "No release is recorded. Check the repository for build instructions." }
                        }
                    } else {
                        p { class: "text-sm text-muted-foreground", "Source project, not a playable download." }
                    }
                    ExternalLink { url: port.repo_url.clone(), class: format!("{TEXT_LINK} text-sm"), "View repository" }
                }
                InternalLink { href: "#ratings-heading".to_string(), class: "text-sm font-medium hover:underline".to_string(), "{rating_preview}" }
                dl { class: "flex flex-col gap-2",
                    Fact { label: "Release".to_string(),
                        ReleaseChip {
                            version: port.latest_version.clone(),
                            at: port.latest_release_at,
                            state_label: port.release_state_label.clone(),
                        }
                    }
                    Fact { label: "Activity".to_string(),
                        if let Some(maintenance) = port.maintenance {
                            MaintenanceBadge { level: maintenance.level }
                        } else {
                            "Not checked"
                        }
                    }
                    Fact { label: "Developer".to_string(),
                        div { class: "flex flex-wrap justify-end gap-x-1.5",
                            for (index, (href, name, _)) in authors.iter().cloned().enumerate() {
                                span { key: "{index}",
                                    if let Some(href) = href {
                                        InternalLink { href, class: "font-medium hover:underline".to_string(), "{name}" }
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
                        AiBadge { usage: port.ai_usage.clone(), tools: ai_tools.clone(), evidence: ai_evidence.clone(), align_end: true }
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

        if show_installation { {native_actions} }

        if show_installation {
            Section { heading: "Step 1: Get the original game".to_string(), class: "mt-8".to_string(),
                div { id: "game-data", class: "{CARD_SURFACE} scroll-mt-6 p-5",
                    div {
                        h3 { class: "font-medium", "Check the original game files" }
                        p { class: "mt-2 text-sm text-muted-foreground", "Original version used by this project: {source_name}. Already own a compatible copy? You don't need to buy it again. Another edition may not contain compatible files." }
                        if !installation_requirements.is_empty() {
                            ul { class: "mt-3 flex list-disc flex-col gap-2 pl-5 text-sm leading-relaxed",
                                for requirement in installation_requirements.iter() { li { "{requirement}" } }
                            }
                        } else {
                            p { class: "mt-3 text-sm", "The exact files and supported editions are not recorded. Check the project's requirements before buying a copy or moving game data." }
                        }
                        ExternalLink { url: format!("{}#readme", port.repo_url), class: format!("{TEXT_LINK} mt-3 inline-block text-sm"), "Check project requirements ↗" }
                        p { class: "mt-3 text-xs text-muted-foreground", "Use game files you are entitled to supply. RecompHub does not provide original game data." }
                    }
                    BuyShelf { offers: d.buy_offers.clone(), game_name: game.name.clone(), cover: game.cover.clone(), has_ports: true, embedded: true }
                }
            }
            InstallationGuide {
                guide: installation,
                repo_url: port.repo_url.clone(),
            }
        } else if !installation_requirements.is_empty() {
            Section { heading: "Project requirements".to_string(), class: "mt-8".to_string(),
                ul { class: "flex list-disc flex-col gap-2 pl-5 text-sm",
                    for requirement in installation_requirements.iter() { li { "{requirement}" } }
                }
            }
        }

        if !other_ports.is_empty() {
            Section { heading: other_title, class: "mt-8".to_string(),
                ul { class: "grid gap-4 md:grid-cols-2",
                    for other in other_ports.iter() {
                        {
                            let key = format!("{}-{}", other.id, other.star_count);
                            rsx! { li { key: "{key}", PortCardView { port: other.clone() } } }
                        }
                    }
                }
            }
        }

        div { class: "mt-6",
            if let Some(m) = port.maintenance {
                MaintenancePanel { maintenance: m, weekly_commits: d.weekly_commits.clone() }
            } else {
                p { class: "text-sm text-muted-foreground", "Project activity has not been checked yet." }
            }
        }
        div { class: "mt-3 flex flex-wrap items-center gap-4 text-sm text-muted-foreground",
            LiveCount { game_id }
            {keyed(rsx! { StarButton { key: "{star_key}", port_id, count: star_count } })}
        }

        div { class: "{CARD_SURFACE} mt-6 flex flex-wrap items-center justify-between gap-4 p-4",
            span { class: "text-sm text-muted-foreground",
                "Stuck on {port.name}, or want to help maintain it? Ask in the Discord."
            }
            div { class: "flex shrink-0 items-center gap-3",
                DiscordCount {}
                DiscordLink {
                    href: DISCORD_INVITE_LISTING,
                    label: "Join Discord",
                    class: "{BUTTON} {BUTTON_DISCORD} {BUTTON_MD}",
                }
            }
        }

        if let Some(about) = about {
            AboutPort { about, repo_url: port.repo_url.clone() }
        }

        if !has_about {
            Section { heading: "About this port".to_string(), class: "mt-12".to_string(),
                if let Some(text) = description_text {
                    p { class: "max-w-3xl whitespace-pre-line leading-relaxed", "{text}" }
                } else {
                    p { class: "max-w-3xl leading-relaxed",
                        "{port.name} is a {port.kind_label.to_lowercase()} of {game.name}. "
                        if port.kind == "static_recomp" {
                            "Static recompilation translates the original game's machine code to run natively on a new system."
                        } else if port.kind == "decompilation" {
                            "This project reconstructs the original source code. It is not a playable native port."
                        } else {
                            "The project's supported systems and release status are listed above. Check its documentation for features and known limitations."
                        }
                    }
                }
            }
        }
        div { class: "mt-4 flex flex-wrap gap-x-5 gap-y-2 text-sm",
            if let Some(url) = homepage {
                ExternalLink { url, class: TEXT_LINK.to_string(), "Project website" }
            }
            Link { to: crate::routes::Route::EditPortPage { port_id: port.id }, class: TEXT_LINK, "Suggest an edit" }
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
