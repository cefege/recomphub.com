//! Native device snapshots remain independent of catalog availability.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use dioxus::prelude::*;
use recomphub_desktop::{InstalledGame, OperationProgress};
use recomphub_shared::{HostPlatform, InstallOffer, InstallOffersQuery};

use super::install_actions::{InstallActions, NativeAction, dispatch};
use super::runtime;

#[derive(Clone, Copy)]
pub struct NativeLibraryStore {
    pub games: Signal<Vec<InstalledGame>>,
    pub operations: Signal<Vec<OperationProgress>>,
    pub progress: Signal<OperationProgress>,
    pub pending: Signal<HashSet<i64>>,
    pub errors: Signal<HashMap<i64, String>>,
    pub notices: Signal<HashMap<i64, String>>,
    pub offers: Signal<Vec<InstallOffer>>,
    pub host: Signal<Option<HostPlatform>>,
    pub local_error: Signal<Option<String>>,
    pub offers_error: Signal<Option<String>>,
    pub loaded: Signal<bool>,
    pub offers_loaded: Signal<bool>,
    pub checking: Signal<bool>,
    owner: ScopeId,
    refresh: Signal<u64>,
}

impl NativeLibraryStore {
    pub fn provide() -> Self {
        let mut store = Self {
            games: use_signal(Vec::new),
            operations: use_signal(Vec::new),
            progress: use_signal(OperationProgress::default),
            pending: use_signal(HashSet::new),
            errors: use_signal(HashMap::new),
            notices: use_signal(HashMap::new),
            offers: use_signal(Vec::new),
            host: use_signal(|| None),
            local_error: use_signal(|| None),
            offers_error: use_signal(|| None),
            loaded: use_signal(|| false),
            checking: use_signal(|| false),
            refresh: use_signal(|| 0),
            offers_loaded: use_signal(|| false),
            owner: dioxus::core::current_scope_id(),
        };
        use_context_provider(|| store);
        use_future(move || async move {
            let runtime = runtime();
            let manager = match runtime.library().await {
                Ok(manager) => manager,
                Err(error) => {
                    store.local_error.set(Some(error));
                    store.loaded.set(true);
                    return;
                }
            };
            let mut receiver = manager.subscribe_progress();
            let host = tokio::task::spawn_blocking(recomphub_desktop::detect_host_platform).await;
            match host {
                Ok(Ok(host)) => store.host.set(Some(host)),
                Ok(Err(error)) => store.offers_error.set(Some(error.to_string())),
                Err(_) => store.offers_error.set(Some(
                    "Native machine detection stopped unexpectedly.".into(),
                )),
            }
            loop {
                let snapshot_manager = manager.clone();
                match tokio::task::spawn_blocking(move || {
                    Ok::<_, recomphub_desktop::DesktopError>((snapshot_manager.installed()?, snapshot_manager.operations()?))
                }).await {
                    Ok(Ok((games, operations))) => {
                        store.games.set(games);
                        store.operations.set(operations);
                        store.local_error.set(None);
                    }
                    Ok(Err(error)) => store.local_error.set(Some(error.to_string())),
                    Err(_) => store.local_error.set(Some("The device snapshot worker stopped unexpectedly. Your last snapshot is retained.".into())),
                }
                store.progress.set(receiver.borrow_and_update().clone());
                if !*store.loaded.peek() {
                    store.loaded.set(true);
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
        use_effect(move || {
            let loaded = *store.loaded.read();
            let host = store.host.read().clone();
            let _refresh = *store.refresh.read();
            if !loaded || host.is_none() {
                return;
            }
            spawn(async move {
                refresh_offers(store).await;
            });
        });
        use_future(move || async move {
            let mut timer = tokio::time::interval(Duration::from_secs(6 * 60 * 60));
            timer.tick().await;
            loop {
                timer.tick().await;
                refresh_offers(store).await;
            }
        });
        store
    }

    pub fn request_refresh(&mut self) {
        let next = self.refresh.peek().wrapping_add(1);
        self.refresh.set(next);
    }

    pub fn spawn_action(&self, future: impl Future<Output = ()> + 'static) {
        dioxus::core::Runtime::current().spawn(self.owner, future);
    }
}

async fn fetch_offers(
    host: &HostPlatform,
    port_id: Option<i64>,
) -> Result<Vec<InstallOffer>, String> {
    let mut offers = Vec::new();
    let mut page = 1;
    let mut expected = None;
    loop {
        let response = crate::api::install_offers(InstallOffersQuery { host: host.clone(), port_id, page })
            .await.map_err(|error| format!("Could not fetch reviewed offers: {error}. Existing installs and update candidates remain unchanged."))?;
        if response.page != page || response.pages < 1 || response.total < 0 {
            return Err("RecompHub returned invalid offer pagination.".into());
        }
        if expected.is_some_and(|count| count != (response.pages, response.total)) {
            return Err("The offer list changed during the update check. Retry to obtain a complete snapshot.".into());
        }
        expected = Some((response.pages, response.total));
        for offer in response.items {
            offer.validate().map_err(|error| error.to_string())?;
            if !offer
                .manifest
                .matches_host(host)
                .map_err(|error| error.to_string())?
                || port_id.is_some_and(|id| offer.port.id != id)
            {
                return Err(
                    "The offer response did not match the requested machine or project.".into(),
                );
            }
            offers.push(offer);
        }
        if page >= response.pages {
            if offers.len() as i64 != response.total {
                return Err(
                    "The offer response was incomplete. Previous update candidates are retained."
                        .into(),
                );
            }
            return Ok(offers);
        }
        page += 1;
    }
}

async fn refresh_offers(mut store: NativeLibraryStore) {
    if *store.checking.peek() {
        return;
    }
    store.checking.set(true);
    let result = async {
        let host = store
            .host
            .peek()
            .clone()
            .ok_or_else(|| "This machine could not be identified safely.".to_string())?;
        let browse_offers = fetch_offers(&host, None).await?;
        let ids: Vec<_> = store.games.peek().iter().map(|game| game.port_id).collect();
        let mut updates = Vec::new();
        for id in ids {
            updates.extend(fetch_offers(&host, Some(id)).await?);
        }
        let runtime = runtime();
        let manager = runtime.library().await?;
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            handle
                .block_on(manager.check_updates(updates))
                .map_err(|error| error.to_string())
        })
        .await
        .map_err(|_| "The update-check worker stopped unexpectedly.".to_string())??;
        store.offers.set(browse_offers);
        store.offers_loaded.set(true);
        Ok::<_, String>(())
    }
    .await;
    store.offers_error.set(result.err());
    store.checking.set(false);
}

#[component]
pub fn NativeLibrary(page: String) -> Element {
    let mut store = use_context::<NativeLibraryStore>();
    let mut pane = use_context::<Signal<super::workspace::DesktopPane>>();
    let mut selected = use_context::<Signal<Option<i64>>>();
    let mut browse_page = use_signal(|| 1usize);
    let mut compatible_only = use_signal(|| false);
    let offers = store.offers.read();
    let browse_pages = offers.len().div_ceil(50).max(1);
    let current_page = browse_page().min(browse_pages);
    let games = store.games.read();
    let selected_game = selected().and_then(|id| games.iter().find(|game| game.port_id == id));
    let failed_ports: Vec<_> = store
        .operations
        .read()
        .iter()
        .filter(|operation| {
            operation.stage != "committed"
                && operation
                    .port_id
                    .is_some_and(|id| !games.iter().any(|game| game.port_id == id))
        })
        .cloned()
        .collect();
    rsx! {
        div { class: "desktop-library-content",
            if pane() != super::workspace::DesktopPane::Installed {
                header { class: "desktop-page-header",
                    h1 {
                        match pane() {
                            super::workspace::DesktopPane::Saved => "Saved games",
                            super::workspace::DesktopPane::Browse => "Add a game",
                            super::workspace::DesktopPane::Updates => "Downloads & updates",
                            super::workspace::DesktopPane::Installed => "Library",
                        }
                    }
                    div { class: "desktop-page-tools",
                        button { r#type: "button", disabled: *store.checking.read(), onclick: move |_| store.request_refresh(), if *store.checking.read() { "Checking…" } else { "Check for updates" } }
                    }
                }
            }
            if let Some(message) = store.local_error.read().as_deref() {
                div { role: "alert", class: "rounded-lg border border-destructive/50 bg-destructive/10 p-3 text-sm",
                    p { "{message}" }
                    button { r#type: "button", class: "mt-2 underline", onclick: move |_| store.request_refresh(), "Retry local snapshot" }
                }
            }
            if let Some(message) = store.offers_error.read().as_deref() {
                details { class: "desktop-catalog-notice",
                    summary { "Catalog connection unavailable · Installed games stay available" }
                    p { role: "status", "{message}" }
                    button { r#type: "button", class: "mt-2 underline", onclick: move |_| store.request_refresh(), "Retry catalog connection" }
                }
            }
            match pane() {
                super::workspace::DesktopPane::Installed => rsx! {
                    if !*store.loaded.read() {
                        div { class: "rounded-xl border border-border bg-card p-8 text-sm text-muted-foreground", "Loading local installations…" }
                    } else if games.is_empty() && failed_ports.is_empty() {
                        div { class: "desktop-empty-library",
                            h1 { "Your game library" }
                            p { "Add a game to your library. Select it in the sidebar to install, set up and play." }
                            button { r#type: "button", class: "native-primary-action", onclick: move |_| pane.set(super::workspace::DesktopPane::Browse), "Add a game" }
                        }
                    } else if let Some(game) = selected_game {
                        article { class: "desktop-game-page",
                            div { class: "desktop-game-hero",
                                crate::components::Picture {
                                    media: offers.iter().find(|offer| offer.port.id == game.port_id).and_then(|offer| offer.game.cover.clone()),
                                    alt: format!("{} artwork", game.game_name),
                                    initials: crate::components::initials_of(&game.game_name),
                                    sizes: "(min-width: 900px) calc(100vw - 248px), calc(100vw - 210px)",
                                    eager: true,
                                    class: "desktop-hero-art",
                                }
                                div { class: "desktop-hero-shade" }
                                div { class: "desktop-hero-title",
                                    p { "{game.port_name}" }
                                    h1 { "{game.game_name}" }
                                }
                            }
                            div { class: "desktop-game-launch",
                                InstallActions { port_id: game.port_id }
                            }
                            section { class: "desktop-game-overview",
                                div { class: "desktop-game-overview-header",
                                    h2 { "Game information" }
                                    crate::components::FavoriteButton {
                                        game_id: game.game_id,
                                        game_name: game.game_name.clone(),
                                    }
                                }
                                p { "Installed on this device. Your saves and original game files stay separate from the game installation." }
                                details { class: "desktop-install-details",
                                    summary { "Installation details" }
                                    if let Some(manifest) = game.manifest.as_ref() {
                                        p { "Version {manifest.version} · Build {manifest.recipe.id}" }
                                    }
                                    p { class: "break-all", "{runtime().root.display()}" }
                                    button { r#type: "button", class: "mt-3 underline",
                                        onclick: move |_| { spawn(async move { if let Err(error) = dispatch(NativeAction::OpenLibrary).await { store.local_error.set(Some(error)); } }); },
                                        "Open library folder"
                                    }
                                }
                            }
                        }
                    } else {
                        div { class: "grid gap-5 lg:grid-cols-[minmax(0,1.35fr)_minmax(240px,0.65fr)]",
                            div { class: "rounded-xl border border-border bg-card p-6",
                                h2 { class: "text-xl font-semibold", "Choose an installed game" }
                                p { class: "mt-2 text-sm text-muted-foreground", "Select a game in the sidebar to view its local status and actions." }
                                div { class: "mt-5 grid gap-2 sm:grid-cols-2",
                                    for game in games.iter() {
                                        button { r#type: "button", class: "min-w-0 rounded-lg border border-border p-4 text-left hover:bg-accent", onclick: { let id = game.port_id; move |_| selected.set(Some(id)) },
                                            span { class: "block truncate font-semibold", "{game.game_name}" }
                                            span { class: "mt-1 block truncate text-sm text-muted-foreground", "{game.port_name}" }
                                        }
                                    }
                                }
                            }
                            div { class: "rounded-xl border border-border bg-card p-5",
                                h2 { class: "font-semibold", "Recent operations" }
                                if store.operations.read().is_empty() { p { class: "mt-2 text-sm text-muted-foreground", "No active or failed operations." } }
                                for operation in store.operations.read().iter().filter(|operation| operation.stage != "committed") {
                                    div { class: "mt-3 rounded-md bg-muted/60 p-3",
                                        p { class: "text-sm font-medium", "{operation.stage}" }
                                        if let Some(message) = operation.message.as_deref() { p { class: "mt-1 break-words text-xs text-muted-foreground", "{message}" } }
                                    }
                                }
                            }
                        }
                    }
                    for operation in failed_ports.iter() {
                        if let Some(port_id) = operation.port_id {
                            article { class: "rounded-lg border border-destructive/40 bg-card p-4",
                                h2 { class: "font-semibold", "Installation for project {port_id}: {operation.stage}" }
                                if let Some(message) = operation.message.as_deref() { p { class: "mt-1 text-sm text-destructive", "{message}" } }
                                InstallActions { port_id }
                            }
                        }
                    }
                },
                super::workspace::DesktopPane::Saved => rsx! {
                    label { class: "flex items-center gap-2 text-sm text-muted-foreground",
                        input { r#type: "checkbox", checked: compatible_only(), disabled: !*store.offers_loaded.read(), onchange: move |event| compatible_only.set(event.checked()) }
                        "Show only saved games with a managed installer for this machine"
                    }
                    crate::pages::library::SavedLibraryContent { page, compatible_ids: compatible_only().then(|| offers.iter().map(|offer| offer.game.id).collect::<Vec<_>>()) }
                },
                super::workspace::DesktopPane::Updates => rsx! {
                    if !*store.offers_loaded.read() { p { class: "text-sm text-muted-foreground", "Update candidates appear after a complete managed-installer check." } }
                    else if games.iter().all(|game| game.update_available.is_none() || game.update_suppressed) { p { class: "rounded-lg border border-border bg-card p-5 text-sm text-muted-foreground", "No automatic update notices from the last successful check. Installed games keep their manual Update and Roll back controls." } }
                    for game in games.iter().filter(|game| game.update_available.is_some() && !game.update_suppressed) {
                        article { class: "rounded-lg border border-border bg-card p-5",
                            h2 { class: "text-lg font-semibold", "{game.game_name}" }
                            p { class: "text-sm text-muted-foreground", "{game.port_name}" }
                            InstallActions { port_id: game.port_id }
                        }
                    }
                },
                super::workspace::DesktopPane::Browse => rsx! {
                    if !*store.offers_loaded.read() { p { class: "rounded-lg border border-border bg-card p-5 text-sm text-muted-foreground", "Managed installers have not been checked successfully yet." } }
                    else if offers.is_empty() { p { class: "rounded-lg border border-border bg-card p-5 text-sm text-muted-foreground", "No managed installer for this machine" } }
                    else {
                        for offer in offers.iter().skip((current_page - 1) * 50).take(50) {
                            article { key: "{offer.manifest.recipe.id}", class: "rounded-lg border border-border bg-card p-5",
                                div { class: "flex flex-wrap items-start justify-between gap-3",
                                    div { class: "min-w-0",
                                        h2 { class: "text-xl font-semibold", "{offer.game.name}" }
                                        p { class: "mt-1 text-sm text-muted-foreground", "{offer.port.name}" }
                                    }
                                    span { class: "rounded-full border border-border px-3 py-1 text-xs", "{offer.manifest.version} · {offer.manifest.size_bytes / 1_048_576} MiB" }
                                }
                                dioxus::prelude::Link { to: format!("/games/{}", offer.game.slug), class: "mt-3 inline-block text-sm text-primary underline-offset-4 hover:underline", "Project details" }
                                InstallActions { port_id: offer.port.id }
                            }
                        }
                        div { class: "flex items-center justify-center gap-4 text-sm",
                            button { r#type: "button", class: "rounded-md border px-3 py-2 disabled:opacity-40", disabled: current_page <= 1, onclick: move |_| browse_page.set(current_page.saturating_sub(1).max(1)), "Previous" }
                            span { class: "tabular text-muted-foreground", "Page {current_page} of {browse_pages}" }
                            button { r#type: "button", class: "rounded-md border px-3 py-2 disabled:opacity-40", disabled: current_page >= browse_pages, onclick: move |_| browse_page.set(current_page + 1), "Next" }
                        }
                    }
                    dioxus::prelude::Link { to: "/games", class: "block text-sm text-primary underline-offset-4 hover:underline", "Browse all projects" }
                },
            }
        }
    }
}
