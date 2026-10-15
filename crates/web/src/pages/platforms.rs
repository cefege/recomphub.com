use std::collections::BTreeMap;

use dioxus::prelude::*;
use recomphub_shared::PlatformRef;
use recomphub_shared::seo::{meta_description, platform_target_title, platform_title};

use crate::api;
use crate::components::listing::{PRIMARY_TARGETS, WatchVisible, chip_class, parse_page};
use crate::components::{
    Breadcrumb, EmptyState, Pager, Seo, ViewToggle, keyed, target_label, with_query,
};
use crate::pages::browse::{GameGrid, PortGrid};

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

    let title = "Game Platforms: Ports & Recompilations by Console | RecompHub".to_string();
    let description = meta_description(
        "Browse every console platform with recompilations and native PC ports, from Nintendo 64 and PlayStation to Xbox, Sega and Game Boy.",
    );

    rsx! {
        Seo { title, description, path: "/platforms".to_string() }
        div { class: "mx-auto max-w-7xl px-4 py-8",
            Breadcrumb { items: vec![
                ("Home".to_string(), "/".to_string()),
                ("Platforms".to_string(), "/platforms".to_string()),
            ] }
            h1 { class: "mb-8 text-3xl font-black tracking-tight md:text-4xl", "Platforms" }
            if groups.is_empty() {
                EmptyState { text: "No platforms have games yet.".to_string() }
            }
            for (family, items) in groups {
                section { key: "{family}", class: "mb-10",
                    h2 { class: "mb-4 text-xl font-extrabold tracking-tight", "{family}" }
                    ul { class: "grid grid-cols-2 gap-3 md:grid-cols-4 lg:grid-cols-6",
                        for (slug, p) in items {
                            li { key: "{p.id}",
                                a {
                                    href: "/platforms/{slug}",
                                    class: "block rounded-2xl border border-border bg-surface p-4 transition hover:-translate-y-1 hover:border-accent hover:shadow-lg",
                                    span { class: "block text-sm font-extrabold leading-tight", "{p.name}" }
                                    span { class: "mt-2 block text-xs text-muted tabular",
                                        "{p.ported_count} with a port · {p.game_count} games"
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
        "{} {} games have a native PC port or recompilation, out of {} in total. Pick a target to see ports that run on Windows, Mac, Linux or the web.",
        platform.ported_count, platform.name, platform.game_count
    ));
    let visible: Vec<i64> = data.items.iter().map(|g| g.id).collect();

    rsx! {
        Seo { title, description, path: path.clone(), noindex: list }
        WatchVisible { ids: visible }
        div { class: "mx-auto max-w-7xl px-4 py-8",
            Breadcrumb { items: vec![
                ("Home".to_string(), "/".to_string()),
                ("Platforms".to_string(), "/platforms".to_string()),
                (platform.name.clone(), path.clone()),
            ] }
            div { class: "mb-6 flex flex-wrap items-end justify-between gap-4",
                div {
                    h1 { class: "text-3xl font-black tracking-tight md:text-4xl", "{platform.name} games" }
                    p { class: "mt-2 text-sm text-muted tabular",
                        "{platform.ported_count} with a port · {platform.game_count} games"
                    }
                }
                ViewToggle { base: path.clone(), list }
            }
            nav { class: "mb-8 flex flex-wrap gap-2", "aria-label": "Runs on",
                for t in PRIMARY_TARGETS {
                    a {
                        key: "{t}",
                        href: "/platforms/{slug}/{t}",
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
        "{} ports of {} games that run natively on {label}. Compare GitHub stars, releases, AI usage and gameplay videos for each port.",
        data.total, platform.name
    ));
    let noindex = list || data.total < 3;
    let total = data.total;

    rsx! {
        Seo { title, description, path: path.clone(), noindex }
        div { class: "mx-auto max-w-7xl px-4 py-8",
            Breadcrumb { items: vec![
                ("Home".to_string(), "/".to_string()),
                ("Platforms".to_string(), "/platforms".to_string()),
                (platform.name.clone(), format!("/platforms/{slug}")),
                (label.clone(), path.clone()),
            ] }
            div { class: "mb-6 flex flex-wrap items-end justify-between gap-4",
                div {
                    h1 { class: "text-3xl font-black tracking-tight md:text-4xl",
                        "{platform.name} games on {label}"
                    }
                    p { class: "mt-2 text-sm text-muted tabular", "{total} ports" }
                }
                ViewToggle { base: path.clone(), list }
            }
            PortGrid { ports: data.items.clone(), list }
            Pager { base: pager_base, page, pages: data.pages }
        }
    }
}
