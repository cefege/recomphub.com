//! Private saved-games library. SSR stays anonymous; cards load only after browser state settles.

use dioxus::prelude::*;
use recomphub_shared::GameCard;

use crate::api;
use crate::components::styles::{BUTTON, BUTTON_MD, BUTTON_OUTLINE, H1, LEAD};
use crate::components::{
    CardGrid, EmptyState, GameCardView, LibraryIdentityRetry, LibraryStore, Pager, Seo,
    login_redirect,
};
#[cfg(not(feature = "desktop"))]
use crate::components::{StorageState, ViewerStore, prune_missing_guest, reset_device_favorites};

const PAGE_SIZE: usize = 50;

#[derive(Clone, Default)]
enum CardsLoad {
    #[default]
    Idle,
    Loading,
    Ready(Vec<GameCard>),
    Error,
}

#[component]
pub fn LibraryPage(page: String) -> Element {
    #[cfg(feature = "desktop")]
    return rsx! { crate::desktop::library::NativeLibrary { page } };
    #[cfg(not(feature = "desktop"))]
    rsx! { SavedLibraryContent { page } }
}

#[component]
pub fn SavedLibraryContent(
    page: String,
    #[props(default)] compatible_ids: Option<Vec<i64>>,
) -> Element {
    let store = use_context::<LibraryStore>();
    #[cfg(not(feature = "desktop"))]
    let viewer = use_context::<ViewerStore>();
    let mut cards = use_signal(CardsLoad::default);
    let mut retry = use_signal(|| 0u64);
    let mut generation = use_signal(|| 0u64);
    let mut card_error = use_signal(|| None::<String>);
    let mut requested_ids = use_signal(Vec::<i64>::new);
    let navigator = use_navigator();
    let requested_page = page
        .parse::<usize>()
        .ok()
        .filter(|page| *page > 0)
        .unwrap_or(1);
    let all_ids = store.ids.read();
    let unfiltered_total = all_ids.len();
    let filtered: Vec<i64> = all_ids
        .iter()
        .copied()
        .filter(|id| {
            compatible_ids
                .as_ref()
                .is_none_or(|compatible| compatible.contains(id))
        })
        .collect();
    let total = filtered.len();
    let pages = total.div_ceil(PAGE_SIZE).max(1);
    let current_page = requested_page.min(pages);
    let ids: Vec<i64> = filtered
        .into_iter()
        .skip((current_page - 1) * PAGE_SIZE)
        .take(PAGE_SIZE)
        .collect();

    use_effect(use_reactive((&ids,), move |(ids,)| {
        let initialized = *store.initialized.read();
        let _retry = retry();
        let next_generation = generation.peek().wrapping_add(1);
        generation.set(next_generation);
        if *requested_ids.peek() != ids {
            requested_ids.set(ids.clone());
            cards.set(CardsLoad::Idle);
            card_error.set(None);
        }
        if !initialized {
            #[cfg(not(feature = "desktop"))]
            cards.set(CardsLoad::Idle);
            return;
        }
        card_error.set(None);
        if ids.is_empty() {
            cards.set(CardsLoad::Ready(Vec::new()));
            return;
        }
        if !matches!(*cards.peek(), CardsLoad::Ready(_)) {
            cards.set(CardsLoad::Loading);
        }
        let request_ids = ids.clone();
        #[cfg(not(feature = "desktop"))]
        let prune_ids = request_ids.clone();
        spawn(async move {
            match api::game_cards_by_ids(request_ids).await {
                _ if *generation.peek() != next_generation => {}
                Ok(games) => {
                    #[cfg(not(feature = "desktop"))]
                    {
                        let present = games.iter().map(|game| game.id).collect();
                        spawn(prune_missing_guest(store, prune_ids, present));
                    }
                    cards.set(CardsLoad::Ready(games));
                }
                Err(_) => {
                    card_error.set(Some(
                        "Could not refresh saved catalog cards. Last confirmed cards are retained."
                            .into(),
                    ));
                    if !matches!(*cards.peek(), CardsLoad::Ready(_)) {
                        cards.set(CardsLoad::Error);
                    }
                }
            }
        });
    }));

    use_effect(use_reactive(
        (&requested_page, &current_page),
        move |(requested_page, current_page)| {
            if *store.initialized.read() && requested_page != current_page {
                navigator.replace(format!("/library?page={current_page}"));
            }
        },
    ));

    let mode = if *store.account_mode.read() {
        "Account-synced saved games"
    } else {
        "Device-only saved games"
    };
    #[cfg(not(feature = "desktop"))]
    let identity_failed = viewer.error.read().is_some();
    #[cfg(feature = "desktop")]
    let identity_failed = false;
    #[cfg(not(feature = "desktop"))]
    let device_warning = rsx! {
        if let Some(warning) = store.warning.read().clone() {
            div { class: "mt-6 flex flex-wrap items-center justify-between gap-3 rounded-lg border border-warning/40 bg-warning/10 p-4 text-sm",
                p { "{warning}" }
                if *store.storage.read() == StorageState::Invalid && !*store.account_mode.read() {
                    button {
                        r#type: "button",
                        class: "{BUTTON} {BUTTON_OUTLINE} h-9 px-3 text-sm",
                        onclick: move |_| { spawn(reset_device_favorites(store)); },
                        "Reset device favorites"
                    }
                }
            }
        }
    };
    #[cfg(feature = "desktop")]
    let device_warning = rsx! {
        if let Some(warning) = store.warning.read().clone() {
            div { class: "mt-6 flex flex-wrap items-center justify-between gap-3 rounded-lg border border-warning/40 bg-warning/10 p-4 text-sm",
                p { "{warning}" }
                if *store.account_mode.read() {
                    button {
                        r#type: "button",
                        class: "{BUTTON} {BUTTON_OUTLINE} h-9 px-3 text-sm",
                        onclick: move |_| { store.retry(); },
                        "Retry sync"
                    }
                }
            }
        }
    };
    rsx! {
        Seo {
            title: "Your library | RecompHub".to_string(),
            description: "Private saved games in your RecompHub library.".to_string(),
            path: "/library".to_string(),
            noindex: true,
        }
        div { class: "mx-auto max-w-7xl",
            header { class: "max-w-2xl",
                h1 { class: H1, if cfg!(feature = "desktop") { "Saved games" } else { "Your library" } }
                p { class: "mt-2 {LEAD}", "{mode}" }
                if !cfg!(feature = "desktop") && !*store.account_mode.read() && !identity_failed {
                    p { class: "mt-3 text-sm text-muted-foreground",
                        "Save games on this device, then sign in to add them to your account without replacing existing saved games."
                    }
                    a {
                        href: "/auth/google",
                        class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD} mt-4",
                        onclick: move |event| {
                            event.prevent_default();
                            login_redirect();
                        },
                        "Sign in with Google"
                    }
                }
            }
            if identity_failed {
                div { class: "mt-6", LibraryIdentityRetry {} }
            } else if !*store.initialized.read() {
                div { class: "mt-8 grid grid-cols-2 gap-4 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-6", "aria-label": "Loading saved games",
                    for index in 0..6 {
                        div { key: "{index}", class: "skeleton aspect-[3/4] rounded-lg" }
                    }
                }
            } else {
                {device_warning}
                if let Some(error) = store.error.read().clone() {
                    div { class: "mt-6 rounded-lg border border-destructive/40 bg-destructive/10 p-4 text-sm",
                        p { "{error}" }
                        button {
                            r#type: "button",
                            class: "{BUTTON} {BUTTON_OUTLINE} mt-3 h-9 px-3 text-sm",
                            onclick: move |_| {
                                store.retry();
                            },
                            "Retry sync"
                        }
                    }
                }
                if let Some(message) = card_error() {
                    p { role: "status", class: "mt-4 text-sm text-muted-foreground", "{message}" }
                }
                match cards() {
                    CardsLoad::Idle | CardsLoad::Loading => rsx! {
                        div { class: "mt-8 grid grid-cols-2 gap-4 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-6", "aria-label": "Loading saved games",
                            for index in 0..6 {
                                div { key: "{index}", class: "skeleton aspect-[3/4] rounded-lg" }
                            }
                        }
                    },
                    CardsLoad::Error => rsx! {
                        div { class: "mt-8",
                            EmptyState { text: "Could not load saved games.".to_string() }
                            div { class: "mt-4 flex justify-center",
                                button {
                                    r#type: "button",
                                    class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD}",
                                    onclick: move |_| {
                                        let next = retry().wrapping_add(1);
                                        retry.set(next);
                                    },
                                    "Retry"
                                }
                            }
                        }
                    },
                    CardsLoad::Ready(games) if games.is_empty() && total == 0 && unfiltered_total > 0 => rsx! {
                        EmptyState { text: "No saved games match the compatible-only filter. Your saved games are unchanged.".to_string() }
                    },
                    CardsLoad::Ready(games) if games.is_empty() && total == 0 && unfiltered_total == 0 => rsx! {
                        div { class: "mt-8",
                            EmptyState { text: "No saved games yet. Browse the catalog to save one.".to_string() }
                            div { class: "mt-4 flex justify-center",
                                Link { to: "/games", class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD}", "Browse games" }
                            }
                        }
                    },
                    CardsLoad::Ready(games) if games.is_empty() => rsx! {
                        div { class: "mt-8",
                            EmptyState { text: "Those saved games are no longer available in the catalog.".to_string() }
                        }
                    },
                    CardsLoad::Ready(games) => rsx! {
                        CardGrid {
                            for game in games {
                                li { key: "{game.id}", GameCardView { game } }
                            }
                        }
                    },
                }
                Pager { base: "/library".to_string(), page: current_page as i64, pages: pages as i64 }
            }
        }
    }
}
