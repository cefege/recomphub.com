use dioxus::prelude::*;
use recomphub_shared::{ActivityItem, HitWindow, PlatformRef};

use crate::api;
use crate::components::listing::{WINDOWS, chip_class};
use crate::components::{GameCardView, LiveCount, Seo, keyed};

/// Home page: hero, live rail, trending, just released, live activity, wanted and browse tiles.
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
    let d = data();
    let title = "RecompHub – Every Game Recompilation & Native PC Port".to_string();
    let description = "Every recompilation and native port of console games, on Windows, Mac, Linux and in your browser.".to_string();
    let top10: Vec<_> = d.trending.iter().take(10).cloned().collect();
    let rail_fallback: Vec<_> = d.trending.iter().take(12).cloned().collect();
    let rail_live = !d.live_top.is_empty();
    rsx! {
        Seo { title, description, path: "/".to_string() }

        section { class: "relative -mx-4 mb-12 overflow-hidden rounded-3xl border border-border/60 px-6 py-16 text-center",
            h1 { class: "text-4xl font-black tracking-tight md:text-6xl", "Play the classics natively." }
            p { class: "mx-auto mt-4 max-w-2xl text-lg text-muted",
                "Every recompilation and native port of console games, on Windows, Mac, Linux and in your browser."
            }
            form { action: "/search", method: "get", class: "mx-auto mt-8 flex max-w-xl",
                input {
                    r#type: "search",
                    name: "q",
                    placeholder: "Search a game or port",
                    class: "w-full rounded-2xl border border-border bg-surface px-5 py-4 text-lg",
                }
            }
            div { class: "mt-8 flex flex-wrap justify-center gap-6 text-sm text-muted tabular",
                span { "{d.stats.games_with_port} games with a native port" }
                span { "{d.stats.ports} ports" }
                span { "{d.stats.platforms_with_ports} original platforms" }
            }
        }

        section { class: "mb-12",
            h2 { class: "mb-4 text-2xl font-extrabold tracking-tight",
                if rail_live { "Live now" } else { "Trending" }
            }
            div { class: "rail flex snap-x gap-4 overflow-x-auto pb-2",
                if rail_live {
                    for g in d.live_top.iter().cloned() {
                        div { key: "{g.game_id}", class: "w-40 shrink-0 snap-start",
                            a { href: "/games/{g.slug}", class: "block",
                                h3 { class: "mt-1 text-sm font-extrabold", "{g.name}" }
                                LiveCount { game_id: g.game_id, portless: g.port_count == 0 }
                            }
                        }
                    }
                } else {
                    for r in rail_fallback.iter().cloned() {
                        div { key: "{r.game.id}", class: "w-40 shrink-0 snap-start",
                            GameCardView { game: r.game }
                        }
                    }
                }
            }
        }

        section { class: "mb-12",
            h2 { class: "mb-4 text-2xl font-extrabold tracking-tight", "Trending" }
            nav { class: "mb-6 flex flex-wrap gap-2",
                for (w, key, label) in WINDOWS {
                    a {
                        key: "{key}",
                        href: "/?window={key}",
                        class: chip_class(w == selected),
                        "{label}"
                    }
                }
            }
            if !d.window_has_scores {
                p { class: "mb-4 text-sm text-muted", "Not enough activity in this window yet; ordered by popularity." }
            }
            ol { class: "grid grid-cols-2 gap-6 md:grid-cols-5",
                for (i, r) in top10.into_iter().enumerate() {
                    li { key: "{r.game.id}", class: "relative",
                        GameCardView { game: r.game, rank: Some((i + 1) as i64) }
                    }
                }
            }
            a { href: "/trending?window={selected_key}", class: "mt-6 inline-block font-semibold accent-text", "Full ranking" }
        }

        section { class: "mb-12 grid gap-10 lg:grid-cols-2",
            div {
                h2 { class: "mb-4 text-2xl font-extrabold tracking-tight", "Just released" }
                ul { class: "grid grid-cols-2 gap-4 md:grid-cols-3",
                    for p in d.just_released.iter().cloned() {
                        li { key: "{p.id}",
                            article { class: "card rounded-2xl border border-border bg-surface p-3",
                                a { href: "/games/{p.game_slug}/{p.slug}", class: "block",
                                    h3 { class: "font-extrabold leading-tight", "{p.name}" }
                                    p { class: "text-xs text-muted", "{p.game_name}" }
                                }
                                if let Some(v) = p.latest_version.clone() {
                                    span { class: "mt-2 inline-block rounded-full bg-surface-2 px-2 py-0.5 text-xs", "{v}" }
                                }
                            }
                        }
                    }
                }
            }
            div {
                h2 { class: "mb-4 text-2xl font-extrabold tracking-tight", "Live activity" }
                ol { class: "flex flex-col gap-2",
                    for a in d.activity.iter().cloned() {
                        li { key: "{a.id}", class: "animate-rise rounded-xl border border-border bg-surface px-4 py-2 text-sm",
                            a { href: "/games/{a.game_slug}", class: "hover:text-foreground", "{activity_text(&a)}" }
                        }
                    }
                }
            }
        }

        section { class: "mb-12",
            h2 { class: "mb-4 text-2xl font-extrabold tracking-tight", "Most wanted (no port yet)" }
            div { class: "grid grid-cols-2 gap-6 md:grid-cols-4",
                for g in d.wanted.iter().cloned() {
                    div { key: "{g.id}", GameCardView { game: g } }
                }
            }
        }

        section { class: "mb-12 grid gap-10 lg:grid-cols-2",
            div {
                h2 { class: "mb-4 text-2xl font-extrabold tracking-tight", "Where are they from?" }
                div { class: "grid grid-cols-2 gap-4 md:grid-cols-3",
                    for p in d.platforms.iter().cloned() {
                        PlatformTile { platform: p }
                    }
                }
            }
            div {
                h2 { class: "mb-4 text-2xl font-extrabold tracking-tight", "Where do they run?" }
                div { class: "grid grid-cols-2 gap-4",
                    for t in d.runs_on.iter().cloned() {
                        a { key: "{t.target}", href: "/runs-on/{t.target}",
                            class: "rounded-2xl border border-border bg-surface p-6 transition hover:-translate-y-1",
                            p { class: "text-lg font-extrabold", "{t.label}" }
                            p { class: "text-sm text-muted tabular", "{t.port_count} ports" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn PlatformTile(platform: PlatformRef) -> Element {
    let slug = platform.slug.clone().unwrap_or_default();
    let tint = family_tint(platform.family.as_deref().unwrap_or(""));
    rsx! {
        a { href: "/platforms/{slug}",
            class: "rounded-2xl border border-border bg-gradient-to-br p-5 transition hover:-translate-y-1 {tint}",
            p { class: "font-extrabold", "{platform.name}" }
            p { class: "text-sm text-muted tabular", "{platform.ported_count} ported" }
        }
    }
}

/// Gradient tint per platform family: Nintendo red, PlayStation blue, Xbox green, Sega blue-violet, PC grey.
fn family_tint(family: &str) -> &'static str {
    let f = family.to_ascii_lowercase();
    if f.contains("nintendo") {
        "from-red-500/30 to-surface"
    } else if f.contains("playstation") {
        "from-blue-500/30 to-surface"
    } else if f.contains("xbox") {
        "from-green-500/30 to-surface"
    } else if f.contains("sega") {
        "from-indigo-500/30 to-surface"
    } else {
        "from-slate-500/30 to-surface"
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
