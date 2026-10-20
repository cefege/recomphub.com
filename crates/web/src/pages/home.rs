use dioxus::prelude::*;
use recomphub_shared::seo::home_title;
use recomphub_shared::{ActivityItem, HitWindow, PlatformRef, PortCard};

use crate::app::DISCORD_INVITE_HOME;

use crate::api;
use crate::components::listing::WINDOWS;
use crate::components::styles::{
    BADGE, BADGE_SECONDARY, BUTTON, BUTTON_DISCORD, BUTTON_GHOST, BUTTON_MD, BUTTON_OUTLINE,
    BUTTON_PRIMARY, BUTTON_SM, CARD_SURFACE, H2, INPUT, LEAD, TAB, TAB_ACTIVE, TABS_LIST,
};
use crate::components::ui::version_label;
use crate::components::{
    DiscordCount, DiscordLink, GameCardView, LiveCount, Picture, PlatformCoverage, RelativeTime,
    Seo, initials_of, keyed,
};

/// Home page: search, trending top 10, live now, just released, live activity, wanted,
/// a pointer for creators, and browse tiles.
#[component]
pub fn Home(window: String) -> Element {
    let selected = window
        .parse::<HitWindow>()
        .unwrap_or_else(|_| HitWindow::default_window());
    keyed(rsx! { HomeInner { key: "{selected.as_str()}", selected } })
}

#[component]
fn HomeInner(selected: HitWindow) -> Element {
    let selected_key = selected.as_str();
    let data = use_loader(move || api::home(selected))?;
    let coverage = use_loader(api::stats_coverage)?;
    let d = data();
    let description = "Recompilations and native ports of console and PC games, for Windows, macOS, Linux, Android, handhelds and the web.".to_string();
    let top10: Vec<_> = d.trending.iter().take(10).cloned().collect();
    rsx! {
        Seo { title: home_title().to_string(), description: description.clone(), path: "/".to_string() }

        section { class: "flex flex-col items-start gap-6 border-b pb-10",
            div {
                h1 { class: "max-w-3xl text-4xl font-semibold tracking-tight text-balance sm:text-5xl",
                    "Every game recompilation and native port"
                }
                p { class: "mt-4 max-w-2xl text-lg {LEAD}", "{description}" }
            }
            form { action: "/search", method: "get", role: "search", class: "flex w-full max-w-xl gap-2",
                input {
                    r#type: "search",
                    name: "q",
                    "aria-label": "Search a game or port",
                    placeholder: "Search a game or port…",
                    class: "{INPUT} h-10",
                }
                button { r#type: "submit", class: "{BUTTON} {BUTTON_PRIMARY} h-10 px-5", "Search" }
            }
            dl { class: "flex flex-wrap gap-x-8 gap-y-2 text-sm",
                Stat { value: d.stats.games_with_port, label: "games with a native port" }
                Stat { value: d.stats.ports, label: "ports and recompilations" }
                Stat { value: d.stats.platforms_with_ports, label: "original platforms" }
            }
            div { class: "flex flex-wrap items-center gap-3",
                DiscordLink {
                    href: DISCORD_INVITE_HOME,
                    label: "Join the Discord",
                    class: "{BUTTON} {BUTTON_DISCORD} {BUTTON_MD}",
                }
                DiscordCount {}
                span { class: "text-sm text-muted-foreground", "Share a build, ask for help, or spot a site bug." }
            }
        }

        section { class: "mt-12",
            div { class: "mb-6 flex flex-wrap items-center justify-between gap-4",
                h2 { class: H2, "Trending" }
                div { class: "flex items-center gap-2",
                    nav { class: TABS_LIST, "aria-label": "Ranking window",
                        for (w, key, label) in WINDOWS {
                            a {
                                key: "{key}",
                                href: "/?window={key}",
                                class: if w == selected { "{TAB} {TAB_ACTIVE}" } else { "{TAB}" },
                                "aria-current": if w == selected { "page" } else { "false" },
                                "{label}"
                            }
                        }
                    }
                    a { href: "/trending?window={selected_key}", class: "{BUTTON} {BUTTON_GHOST} {BUTTON_SM}", "Full ranking" }
                }
            }
            if !d.window_has_scores {
                p { class: "mb-4 text-sm text-muted-foreground", "Not enough activity in this window yet, so these are ordered by popularity." }
            }
            ol { class: "grid grid-cols-2 gap-x-4 gap-y-8 sm:grid-cols-3 md:grid-cols-5",
                for (i, r) in top10.into_iter().enumerate() {
                    li { key: "{r.game.id}",
                        GameCardView { game: r.game, rank: Some((i + 1) as i64) }
                    }
                }
            }
        }

        div { class: "mt-12",
            PlatformCoverage { coverage: coverage() }
        }

        if !d.live_top.is_empty() {
            section { class: "mt-12",
                h2 { class: "mb-4 {H2}", "Live now" }
                ul { class: "rail flex snap-x gap-3 overflow-x-auto pb-2",
                    for g in d.live_top.iter().cloned() {
                        li { key: "{g.game_id}", class: "shrink-0 snap-start",
                            a { href: "/games/{g.slug}", class: "{CARD_SURFACE} flex w-56 flex-col gap-1 p-3 transition-colors hover:bg-accent",
                                span { class: "truncate text-sm font-medium", "{g.name}" }
                                LiveCount { game_id: g.game_id, portless: g.port_count == 0 }
                            }
                        }
                    }
                }
            }
        }

        div { class: "mt-12 grid gap-12 lg:grid-cols-3",
            section { class: "lg:col-span-2",
                div { class: "mb-4 flex min-h-8 items-center justify-between gap-4",
                    h2 { class: H2, "Just released" }
                    a { href: "/new", class: "{BUTTON} {BUTTON_GHOST} {BUTTON_SM}", "View all" }
                }
                ul { class: "{CARD_SURFACE} divide-y",
                    for p in d.just_released.iter().cloned() {
                        ReleaseRow { key: "{p.id}", port: p }
                    }
                }
            }
            section {
                div { class: "mb-4 flex min-h-8 items-center",
                    h2 { class: H2, "Activity" }
                }
                ol { class: "flex flex-col gap-1 text-sm",
                    for a in d.activity.iter().cloned() {
                        li { key: "{a.id}", class: "animate-rise",
                            a { href: "/games/{a.game_slug}", class: "block rounded-md px-2 py-1.5 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground",
                                "{activity_text(&a)}"
                            }
                        }
                    }
                }
            }
        }

        section { class: "mt-12",
            div { class: "mb-4 flex min-h-8 items-center justify-between gap-4",
                h2 { class: H2, "Most wanted, no port yet" }
                a { href: "/wanted", class: "{BUTTON} {BUTTON_GHOST} {BUTTON_SM}", "View all" }
            }
            ul { class: "grid grid-cols-2 gap-x-4 gap-y-8 sm:grid-cols-4 lg:grid-cols-8",
                for g in d.wanted.iter().cloned() {
                    li { key: "{g.id}", GameCardView { game: g } }
                }
            }
        }

        section { class: "{CARD_SURFACE} mt-12 flex flex-col gap-4 p-6 sm:flex-row sm:items-center sm:justify-between",
            div {
                h2 { class: "text-lg font-semibold tracking-tight", "Make the next one" }
                p { class: "mt-1 text-sm text-muted-foreground",
                    "The most wanted games without a port, ports missing platforms, and ports nobody has filmed yet."
                }
            }
            a { href: "/creators", class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM} self-start sm:self-auto", "What to work on next" }
        }

        div { class: "mt-12 grid gap-12 lg:grid-cols-2",
            section {
                div { class: "mb-4 flex min-h-8 items-center justify-between gap-4",
                    h2 { class: H2, "Original platforms" }
                    a { href: "/platforms", class: "{BUTTON} {BUTTON_GHOST} {BUTTON_SM}", "All platforms" }
                }
                ul { class: "grid grid-cols-2 gap-3 sm:grid-cols-3",
                    for p in d.platforms.iter().take(12).cloned() {
                        PlatformTile { key: "{p.id}", platform: p }
                    }
                }
            }
            section {
                div { class: "mb-4 flex min-h-8 items-center",
                    h2 { class: H2, "Runs on" }
                }
                ul { class: "grid grid-cols-2 gap-3",
                    for t in d.runs_on.iter().cloned() {
                        li { key: "{t.target}",
                            a { href: "/runs-on/{t.target}",
                                class: "{CARD_SURFACE} flex flex-col gap-0.5 p-4 transition-colors hover:bg-accent",
                                span { class: "font-medium", "{t.label}" }
                                span { class: "text-sm text-muted-foreground tabular", "{plural(t.port_count, \"port\", \"ports\")}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn Stat(value: i64, label: &'static str) -> Element {
    rsx! {
        div { class: "flex items-baseline gap-1.5",
            dt { class: "sr-only", "{label}" }
            dd { class: "font-semibold tabular", "{value}" }
            span { class: "text-muted-foreground", "aria-hidden": "true", "{label}" }
        }
    }
}

/// One row of the "Just released" list: cover, port and game, version and time.
#[component]
fn ReleaseRow(port: PortCard) -> Element {
    let href = format!("/games/{}/{}", port.game_slug, port.slug);
    rsx! {
        li {
            a { href: "{href}", class: "flex items-center gap-3 p-3 transition-colors hover:bg-muted/50",
                div { class: "h-12 w-9 shrink-0 overflow-hidden rounded-sm bg-muted",
                    Picture {
                        media: port.cover.clone(),
                        alt: format!("{} cover art", port.game_name),
                        initials: initials_of(&port.game_name),
                        sizes: "36px",
                        class: "h-full w-full object-cover text-[10px]",
                    }
                }
                div { class: "min-w-0 flex-1",
                    p { class: "truncate text-sm font-medium", "{port.name}" }
                    p { class: "truncate text-xs text-muted-foreground", "{port.game_name}" }
                }
                div { class: "flex shrink-0 flex-col items-end gap-1",
                    if let Some(v) = port.latest_version.clone() {
                        span { class: "{BADGE} {BADGE_SECONDARY} tabular", "{version_label(&v)}" }
                    }
                    if let Some(at) = port.latest_release_at {
                        span { class: "text-xs text-muted-foreground", RelativeTime { at } }
                    }
                }
            }
        }
    }
}

#[component]
fn PlatformTile(platform: PlatformRef) -> Element {
    let slug = platform.slug.clone().unwrap_or_default();
    rsx! {
        li {
            a { href: "/platforms/{slug}",
                class: "{CARD_SURFACE} flex h-full flex-col gap-0.5 p-4 transition-colors hover:bg-accent",
                span { class: "font-medium leading-snug", "{platform.name}" }
                span { class: "text-sm text-muted-foreground tabular", "{platform.ported_count} ported" }
            }
        }
    }
}

/// `1 port`, `3 ports`.
fn plural(n: i64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// One-line activity text, linking to the game. Templates follow plan §9.
fn activity_text(a: &ActivityItem) -> String {
    let port = a.port_name.clone().unwrap_or_default();
    match a.kind.as_str() {
        "starred" => format!("Someone starred {port}"),
        "wanted" => format!("Someone wants a port of {}", a.game_name),
        "port_added" => format!("New port: {port} for {}", a.game_name),
        "release" => format!("{port} {} released", a.version.clone().unwrap_or_default()),
        "game_added" => format!("{} added", a.game_name),
        _ => a.game_name.clone(),
    }
}
