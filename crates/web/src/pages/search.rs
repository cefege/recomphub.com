use dioxus::prelude::*;
use recomphub_shared::seo::meta_description;

use crate::api;
use crate::components::listing::WatchVisible;
use crate::components::styles::{BUTTON, BUTTON_MD, BUTTON_PRIMARY, H1, INPUT};
use crate::components::{EmptyState, Section, Seo};
use crate::pages::browse::{GameGrid, PortGrid, count_label};

/// `/search?q=`: games and ports matching a term. Plain GET form, so it works without JavaScript.
#[component]
pub fn SearchPage(q: String) -> Element {
    let term = q.trim().to_string();
    let mut query = use_signal(|| term.clone());
    use_effect(use_reactive((&term,), move |(term,)| {
        query.set(term);
    }));
    #[cfg(feature = "desktop")]
    let navigator = use_navigator();
    let title = if term.is_empty() {
        "Search | RecompHub".to_string()
    } else {
        format!("Search: {term} | RecompHub")
    };
    let description = meta_description(
        "Search RecompHub for game recompilations and native ports by game or port name.",
    );

    rsx! {
        Seo { title, description, path: "/search", noindex: true }
        div { class: "space-y-8",
            div { class: "space-y-4",
                h1 { class: H1, "Search" }
                form {
                    method: "get",
                    action: "/search",
                    role: "search",
                    class: "flex max-w-2xl gap-2",
                    onsubmit: move |event| {
                        #[cfg(feature = "desktop")]
                        {
                            event.prevent_default();
                            navigator.push(crate::routes::Route::SearchPage { q: query() });
                        }
                        #[cfg(not(feature = "desktop"))]
                        let _ = event;
                    },
                    input {
                        r#type: "search",
                        name: "q",
                        value: "{query}",
                        oninput: move |event| query.set(event.value()),
                        placeholder: "Game or port name",
                        class: "{INPUT} flex-1",
                    }
                    button {
                        r#type: "submit",
                        class: "{BUTTON} {BUTTON_PRIMARY} {BUTTON_MD}",
                        "Search"
                    }
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
    let summary = (total > 0).then(|| {
        format!(
            "{} for '{term}'",
            count_label(total as i64, "result", "results")
        )
    });

    rsx! {
        WatchVisible { ids: visible }
        if let Some(summary) = summary {
            p { class: "text-sm text-muted-foreground tabular", "{summary}" }
        }
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
