use std::collections::BTreeMap;

use dioxus::prelude::*;
use recomphub_shared::PlatformRef;
use recomphub_shared::seo::{meta_description, platform_target_title, platform_title};

use crate::api;
use crate::components::listing::{PRIMARY_TARGETS, WatchVisible, chip_class, parse_page};
use crate::components::styles::{H1, H2, LEAD};
use crate::components::{
    Breadcrumb, EmptyState, Pager, Seo, ViewToggle, keyed, target_label, with_query,
};
use crate::pages::browse::{GameGrid, PortGrid, count_label};

/// "3 games with a port · 24 games". The port part is hidden while no game has one.
fn port_and_game_counts(ported: i64, games: i64) -> String {
    let games = count_label(games, "game", "games");
    if ported == 0 {
        games
    } else {
        format!(
            "{} · {games}",
            count_label(ported, "game with a port", "games with a port")
        )
    }
}

/// `/platforms`: every original platform that has at least one game, grouped by family.
#[component]
pub fn PlatformsIndex() -> Element {
    let loaded = use_loader(api::platforms)?;
    let platforms = loaded();

    let mut families: BTreeMap<String, Vec<(String, PlatformRef)>> = BTreeMap::new();
    for p in platforms {
        let Some(slug) = p.slug.clone() else {
            continue;
        };
        let family = p.family.clone().unwrap_or_else(|| "Other".to_string());
        families.entry(family).or_default().push((slug, p));
    }
    let groups: Vec<(String, Vec<(String, PlatformRef)>)> = families.into_iter().collect();

    let title = "Game Platforms: Ports & Recompilations by Platform | RecompHub".to_string();
    let description = meta_description(
        "Browse every platform with native ports and recompilations, from Nintendo 64 and PlayStation to Xbox, Sega, Game Boy and PC.",
    );

    rsx! {
        Seo { title, description, path: "/platforms".to_string() }
        div { class: "space-y-8",
            div { class: "space-y-4",
                Breadcrumb { items: vec![
                    ("Home".to_string(), "/".to_string()),
                    ("Platforms".to_string(), "/platforms".to_string()),
                ] }
                div { class: "space-y-2",
                    h1 { class: H1, "Platforms" }
                    p { class: "{LEAD} max-w-2xl", "Every platform with games in the catalog, grouped by family." }
                }
            }
            if groups.is_empty() {
                EmptyState { text: "No platforms have games yet.".to_string() }
            }
            div { class: "space-y-12",
                for (family, items) in groups {
                    section { key: "{family}", class: "space-y-4",
                        h2 { class: H2, "{family}" }
                        ul { class: "grid grid-cols-2 gap-4 md:grid-cols-4 lg:grid-cols-6",
                            for (slug, p) in items {
                                li { key: "{p.id}",
                                    Link {
                                        to: crate::routes::Route::PlatformPage { slug: slug.clone(), page: String::new(), view: String::new() },
                                        class: "block rounded-xl border bg-card p-4 shadow-sm transition-colors hover:bg-accent",
                                        span { class: "block text-sm font-semibold leading-snug", "{p.name}" }
                                        span { class: "mt-2 block text-xs text-muted-foreground tabular",
                                            "{port_and_game_counts(p.ported_count, p.game_count)}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// `/platforms/:slug`: games for one original platform, ported games first.
#[component]
pub fn PlatformPage(slug: String, page: String, view: String) -> Element {
    let page = parse_page(&page);
    let list = view == "list";
    keyed(rsx! { PlatformInner { key: "{slug}-{page}-{list}", slug, page, list } })
}

#[component]
fn PlatformInner(slug: String, page: i64, list: bool) -> Element {
    let platform = use_loader({
        let slug = slug.clone();
        move || api::platform_ref(slug.clone())
    })?;
    let games = use_loader({
        let slug = slug.clone();
        move || api::platform_games(slug.clone(), page)
    })?;
    let platform = platform();
    let data = games();

    let path = format!("/platforms/{slug}");
    let pager_base = if list {
        with_query(&path, "view=list")
    } else {
        path.clone()
    };
    let title = platform_title(&platform.name, platform.game_count);
    let description = meta_description(&format!(
        "{} of {} {} games have a native port or recompilation. Pick a target to see which ones run on Windows, macOS, Linux or the web.",
        platform.ported_count, platform.game_count, platform.name
    ));
    let visible: Vec<i64> = data.items.iter().map(|g| g.id).collect();

    rsx! {
        Seo { title, description, path: path.clone(), noindex: list }
        WatchVisible { ids: visible }
        div { class: "space-y-8",
            div { class: "space-y-4",
                Breadcrumb { items: vec![
                    ("Home".to_string(), "/".to_string()),
                    ("Platforms".to_string(), "/platforms".to_string()),
                    (platform.name.clone(), path.clone()),
                ] }
                div { class: "flex flex-wrap items-end justify-between gap-4",
                    div { class: "space-y-2",
                        h1 { class: H1, "{platform.name} games" }
                        p { class: "text-sm text-muted-foreground tabular",
                            "{port_and_game_counts(platform.ported_count, platform.game_count)}"
                        }
                    }
                    ViewToggle { base: path.clone(), list }
                }
            }
            nav { class: "flex flex-wrap gap-2", "aria-label": "Runs on",
                for t in PRIMARY_TARGETS {
                    Link {
                        key: "{t}",
                        to: crate::routes::Route::PlatformTargetPage { slug: slug.clone(), target: t.to_string(), page: String::new(), view: String::new() },
                        class: chip_class(false),
                        "Runs on {target_label(t)}"
                    }
                }
            }
            GameGrid { games: data.items.clone(), list }
            Pager { base: pager_base, page, pages: data.pages }
        }
    }
}

/// `/platforms/:slug/:target`: ports of one platform that run on one target.
#[component]
pub fn PlatformTargetPage(slug: String, target: String, page: String, view: String) -> Element {
    let page = parse_page(&page);
    let list = view == "list";
    keyed(
        rsx! { PlatformTargetInner { key: "{slug}-{target}-{page}-{list}", slug, target, page, list } },
    )
}

#[component]
fn PlatformTargetInner(slug: String, target: String, page: i64, list: bool) -> Element {
    let platform = use_loader({
        let slug = slug.clone();
        move || api::platform_ref(slug.clone())
    })?;
    let ports = use_loader({
        let slug = slug.clone();
        let target = target.clone();
        move || api::platform_target(slug.clone(), target.clone(), page)
    })?;
    let platform = platform();
    let data = ports();

    let label = target_label(&target);
    let path = format!("/platforms/{slug}/{target}");
    let pager_base = if list {
        with_query(&path, "view=list")
    } else {
        path.clone()
    };
    let title = platform_target_title(&platform.name, &label);
    let description = meta_description(&format!(
        "{} games running natively on {label}, {}. Compare GitHub stars, releases, AI usage and gameplay videos for each port.",
        platform.name,
        count_label(data.total, "port", "ports")
    ));
    let noindex = list || data.total < 3;
    let total = data.total;
    let count = (total > 0).then(|| count_label(total, "port", "ports"));

    rsx! {
        Seo { title, description, path: path.clone(), noindex }
        div { class: "space-y-8",
            div { class: "space-y-4",
                Breadcrumb { items: vec![
                    ("Home".to_string(), "/".to_string()),
                    ("Platforms".to_string(), "/platforms".to_string()),
                    (platform.name.clone(), format!("/platforms/{slug}")),
                    (label.clone(), path.clone()),
                ] }
                div { class: "flex flex-wrap items-end justify-between gap-4",
                    div { class: "space-y-2",
                        h1 { class: H1, "{platform.name} games on {label}" }
                        if let Some(count) = count {
                            p { class: "text-sm text-muted-foreground tabular", "{count}" }
                        }
                    }
                    ViewToggle { base: path.clone(), list }
                }
            }
            PortGrid { ports: data.items.clone(), list }
            Pager { base: pager_base, page, pages: data.pages }
        }
    }
}
