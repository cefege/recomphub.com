use dioxus::prelude::*;
use recomphub_shared::HitWindow;
use recomphub_shared::seo::meta_description;

use crate::api;
use crate::components::listing::{RankingTable, WINDOWS, WatchVisible, parse_page};
use crate::components::styles::{H1, TAB, TAB_ACTIVE, TABS_LIST};
use crate::components::{
    Breadcrumb, CardGrid, EmptyState, GameCardView, Pager, Seo, TrendChip, ViewToggle, keyed,
    with_query,
};

/// Display label for a canonical window query value.
fn window_label(key: &str) -> &'static str {
    WINDOWS
        .iter()
        .find(|(_, k, _)| *k == key)
        .map(|(_, _, label)| *label)
        .unwrap_or("This week")
}

/// Class for a window switch: a segmented tab, active when it is the selected window.
fn window_tab_class(on: bool) -> String {
    if on {
        format!("{TAB} {TAB_ACTIVE}")
    } else {
        TAB.to_string()
    }
}

/// `/trending`: the top 100 games for a window, 50 per page, as cards or a table.
#[component]
pub fn TrendingPage(window: String, page: String, view: String) -> Element {
    let window = window
        .parse::<HitWindow>()
        .unwrap_or_else(|_| HitWindow::default_window());
    let page = parse_page(&page);
    let list = view == "list";
    let key = format!("{}-{page}-{list}", window.as_str());
    keyed(rsx! { TrendingInner { key: "{key}", window, page, list } })
}

#[component]
fn TrendingInner(window: HitWindow, page: i64, list: bool) -> Element {
    let is_default = window == HitWindow::default_window();
    let loaded = use_loader(move || api::trending(window, page))?;
    let data = loaded();

    let label = window_label(window.as_str());
    let base = format!("/trending?window={}", window.as_str());
    let pager_base = if list {
        with_query(&base, "view=list")
    } else {
        base.clone()
    };
    let path = if is_default && page > 1 {
        format!("/trending?page={page}")
    } else {
        "/trending".to_string()
    };
    let noindex = list || !is_default;
    let title = format!("Trending Game Ports & Recomps, {label} | RecompHub");
    let description = meta_description(&format!(
        "The most popular game recompilations and native ports, ranked by visits and live viewing ({label})."
    ));
    let visible: Vec<i64> = data.items.iter().map(|r| r.game.id).collect();
    let scored: Vec<(recomphub_shared::RankedGame, String)> = data
        .items
        .iter()
        .cloned()
        .map(|r| {
            let score = format!("{:.1}", r.score);
            (r, score)
        })
        .collect();
    let pages = data.pages;

    rsx! {
        Seo { title, description, path, noindex }
        WatchVisible { ids: visible }
        div { class: "space-y-8",
            div { class: "space-y-4",
                Breadcrumb { items: vec![
                    ("Home".to_string(), "/".to_string()),
                    ("Trending".to_string(), "/trending".to_string()),
                ] }
                h1 { class: H1, "Trending games" }
            }
            div { class: "flex flex-wrap items-center justify-between gap-4",
                nav { class: TABS_LIST, "aria-label": "Ranking window",
                    for (_, key, chip) in WINDOWS {
                        Link {
                            key: "{key}",
                            to: crate::routes::Route::TrendingPage { window: key.to_string(), page: String::new(), view: String::new() },
                            class: window_tab_class(key == window.as_str()),
                            "{chip}"
                        }
                    }
                }
                ViewToggle { base: base.clone(), list }
            }
            if data.items.is_empty() {
                EmptyState { text: "No ranked games in this window yet.".to_string() }
            } else if list {
                RankingTable { rows: data.items.clone() }
            } else {
                CardGrid {
                    for (r, score) in scored {
                        li { key: "{r.game.id}", class: "flex flex-col gap-2",
                            GameCardView { game: r.game.clone(), rank: Some(r.rank) }
                            div { class: "flex items-center justify-between px-1 text-xs text-muted-foreground",
                                if r.score > 0.0 {
                                    span { class: "tabular", "{score} pts" }
                                }
                                TrendChip { rank: r.rank, prev_rank: r.prev_rank }
                            }
                        }
                    }
                }
            }
            Pager { base: pager_base, page, pages }
        }
    }
}
