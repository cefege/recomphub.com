use dioxus::prelude::*;
use recomphub_shared::seo::meta_description;

use crate::api;
use crate::components::listing::WatchVisible;
use crate::components::{EmptyState, Section, Seo};
use crate::pages::browse::{GameGrid, PortGrid};

/// `/search?q=`: games and ports matching a term. Plain GET form, so it works without JavaScript.
#[component]
pub fn SearchPage(q: String) -> Element {
    let term = q.trim().to_string();
    let title = if term.is_empty() {
        "Search | RecompHub".to_string()
    } else {
        format!("Search: {term} | RecompHub")
    };
    let description = meta_description(
        "Search RecompHub for console game recompilations and native PC ports by game or port name.",
    );

    rsx! {
        Seo { title, description, path: "/search", noindex: true }
        div { class: "mx-auto max-w-7xl px-4 py-8",
            h1 { class: "text-3xl font-black tracking-tight md:text-4xl", "Search" }
            form {
                method: "get",
                action: "/search",
                role: "search",
                class: "mt-6 flex max-w-2xl gap-2",
                input {
                    r#type: "search",
                    name: "q",
                    value: "{term}",
                    placeholder: "Game or port name",
                    class: "min-w-0 flex-1 rounded-xl border border-border bg-surface px-4 py-3 text-base outline-none focus:border-accent",
                }
                button {
                    r#type: "submit",
                    class: "rounded-xl accent-bg px-5 py-3 font-semibold text-white",
                    "Search"
                }
            }
            if term.is_empty() {
                EmptyState { text: "Type a game or port name to search.".to_string() }
            } else {
                SearchInner { key: "{term}", term: term.clone() }
            }
        }
    }
}

#[component]
fn SearchInner(term: String) -> Element {
    let loaded = use_loader({
        let term = term.clone();
        move || api::search(term.clone())
    })?;
    let results = loaded();
    let games = results.games.clone();
    let ports = results.ports.clone();
    let total = games.len() + ports.len();
    let visible: Vec<i64> = games
        .iter()
        .map(|g| g.id)
        .chain(ports.iter().map(|p| p.game_id))
        .collect();

    rsx! {
        WatchVisible { ids: visible }
        p { class: "mt-4 text-sm text-muted tabular", "{total} results for '{term}'" }
        if total == 0 {
            EmptyState { text: "No games or ports match this search.".to_string() }
        }
        if !games.is_empty() {
            Section { heading: "Games".to_string(), GameGrid { games, list: false } }
        }
        if !ports.is_empty() {
            Section { heading: "Ports".to_string(), PortGrid { ports, list: false } }
        }
    }
}
