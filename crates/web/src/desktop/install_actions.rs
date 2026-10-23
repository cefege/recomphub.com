//! One native action boundary shared by catalog and device-local entries.

use std::path::PathBuf;

use crate::components::styles::{BUTTON, BUTTON_MD, BUTTON_OUTLINE, BUTTON_PRIMARY, BUTTON_SM};
use dioxus::prelude::*;
use recomphub_desktop::{DataStatus, InstalledGame};
use recomphub_shared::InstallOffer;

use super::library::NativeLibraryStore;
use super::runtime;

#[derive(Clone, PartialEq)]
pub enum NativeAction {
    Install(InstallOffer),
    Update(i64, InstallOffer),
    Repair(i64),
    Rollback(i64),
    Uninstall(i64),
    Play(i64),
    Setup(i64),
    Import(i64, PathBuf),
    Replace(i64, PathBuf),
    Cancel(i64),
    OpenSaves(i64),
    OpenLibrary,
}

/// Device-local actions and public managed packages do not require an account.
pub async fn dispatch(action: NativeAction) -> Result<Option<String>, String> {
    let runtime = runtime();
    let manager = runtime.library().await?;
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let result: Result<Option<String>, recomphub_desktop::DesktopError> = handle.block_on(async {
            match action {
                NativeAction::Install(offer) => manager.install(offer).await.map(|_| None),
                NativeAction::Update(id, offer) => manager.update(id, offer).await.map(|_| None),
                NativeAction::Repair(id) => manager.repair(id).await.map(|_| None),
                NativeAction::Rollback(id) => manager.rollback(id).await.map(|_| None),
                NativeAction::Uninstall(id) => manager.uninstall(id).await.map(|_| None),
                NativeAction::Play(id) => manager.launch(id).await.map(|_| None),
                NativeAction::Setup(id) => manager.launch_setup(id).await.map(|_| None),
                NativeAction::Import(id, path) => manager.provide_original_data(id, path).await.map(|_| None),
                NativeAction::Replace(id, path) => manager.replace_original_data(id, path).await
                    .map(|backup| Some(format!("Original file backup retained at {}", backup.display()))),
                NativeAction::Cancel(id) => manager.cancel(id).map(|cancelled| {
                    Some(if cancelled { "Cancellation requested.".into() } else { "This operation has reached a noncancellable commit or has already finished.".into() })
                }),
                NativeAction::OpenSaves(id) => {
                    let path = manager.saves_directory(id)?;
                    if !path.is_dir() {
                        return Ok(Some("The game has not created its saves folder yet.".into()));
                    }
                    open_folder(&path).map(|_| None)
                }
                NativeAction::OpenLibrary => open_folder(manager.root()).map(|_| None),
            }
        });
        result.map_err(|error| error.to_string())
    }).await.map_err(|_| "The native library worker stopped unexpectedly.".to_string())?
}

fn open_folder(path: &std::path::Path) -> Result<(), recomphub_desktop::DesktopError> {
    #[cfg(target_os = "macos")]
    let status = std::process::Command::new("/usr/bin/open")
        .arg("--")
        .arg(path)
        .status()?;
    #[cfg(windows)]
    let status = std::process::Command::new("explorer.exe")
        .arg(path)
        .status()?;
    #[cfg(not(any(target_os = "macos", windows)))]
    return Err(recomphub_desktop::DesktopError::UnsupportedHost(
        "File-manager reveal is available on macOS and Windows.".into(),
    ));
    #[cfg(any(target_os = "macos", windows))]
    if !status.success() {
        return Err(recomphub_desktop::DesktopError::Io(std::io::Error::other(
            "The system file manager could not open the folder.",
        )));
    }
    #[cfg(any(target_os = "macos", windows))]
    Ok(())
}

fn run(mut store: NativeLibraryStore, port_id: i64, action: NativeAction) {
    if store.pending.peek().contains(&port_id) && !matches!(action, NativeAction::Cancel(_)) {
        return;
    }
    store.pending.write().insert(port_id);
    store.errors.write().remove(&port_id);
    store.spawn_action(async move {
        match dispatch(action).await {
            Ok(Some(message)) => {
                store.notices.write().insert(port_id, message);
            }
            Ok(None) => {
                store.notices.write().remove(&port_id);
            }
            Err(error) => {
                store.errors.write().insert(port_id, error);
            }
        }
        store.pending.write().remove(&port_id);
    });
}

#[component]
fn ActionButton(
    port_id: i64,
    action: NativeAction,
    label: String,
    disabled: bool,
    #[props(default)] primary: bool,
) -> Element {
    let store = use_context::<NativeLibraryStore>();
    let variant = if primary {
        BUTTON_PRIMARY
    } else {
        BUTTON_OUTLINE
    };
    let size = if primary { BUTTON_MD } else { BUTTON_SM };
    let emphasis = if primary {
        "native-primary-action"
    } else {
        "native-secondary-action"
    };
    let play = matches!(&action, NativeAction::Play(_));
    rsx! {
        button {
            r#type: "button", class: "{BUTTON} {variant} {size} {emphasis}", disabled,
            onclick: move |_| run(store, port_id, action.clone()),
            if play { span { "aria-hidden": "true", "▶" } }
            "{label}"
        }
    }
}

fn status_label(game: &InstalledGame) -> &'static str {
    if game.manifest.is_none() {
        return "Metadata damaged";
    }
    if game.running {
        return "Running";
    }
    if matches!(game.status, recomphub_desktop::InstallStatus::RepairNeeded) {
        return "Repair needed";
    }
    match game.data_status {
        DataStatus::NotRequired | DataStatus::Ready => "Ready to play",
        DataStatus::Missing => "Add your original game file to play",
        DataStatus::Conflict => "Original game file conflict",
        DataStatus::Unsupported => "Original-data adapter unsupported",
        DataStatus::Error => "Original game file could not be checked",
    }
}

#[component]
pub fn InstallActions(port_id: i64) -> Element {
    let store = use_context::<NativeLibraryStore>();
    let game = store
        .games
        .read()
        .iter()
        .find(|game| game.port_id == port_id)
        .cloned();
    let offers: Vec<_> = store
        .offers
        .read()
        .iter()
        .filter(|offer| offer.port.id == port_id)
        .cloned()
        .collect();
    let mut recipe = use_signal(String::new);
    let selected = if offers.len() == 1 {
        offers.first().cloned()
    } else {
        offers
            .iter()
            .find(|offer| offer.manifest.recipe.id == recipe())
            .cloned()
    };
    let operation = store
        .operations
        .read()
        .iter()
        .rev()
        .find(|item| item.port_id == Some(port_id))
        .cloned();
    let active = operation
        .as_ref()
        .is_some_and(|item| !matches!(item.stage.as_str(), "failed" | "cancelled" | "committed"));
    let busy = store.pending.read().contains(&port_id) || active;
    let mut uninstall_confirmation = use_signal(|| false);
    let mut selected_file = use_signal(|| None::<PathBuf>);
    let mut destination = use_signal(|| None::<PathBuf>);
    let running = game.as_ref().is_some_and(|game| game.running);
    let data_status = game.as_ref().map(|game| game.data_status);
    let damaged = game.as_ref().is_some_and(|game| game.manifest.is_none());
    let needs_repair = game
        .as_ref()
        .is_some_and(|game| game.status == recomphub_desktop::InstallStatus::RepairNeeded);
    let candidate = selected.clone().filter(|offer| {
        game.as_ref()
            .and_then(|game| game.update_available.as_ref())
            .is_some_and(|manifest| manifest == &offer.manifest)
    });
    let retry_install = operation
        .as_ref()
        .is_some_and(|item| matches!(item.stage.as_str(), "failed" | "cancelled"));
    let transfer = store.progress.read().clone();
    rsx! {
        section { class: "native-actions", "aria-label": "Managed game actions",
            div { class: "native-action-row",
                if game.is_none() || damaged {
                    if let Some(offer) = selected.clone() {
                        ActionButton { port_id, action: NativeAction::Install(offer), primary: true, label: if busy { "Installing…".to_string() } else if damaged { "Reinstall".to_string() } else if retry_install { "Retry install".to_string() } else { "Install".to_string() }, disabled: busy || running }
                    } else if offers.len() > 1 {
                        p { class: "text-sm text-muted-foreground", "Select a reviewed build before installing." }
                    } else if let Some(message) = store.offers_error.read().as_deref() {
                        p { class: "text-sm text-muted-foreground", "Installer availability is not confirmed: {message}" }
                    } else if !*store.offers_loaded.read() {
                        p { class: "text-sm text-muted-foreground", "Checking managed installer availability…" }
                    } else {
                        p { class: "text-sm text-muted-foreground", "No managed installer for this machine" }
                    }
                } else if running {
                    button { r#type: "button", class: "native-primary-action", disabled: true, "Running" }
                } else if needs_repair {
                    ActionButton { port_id, action: NativeAction::Repair(port_id), primary: true, label: "Repair game", disabled: busy }
                } else if matches!(data_status, Some(DataStatus::Ready | DataStatus::NotRequired)) {
                    ActionButton { port_id, action: NativeAction::Play(port_id), primary: true, label: "Play", disabled: busy }
                } else if matches!(data_status, Some(DataStatus::Missing | DataStatus::Conflict)) && selected_file().is_none() {
                    button { r#type: "button", class: "native-primary-action", disabled: busy,
                        onclick: move |_| {
                            spawn(async move {
                                if let Some(file) = rfd::AsyncFileDialog::new().set_title("Choose your legally supplied original game file").pick_file().await {
                                    let manager = runtime().library().await;
                                    match manager {
                                        Ok(manager) => {
                                            let path = tokio::task::spawn_blocking(move || manager.data_destination(port_id)).await;
                                            match path {
                                                Ok(Ok(path)) => { destination.set(path); selected_file.set(Some(file.path().to_path_buf())); }
                                                _ => { let mut store = store; store.errors.write().insert(port_id, "Could not resolve the original-data destination.".into()); }
                                            }
                                        }
                                        Err(error) => { let mut store = store; store.errors.write().insert(port_id, error); }
                                    }
                                }
                            });
                        },
                        "Set up game files"
                    }
                }
                if game.is_some() {
                    details { class: "native-manage",
                        summary { "Manage game" }
                        div { class: "native-manage-actions",
                            if let Some(offer) = candidate {
                                ActionButton { port_id, action: NativeAction::Update(port_id, offer.clone()), label: format!("Update {}", offer.manifest.version), disabled: busy || running }
                            }
                            if !needs_repair {
                                ActionButton { port_id, action: NativeAction::Repair(port_id), label: "Repair", disabled: busy || running }
                            }
                            if game.as_ref().is_some_and(|game| game.previous_path.is_some()) {
                                ActionButton { port_id, action: NativeAction::Rollback(port_id), label: "Roll back", disabled: busy || running }
                            }
                            if data_status == Some(DataStatus::Missing) && !needs_repair {
                                ActionButton { port_id, action: NativeAction::Setup(port_id), label: "Open game setup", disabled: busy || running }
                            }
                            ActionButton { port_id, action: NativeAction::OpenSaves(port_id), label: "Open saves folder", disabled: false }
                            button { r#type: "button", class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM} native-secondary-action", disabled: busy || running,
                                onclick: move |_| uninstall_confirmation.set(true), "Uninstall"
                            }
                        }
                    }
                }
            }
            if let Some(path) = selected_file() {
                div { class: "space-y-2 rounded border p-3 text-sm",
                    p { class: "break-all", "Source: {path.display()}" }
                    if let Some(destination) = destination() { p { class: "break-all", "Destination: {destination.display()}" } }
                    p { "Use only a game file you are legally entitled to supply. RecompHub does not download original game data." }
                    button { r#type: "button", class: "native-primary-action",
                        onclick: move |_| {
                            let action = if data_status == Some(DataStatus::Conflict) { NativeAction::Replace(port_id, path.clone()) } else { NativeAction::Import(port_id, path.clone()) };
                            run(store, port_id, action);
                            selected_file.set(None);
                        },
                        if data_status == Some(DataStatus::Conflict) { "Back up and replace" } else { "Import original game file" }
                    }
                    button { r#type: "button", class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM}", onclick: move |_| selected_file.set(None), "Cancel" }
                }
            }
            if uninstall_confirmation() {
                div { role: "alertdialog", "aria-label": "Confirm uninstall", class: "rounded border p-3 text-sm",
                    p { "Remove managed game packages from this device? Original game data, saves, configuration and favorites are preserved." }
                    button { r#type: "button", class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM}",
                        onclick: move |_| { uninstall_confirmation.set(false); run(store, port_id, NativeAction::Uninstall(port_id)); },
                        "Confirm uninstall"
                    }
                    button { r#type: "button", class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM}", onclick: move |_| uninstall_confirmation.set(false), "Cancel" }
                }
            }
            if let Some(game) = game.as_ref() {
                div { class: "native-game-state",
                    strong { "{status_label(game)}" }
                    if let Some(manifest) = game.manifest.as_ref() { span { "Version {manifest.version}" } }
                }
                if let Some(message) = game.error.as_deref() {
                    p { role: "alert", class: "text-sm text-destructive", "{message}" }
                }
            }
            if let Some(message) = store.errors.read().get(&port_id) {
                p { role: "alert", class: "text-sm text-destructive", "{message}" }
            }
            if let Some(message) = store.notices.read().get(&port_id) {
                p { role: "status", class: "text-sm", "{message}" }
            }
            if transfer.port_id == Some(port_id) && active {
                p { class: "text-xs", "{transfer.stage}: {transfer.received_bytes} bytes received" }
                if let Some(total) = transfer.total_bytes {
                    progress { class: "w-full", value: transfer.received_bytes.to_string(), max: total.to_string() }
                }
                if !transfer.queued_port_ids.is_empty() {
                    p { class: "text-xs text-muted-foreground", "Queued projects: {transfer.queued_port_ids:?}" }
                }
            }
            if let Some(operation) = operation.as_ref().filter(|operation| operation.stage != "committed") {
                p { class: "text-xs", "Operation: {operation.stage}" }
                if let Some(message) = operation.message.as_deref() { p { class: "text-sm", "{message}" } }
                if active && operation.cancellable {
                    ActionButton { port_id, action: NativeAction::Cancel(port_id), label: "Cancel", disabled: false }
                }
            }
            if offers.len() > 1 {
                label { class: "block text-sm", "Choose a reviewed build"
                    select { class: "ml-2 rounded border bg-background p-2", value: recipe(),
                        onchange: move |event| recipe.set(event.value()),
                        option { value: "", "Select a build" }
                        for offer in offers.iter() {
                            option { value: "{offer.manifest.recipe.id}", "{offer.port.name} — {offer.manifest.recipe.id}" }
                        }
                    }
                }
            }
            if let Some(offer) = selected.as_ref() {
                details { class: "native-build-details",
                    summary { class: "cursor-pointer font-medium", "Build and requirements" }
                    p { class: "mt-3", "Available {offer.manifest.version} · {offer.manifest.size_bytes / 1_048_576} MiB · Build matches this machine" }
                    ul { class: "mt-2 list-disc pl-5",
                        for requirement in offer.manifest.recipe.requirements.iter() { li { "{requirement}" } }
                    }
                    p { class: "mt-2", "SHA-256 records upstream integrity, not malware or signature certification." }
                    button { r#type: "button", class: "mt-2 underline",
                        onclick: {
                            let url = offer.manifest.release_url.clone();
                            move |_| {
                                if let Err(message) = super::external::open_external_http(&url) {
                                    let mut store = store;
                                    store.errors.write().insert(port_id, message);
                                }
                            }
                        },
                        "Upstream release and source"
                    }
                }
            }
        }
    }
}
