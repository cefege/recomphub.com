use dioxus::prelude::*;

use crate::components::LibraryStore;
use crate::components::styles::{BUTTON, BUTTON_GHOST, BUTTON_SM, INPUT};
use crate::desktop::library::NativeLibraryStore;
use crate::routes::Route;

#[derive(Clone, Copy, PartialEq)]
pub enum DesktopPane {
    Saved,
    Installed,
    Browse,
    Updates,
}

const DESKTOP_LOGO: Asset = asset!("/assets/recomphub-logo.svg");
const DESKTOP_STYLE: Asset = asset!("/assets/desktop-library.css");

#[component]
pub fn DesktopWorkspace(children: Element) -> Element {
    let store = use_context::<NativeLibraryStore>();
    let saved = use_context::<LibraryStore>();
    let mut selected = use_signal(|| None::<i64>);
    let mut pane = use_signal(|| DesktopPane::Installed);
    use_context_provider(|| selected);
    use_context_provider(|| pane);
    let navigator = use_navigator();
    let library_page = matches!(use_route::<Route>(), Route::LibraryPage { .. });
    let mut filter = use_signal(String::new);
    let query = filter().trim().to_lowercase();
    use_effect(move || {
        if *store.loaded.read() {
            let selected_id = selected();
            let available = store
                .games
                .read()
                .iter()
                .any(|game| Some(game.port_id) == selected_id);
            if !available && let Some(first) = store.games.read().first() {
                selected.set(Some(first.port_id));
            }
        }
    });
    let games = store.games.read();
    let auth = use_context::<Signal<crate::desktop::session::AuthView>>();
    let auth_view = auth.read().clone();
    rsx! {
        document::Stylesheet { href: DESKTOP_STYLE }
        div { "data-theme": "dark", class: "desktop-workspace",
            header { class: "desktop-topbar",
                div { class: "desktop-brand",
                    img { src: DESKTOP_LOGO, alt: "", width: "26", height: "26" }
                    span { "RecompHub" }
                }
                NativeHistoryControls {}
                nav { class: "desktop-tabs", "aria-label": "Desktop navigation",
                    dioxus::prelude::Link { to: "/library", class: if library_page { "desktop-tab active" } else { "desktop-tab" }, "LIBRARY" }
                    dioxus::prelude::Link { to: "/games", class: if library_page { "desktop-tab" } else { "desktop-tab active" }, "BROWSE GAMES" }
                }
                dioxus::prelude::Link { to: "/search", class: "desktop-search-link", "Search projects" }
            }
            div { class: "desktop-body",
                aside { class: "desktop-sidebar", "aria-label": "Game library",
                    div { class: "desktop-sidebar-tools",
                        input { class: "{INPUT} desktop-game-search", r#type: "search", placeholder: "Search your games", "aria-label": "Search your games", value: "{filter}", oninput: move |event| filter.set(event.value()) }
                        div { class: "desktop-collections",
                            button { r#type: "button", class: if library_page && pane() == DesktopPane::Installed { "active" } else { "" },
                                onclick: move |_| { pane.set(DesktopPane::Installed); navigator.push(Route::LibraryPage { page: "1".into() }); },
                                "Installed"
                                span { "{games.len()}" }
                            }
                            button { r#type: "button", class: if library_page && pane() == DesktopPane::Saved { "active" } else { "" },
                                onclick: move |_| { pane.set(DesktopPane::Saved); navigator.push(Route::LibraryPage { page: "1".into() }); },
                                "Saved"
                                span { "{saved.ids.read().len()}" }
                            }
                        }
                    }
                    div { class: "desktop-game-list", "aria-label": "Installed games",
                        if !*store.loaded.read() {
                            p { class: "desktop-sidebar-message", "Loading your games…" }
                        } else if games.is_empty() {
                            p { class: "desktop-sidebar-message", "Your installed games will appear here." }
                        } else {
                            for game in games.iter().filter(|game| game.game_name.to_lowercase().contains(&query) || game.port_name.to_lowercase().contains(&query)) {
                                button {
                                    r#type: "button",
                                    class: if selected() == Some(game.port_id) && library_page && pane() == DesktopPane::Installed { "desktop-game-row selected" } else { "desktop-game-row" },
                                    "aria-pressed": (selected() == Some(game.port_id) && library_page && pane() == DesktopPane::Installed).to_string(),
                                    onclick: { let id = game.port_id; move |_| { selected.set(Some(id)); pane.set(DesktopPane::Installed); navigator.push(Route::LibraryPage { page: "1".into() }); } },
                                    span { class: "desktop-game-icon", "{game.game_name.chars().next().unwrap_or('G')}" }
                                    span { class: "desktop-game-name", "{game.game_name}" }
                                    if game.running { span { class: "desktop-running-dot", "aria-label": "Running" } }
                                }
                            }
                            if !query.is_empty() && !games.iter().any(|game| game.game_name.to_lowercase().contains(&query) || game.port_name.to_lowercase().contains(&query)) {
                                p { class: "desktop-sidebar-message", "No games match your search." }
                            }
                        }
                    }
                    div { class: "desktop-sidebar-bottom",
                        button { r#type: "button", class: "desktop-add-game",
                            onclick: move |_| { pane.set(DesktopPane::Browse); navigator.push(Route::LibraryPage { page: "1".into() }); },
                            span { "＋" }
                            "Add a game"
                        }
                        details { class: "desktop-account",
                            summary {
                                if auth_view.online() { "Account sync connected" } else if auth_view.unlocked() { "Account sync offline" } else { "Local library · Account sync" }
                            }
                            div { class: "desktop-account-content",
                                if auth_view.unlocked() {
                                    button { r#type: "button", class: "{BUTTON} {BUTTON_GHOST} {BUTTON_SM}", onclick: move |_| { saved.retry(); spawn(crate::desktop::account::restore(crate::desktop::runtime(), false)); }, "Refresh account sync" }
                                    button { r#type: "button", class: "{BUTTON} {BUTTON_GHOST} {BUTTON_SM}", onclick: |_| { spawn(crate::desktop::account::sign_out(crate::desktop::runtime())); }, "Sign out" }
                                } else {
                                    p { "Sign in only if you want to sync saved games." }
                                    crate::desktop::auth::ContinueWithGoogle {}
                                }
                                if let Some(message) = auth_view.message.as_deref() {
                                    p { role: "status", class: "text-warning", "{message}" }
                                    if !auth_view.unlocked() {
                                        button { r#type: "button", class: "{BUTTON} {BUTTON_GHOST} {BUTTON_SM}", onclick: |_| { spawn(crate::desktop::account::restore(crate::desktop::runtime(), true)); }, "Retry account sync" }
                                        button { r#type: "button", class: "{BUTTON} {BUTTON_GHOST} {BUTTON_SM}", onclick: |_| { spawn(crate::desktop::account::remove_saved_credential(crate::desktop::runtime())); }, "Retry credential removal" }
                                    }
                                }
                            }
                        }
                    }
                }
                div { class: "desktop-content",
                    main { id: "main", class: if library_page { "desktop-main desktop-library-main" } else { "desktop-main desktop-catalog-main" }, {children} }
                    footer { class: "desktop-statusbar",
                        button { r#type: "button",
                            onclick: move |_| { pane.set(DesktopPane::Updates); navigator.push(Route::LibraryPage { page: "1".into() }); },
                            if *store.checking.read() { "Checking for updates…" } else { "Downloads & updates" }
                        }
                        span { "Local games work without signing in" }
                    }
                }
            }
        }
    }
}

#[component]
fn NativeHistoryControls() -> Element {
    let _route = use_route::<Route>();
    let navigator = use_navigator();
    rsx! {
        button { r#type: "button", class: "rounded-md px-2 py-1 text-sm text-muted-foreground hover:bg-accent disabled:opacity-40", "aria-label": "Back", disabled: !navigator.can_go_back(), onclick: move |_| navigator.go_back(), "‹" }
        button { r#type: "button", class: "rounded-md px-2 py-1 text-sm text-muted-foreground hover:bg-accent disabled:opacity-40", "aria-label": "Forward", disabled: !navigator.can_go_forward(), onclick: move |_| navigator.go_forward(), "›" }
    }
}
