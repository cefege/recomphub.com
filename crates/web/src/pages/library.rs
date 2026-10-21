//! Private saved-games library. SSR stays anonymous; cards load only after browser state settles.

use dioxus::prelude::*;
use recomphub_shared::GameCard;

use crate::api;
use crate::components::styles::{BUTTON, BUTTON_MD, BUTTON_OUTLINE, H1, LEAD};
use crate::components::{
    CardGrid, EmptyState, GameCardView, LibraryIdentityRetry, LibraryStore, Pager, Seo,
    StorageState, ViewerStore, login_redirect, prune_missing_guest, reset_device_favorites,
};

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
    let store = use_context::<LibraryStore>();
    let viewer = use_context::<ViewerStore>();
    let mut cards = use_signal(CardsLoad::default);
    let mut retry = use_signal(|| 0u64);
    let requested_page = page
        .parse::<usize>()
        .ok()
        .filter(|page| *page > 0)
        .unwrap_or(1);
    let total = store.ids.read().len();
    let pages = total.div_ceil(PAGE_SIZE).max(1);
    let current_page = requested_page.min(pages);
    let ids: Vec<i64> = store
        .ids
        .read()
        .iter()
        .skip((current_page - 1) * PAGE_SIZE)
        .take(PAGE_SIZE)
        .copied()
        .collect();

    use_effect(move || {
        let initialized = *store.initialized.read();
        let _retry = retry();
        if !initialized {
            cards.set(CardsLoad::Idle);
            return;
        }
        if ids.is_empty() {
            cards.set(CardsLoad::Ready(Vec::new()));
            return;
        }
        cards.set(CardsLoad::Loading);
        let request_ids = ids.clone();
        let prune_ids = request_ids.clone();
        spawn(async move {
            match api::game_cards_by_ids(request_ids).await {
                Ok(games) => {
                    let present = games.iter().map(|game| game.id).collect();
                    spawn(prune_missing_guest(store, prune_ids, present));
                    cards.set(CardsLoad::Ready(games));
                }
                Err(_) => cards.set(CardsLoad::Error),
            }
        });
    });

    use_effect(move || {
        if *store.initialized.read() && requested_page != current_page {
            let eval = document::eval(
                r#"
const page = await dioxus.recv();
location.replace("/library?page=" + page);
"#,
            );
            let _ = eval.send(current_page.to_string());
        }
    });

    let mode = if *store.account_mode.read() {
        "Account-synced saved games"
    } else {
        "Device-only saved games"
    };
    let identity_failed = viewer.error.read().is_some();
    rsx! {
        Seo {
            title: "Your library | RecompHub".to_string(),
            description: "Private saved games in your RecompHub library.".to_string(),
            path: "/library".to_string(),
            noindex: true,
        }
        div { class: "mx-auto max-w-7xl",
            header { class: "max-w-2xl",
                h1 { class: H1, "Your library" }
                p { class: "mt-2 {LEAD}", "{mode}" }
                if !*store.account_mode.read() && !identity_failed {
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
                if let Some(warning) = store.warning.read().clone() {
                    div { class: "mt-6 flex flex-wrap items-center justify-between gap-3 rounded-lg border border-warning/40 bg-warning/10 p-4 text-sm",
                        p { "{warning}" }
                        if *store.storage.read() == StorageState::Invalid && !*store.account_mode.read() {
                            button {
                                r#type: "button",
                                class: "{BUTTON} {BUTTON_OUTLINE} h-9 px-3 text-sm",
                                onclick: move |_| {
                                    spawn(reset_device_favorites(store));
                                },
                                "Reset device favorites"
                            }
                        }
                    }
                }
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
                    CardsLoad::Ready(games) if games.is_empty() && total == 0 => rsx! {
                        div { class: "mt-8",
                            EmptyState { text: "No saved games yet. Browse the catalog to save one.".to_string() }
                            div { class: "mt-4 flex justify-center",
                                a { href: "/games", class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD}", "Browse games" }
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
