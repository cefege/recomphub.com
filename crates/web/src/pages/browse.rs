use dioxus::prelude::*;
use recomphub_shared::seo::{meta_description, runs_on_title};
use recomphub_shared::{GameCard, PortCard};

use crate::api;
use crate::components::listing::{GameListTable, PortTable, WatchVisible, parse_page};
use crate::components::styles::{H1, LEAD};
use crate::components::{
    Breadcrumb, CardGrid, EmptyState, GameCardView, Pager, PortCardView, Seo, ViewToggle, keyed,
    target_label, with_query,
};

/// A count with its noun, pluralised: `1 port`, `3 ports`. Callers hide it when it is zero.
pub(crate) fn count_label(n: i64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("{n} {one}")
    } else {
        format!("{n} {many}")
    }
}

/// A page of games as cards, or as a compact table for `?view=list`.
#[component]
pub(crate) fn GameGrid(games: Vec<GameCard>, list: bool) -> Element {
    if games.is_empty() {
        return rsx! { EmptyState { text: "No games here yet.".to_string() } };
    }
    if list {
        return rsx! { GameListTable { games } };
    }
    rsx! {
        CardGrid {
            for g in games.iter().cloned() {
                li { key: "{g.id}", GameCardView { game: g } }
            }
        }
    }
}

/// A page of ports as cards, or as a compact table for `?view=list`.
#[component]
pub(crate) fn PortGrid(ports: Vec<PortCard>, list: bool) -> Element {
    if ports.is_empty() {
        return rsx! { EmptyState { text: "No ports here yet.".to_string() } };
    }
    if list {
        return rsx! { PortTable { ports } };
    }
    rsx! {
        CardGrid {
            for p in ports.iter().cloned() {
                li { key: "{p.id}", PortCardView { port: p, show_game: true } }
            }
        }
    }
}

/// `/runs-on/:target`: every port that runs natively on one target.
#[component]
pub fn RunsOnPage(target: String, page: String, view: String) -> Element {
    let page = parse_page(&page);
    let list = view == "list";
    keyed(rsx! { RunsOnInner { key: "{target}-{page}-{list}", target, page, list } })
}

#[component]
fn RunsOnInner(target: String, page: i64, list: bool) -> Element {
    let loaded = use_loader({
        let target = target.clone();
        move || api::runs_on(target.clone(), page)
    })?;
    let data = loaded();

    let label = target_label(&target);
    let path = format!("/runs-on/{target}");
    let pager_base = if list {
        with_query(&path, "view=list")
    } else {
        path.clone()
    };
    let title = runs_on_title(&label);
    let description = meta_description(&format!(
        "Native ports and recompilations of games that run on {label}. Compare GitHub stars, releases, AI usage and gameplay videos."
    ));
    let noindex = list;
    let count = (data.total > 0).then(|| count_label(data.total, "port", "ports"));

    rsx! {
        Seo { title, description, path: path.clone(), noindex }
        div { class: "space-y-8",
            div { class: "space-y-4",
                Breadcrumb { items: vec![
                    ("Home".to_string(), "/".to_string()),
                    (label.clone(), path.clone()),
                ] }
                div { class: "flex flex-wrap items-end justify-between gap-4",
                    div { class: "space-y-2",
                        h1 { class: H1, "Games running natively on {label}" }
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

/// `/wanted`: games with no port yet, ordered by how many people want one.
#[component]
pub fn WantedPage(page: String, view: String) -> Element {
    let page = parse_page(&page);
    let list = view == "list";
    keyed(rsx! { WantedInner { key: "{page}-{list}", page, list } })
}

#[component]
fn WantedInner(page: i64, list: bool) -> Element {
    let loaded = use_loader(move || api::wanted(page))?;
    let data = loaded();

    let path = "/wanted".to_string();
    let pager_base = if list {
        with_query(&path, "view=list")
    } else {
        path.clone()
    };
    let title = "Wanted: games without a native port | RecompHub".to_string();
    let description = meta_description(
        "Games with no native port or recompilation yet, ranked by how many people want one. Want a game? Press Want a port on its page, and submit a port when one exists.",
    );
    let visible: Vec<i64> = data.items.iter().map(|g| g.id).collect();

    rsx! {
        Seo { title, description, path: path.clone(), noindex: list }
        WatchVisible { ids: visible }
        div { class: "space-y-8",
            div { class: "space-y-4",
                Breadcrumb { items: vec![
                    ("Home".to_string(), "/".to_string()),
                    ("Wanted".to_string(), path.clone()),
                ] }
                div { class: "flex flex-wrap items-end justify-between gap-4",
                    div { class: "space-y-2",
                        h1 { class: H1, "Wanted games" }
                        p { class: "{LEAD} max-w-2xl", "Games with no native port yet, ranked by how many people want one." }
                    }
                    ViewToggle { base: path.clone(), list }
                }
            }
            GameGrid { games: data.items.clone(), list }
            Pager { base: pager_base, page, pages: data.pages }
        }
    }
}

/// `/new`: the newest ports, by the later of their creation and latest release.
#[component]
pub fn NewPage(page: String, view: String) -> Element {
    let page = parse_page(&page);
    let list = view == "list";
    keyed(rsx! { NewInner { key: "{page}-{list}", page, list } })
}

#[component]
fn NewInner(page: i64, list: bool) -> Element {
    let loaded = use_loader(move || api::just_released(page))?;
    let data = loaded();

    let path = "/new".to_string();
    let pager_base = if list {
        with_query(&path, "view=list")
    } else {
        path.clone()
    };
    let title = "New Game Ports and Recomp Releases | RecompHub".to_string();
    let description = meta_description(
        "The newest native ports and recompilations, with their latest releases, runs-on platforms and AI usage.",
    );

    rsx! {
        Seo { title, description, path: path.clone(), noindex: list }
        div { class: "space-y-8",
            div { class: "space-y-4",
                Breadcrumb { items: vec![
                    ("Home".to_string(), "/".to_string()),
                    ("New".to_string(), path.clone()),
                ] }
                div { class: "flex flex-wrap items-end justify-between gap-4",
                    div { class: "space-y-2",
                        h1 { class: H1, "Just released" }
                        p { class: "{LEAD} max-w-2xl", "The newest native ports and recompilations, as they ship." }
                    }
                    ViewToggle { base: path.clone(), list }
                }
            }
            PortGrid { ports: data.items.clone(), list }
            Pager { base: pager_base, page, pages: data.pages }
        }
    }
}
