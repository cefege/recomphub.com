//! Hydration-safe private game library state and controls.

use std::collections::HashSet;

use dioxus::prelude::*;
use recomphub_shared::dto::GameFavoriteState;
use serde::{Deserialize, Serialize};

use crate::api;
use crate::components::styles::{BUTTON, BUTTON_GHOST, BUTTON_ICON};
use crate::components::viewer::{ViewerStore, retry_identity};

const STORAGE_VERSION: u8 = 1;
const MERGE_CHUNK: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct DeviceFavorites {
    version: u8,
    game_ids: Vec<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageState {
    Loading,
    Ready,
    Unavailable,
    Invalid,
}

#[derive(Clone, Copy)]
pub struct LibraryStore {
    pub ids: Signal<Vec<i64>>,
    pub initialized: Signal<bool>,
    pub account_mode: Signal<bool>,
    pub pending: Signal<HashSet<i64>>,
    pub error: Signal<Option<String>>,
    pub warning: Signal<Option<String>>,
    pub storage: Signal<StorageState>,
    generation: Signal<u64>,
    retry: Signal<u64>,
}

impl LibraryStore {
    /// Provides one library state tree. It waits for the session check, so no personalized ids
    /// render in SSR output or before the viewer has a confirmed identity.
    pub fn provide(viewer: ViewerStore) -> Self {
        let mut store = Self {
            ids: use_signal(Vec::new),
            initialized: use_signal(|| false),
            account_mode: use_signal(|| false),
            pending: use_signal(HashSet::new),
            error: use_signal(|| None),
            warning: use_signal(|| None),
            storage: use_signal(|| StorageState::Loading),
            generation: use_signal(|| 0),
            retry: use_signal(|| 0),
        };
        use_context_provider(|| store);

        use_effect(move || {
            if !*viewer.ready.read() {
                return;
            }
            let user_id = viewer.me.read().as_ref().map(|user| user.id);
            let _retry = *store.retry.read();
            let previous_generation = *store.generation.peek();
            let generation = previous_generation.wrapping_add(1);
            store.generation.set(generation);
            store.initialized.set(false);
            store.account_mode.set(user_id.is_some());
            store.pending.set(HashSet::new());
            store.error.set(None);
            store.warning.set(None);
            store.storage.set(StorageState::Loading);
            store.ids.set(Vec::new());
            spawn(initialize(store, user_id, generation));
        });

        use_effect(move || {
            let mut eval = document::eval(
                r#"
const key = "recomphub.game-favorites.v1";
const handler = (event) => {
    if (event.key === key && event.storageArea === window.localStorage) dioxus.send(event.newValue);
};
window.__recomphubFavoriteStorageHandler = handler;
window.addEventListener("storage", handler);
await new Promise(() => {});
"#,
            );
            spawn(async move {
                while let Ok(raw) = eval.recv::<Option<String>>().await {
                    if *store.account_mode.peek() {
                        continue;
                    }
                    match decode_device(raw.as_deref()) {
                        Ok(ids) => store.ids.set(ids),
                        Err(_) => {
                            store.storage.set(StorageState::Invalid);
                            store.warning.set(Some(
                                "Device favorites could not be read. Reset them before changing device favorites."
                                    .to_string(),
                            ));
                        }
                    }
                }
            });
        });
        use_drop(|| {
            let _ = document::eval(
                r#"
const handler = window.__recomphubFavoriteStorageHandler;
if (handler) window.removeEventListener("storage", handler);
delete window.__recomphubFavoriteStorageHandler;
"#,
            );
        });
        store
    }

    pub fn contains(&self, game_id: i64) -> bool {
        self.ids.read().contains(&game_id)
    }

    pub fn controls_ready(&self) -> bool {
        *self.initialized.read()
            && self.error.read().is_none()
            && (*self.account_mode.read() || *self.storage.read() != StorageState::Invalid)
    }

    pub fn retry(&self) {
        let next = self.retry.peek().wrapping_add(1);
        let mut retry = self.retry;
        retry.set(next);
    }
}

/// Small, non-nested control for saving the card/header game to the private library.
#[component]
pub fn FavoriteButton(game_id: i64, game_name: String) -> Element {
    let store = use_context::<LibraryStore>();
    let saved = store.contains(game_id);
    let pending = store.pending.read().contains(&game_id);
    let ready = store.controls_ready();
    let label = if !*store.initialized.read() {
        "Loading saved games".to_string()
    } else if saved {
        format!("Remove {game_name} from your library")
    } else {
        format!("Save {game_name} to your library")
    };
    let text = if !*store.initialized.read() {
        "Loading"
    } else if saved {
        "Saved game"
    } else {
        "Save game"
    };
    rsx! {
        button {
            r#type: "button",
            class: "{BUTTON} {BUTTON_GHOST} {BUTTON_ICON} h-11 w-11 rounded-full border border-transparent focus-visible:border-ring",
            title: "{text}",
            "aria-label": "{label}",
            "aria-pressed": "{saved}",
            disabled: !ready || pending,
            onclick: move |_| {
                spawn(change_favorite(store, game_id, !saved));
            },
            span { class: "text-lg leading-none", "aria-hidden": "true", if saved { "♥" } else { "♡" } }
            span { class: "sr-only", "{text}" }
        }
    }
}

/// Retry an identity lookup rather than treating a failed lookup as a guest session.
#[component]
pub fn LibraryIdentityRetry() -> Element {
    let viewer = use_context::<ViewerStore>();
    let message = viewer.error.read().clone();
    let Some(message) = message else {
        return rsx! {};
    };
    rsx! {
        div { class: "rounded-lg border border-warning/40 bg-warning/10 p-4 text-sm",
            p { "{message}" }
            button {
                r#type: "button",
                class: "{BUTTON} {BUTTON_GHOST} mt-3 h-9 border border-border px-3 text-sm",
                onclick: move |_| {
                    retry_identity(viewer);
                },
                "Retry session check"
            }
        }
    }
}

/// Removes guest ids only after the public catalog batch has confirmed they no longer exist.
///
/// Each storage operation can yield to an identity change. A prune that started for a guest must
/// never publish guest state after sign-in has initialized the account library.
pub async fn prune_missing_guest(mut store: LibraryStore, requested: Vec<i64>, present: Vec<i64>) {
    let generation = *store.generation.peek();
    if !current_guest(store, generation) {
        return;
    }
    let missing: Vec<i64> = requested
        .into_iter()
        .filter(|id| !present.contains(id))
        .collect();
    if missing.is_empty() {
        return;
    }
    let Ok(mut current) = read_device().await else {
        return;
    };
    if !current_guest(store, generation) {
        return;
    }
    current.retain(|id| !missing.contains(id));
    if persist_device(&current).await.is_ok() && current_guest(store, generation) {
        store.ids.set(current);
    }
}
pub async fn reset_device_favorites(mut store: LibraryStore) {
    match clear_device().await {
        Ok(()) => {
            store.ids.set(Vec::new());
            store.storage.set(StorageState::Ready);
            store.warning.set(None);
        }
        Err(_) => store.warning.set(Some(
            "Could not reset device favorites because browser storage is unavailable.".to_string(),
        )),
    }
}

async fn initialize(mut store: LibraryStore, user_id: Option<i64>, generation: u64) {
    let (guest_ids, storage_state, warning) = match read_device().await {
        Ok(ids) => (ids, StorageState::Ready, None),
        Err(DeviceReadError::Unavailable) => (
            Vec::new(),
            StorageState::Unavailable,
            Some("Favorites will not be saved on this device because browser storage is unavailable.".to_string()),
        ),
        Err(DeviceReadError::Invalid) => (
            Vec::new(),
            StorageState::Invalid,
            Some("Device favorites are malformed or from an unknown version and were left unchanged. Reset them to start over.".to_string()),
        ),
    };
    if stale(store, generation) {
        return;
    }
    store.storage.set(storage_state);
    store.warning.set(warning);

    if user_id.is_none() {
        store.ids.set(guest_ids);
        store.initialized.set(true);
        return;
    }

    let account_before = match api::favorite_game_ids().await {
        Ok(ids) => ids,
        Err(_) => {
            sync_failed(store, generation);
            return;
        }
    };
    if stale(store, generation) {
        return;
    }
    store.ids.set(account_before);

    if storage_state != StorageState::Invalid {
        for chunk in guest_ids.chunks(MERGE_CHUNK) {
            let acknowledged = match api::merge_game_favorites(chunk.to_vec()).await {
                Ok(result) => {
                    let mut ids = result.accepted_game_ids;
                    ids.extend(result.missing_game_ids);
                    ids
                }
                Err(_) => {
                    sync_failed(store, generation);
                    return;
                }
            };
            if stale(store, generation) {
                return;
            }
            if let Err(error) = acknowledge_device(&acknowledged).await {
                // The server acknowledged the import; retain device data for a safe idempotent retry.
                store.warning.set(Some(error));
            }
        }
    }

    match api::favorite_game_ids().await {
        Ok(ids) if !stale(store, generation) => {
            store.ids.set(ids);
            store.initialized.set(true);
        }
        _ => sync_failed(store, generation),
    }
}

async fn change_favorite(mut store: LibraryStore, game_id: i64, favorite: bool) {
    if !store.controls_ready() || store.pending.read().contains(&game_id) {
        return;
    }
    let generation = *store.generation.peek();
    store.pending.write().insert(game_id);
    if *store.account_mode.read() {
        match api::set_game_favorite(game_id, favorite).await {
            Ok(GameFavoriteState { favorite, .. }) if !stale(store, generation) => {
                let mut ids = store.ids.read().clone();
                apply_favorite(&mut ids, game_id, favorite);
                store.ids.set(ids);
            }
            Err(error) if !stale(store, generation) => {
                let message = match error {
                    ServerFnError::ServerError { code: 401, .. } => {
                        "Sign in again before changing your saved games.".to_string()
                    }
                    ServerFnError::ServerError { code: 403, .. } => {
                        "This account cannot change saved games.".to_string()
                    }
                    _ => "Could not save this game. Try again.".to_string(),
                };
                store.error.set(Some(message));
            }
            _ => {}
        }
    } else if !stale(store, generation) {
        let mut ids = match read_device().await {
            Ok(ids) => ids,
            Err(DeviceReadError::Unavailable) => store.ids.read().clone(),
            Err(DeviceReadError::Invalid) => {
                store.storage.set(StorageState::Invalid);
                store.warning.set(Some(
                    "Device favorites are malformed or from an unknown version and were left unchanged. Reset them to start over.".to_string(),
                ));
                remove_pending(store, game_id);
                return;
            }
        };
        apply_favorite(&mut ids, game_id, favorite);
        store.ids.set(ids.clone());
        if persist_device(&ids).await.is_err() {
            store.storage.set(StorageState::Unavailable);
            store.warning.set(Some(
                "Favorites will not be saved on this device because browser storage is unavailable.".to_string(),
            ));
        } else {
            store.storage.set(StorageState::Ready);
        }
    }
    if !stale(store, generation) {
        remove_pending(store, game_id);
    }
}

fn remove_pending(mut store: LibraryStore, game_id: i64) {
    store.pending.write().remove(&game_id);
}

fn stale(store: LibraryStore, generation: u64) -> bool {
    !generation_matches(generation, *store.generation.peek())
}

fn current_guest(store: LibraryStore, generation: u64) -> bool {
    guest_generation_matches(
        generation,
        *store.generation.peek(),
        *store.account_mode.peek(),
    )
}

fn guest_generation_matches(
    result_generation: u64,
    current_generation: u64,
    account_mode: bool,
) -> bool {
    generation_matches(result_generation, current_generation) && !account_mode
}

fn generation_matches(result_generation: u64, current_generation: u64) -> bool {
    result_generation == current_generation
}

fn sync_failed(mut store: LibraryStore, generation: u64) {
    if !stale(store, generation) {
        store
            .error
            .set(Some("Could not sync favorites. Try again.".to_string()));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeviceReadError {
    Unavailable,
    Invalid,
}

async fn read_device() -> Result<Vec<i64>, DeviceReadError> {
    let raw = document::eval(
        r#"
try {
    return localStorage.getItem("recomphub.game-favorites.v1");
} catch (_) {
    throw new Error("storage unavailable");
}
"#,
    )
    .join::<Option<String>>()
    .await
    .map_err(|_| DeviceReadError::Unavailable)?;
    decode_device(raw.as_deref()).map_err(|_| DeviceReadError::Invalid)
}

async fn persist_device(ids: &[i64]) -> Result<(), ()> {
    let value = serde_json::to_string(&DeviceFavorites {
        version: STORAGE_VERSION,
        game_ids: ids.to_vec(),
    })
    .map_err(|_| ())?;
    let eval = document::eval(
        r#"
const value = await dioxus.recv();
try {
    localStorage.setItem("recomphub.game-favorites.v1", value);
    return true;
} catch (_) {
    return false;
}
"#,
    );
    eval.send(value).map_err(|_| ())?;
    eval.join::<bool>()
        .await
        .map_err(|_| ())
        .and_then(|ok| ok.then_some(()).ok_or(()))
}

async fn clear_device() -> Result<(), ()> {
    document::eval(
        r#"
try {
    localStorage.removeItem("recomphub.game-favorites.v1");
    return true;
} catch (_) {
    return false;
}
"#,
    )
    .join::<bool>()
    .await
    .map_err(|_| ())
    .and_then(|ok| ok.then_some(()).ok_or(()))
}

async fn acknowledge_device(acknowledged: &[i64]) -> Result<(), String> {
    let current = read_device().await.map_err(|_| {
        "Favorites were imported, but device storage could not be updated. A retry is safe."
            .to_string()
    })?;
    persist_device(&remove_acknowledged(current, acknowledged))
        .await
        .map_err(|_| {
            "Favorites were imported, but device storage could not be updated. A retry is safe."
                .to_string()
        })
}

fn remove_acknowledged(mut current: Vec<i64>, acknowledged: &[i64]) -> Vec<i64> {
    current.retain(|id| !acknowledged.contains(id));
    current
}

fn decode_device(raw: Option<&str>) -> Result<Vec<i64>, ()> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let decoded: DeviceFavorites = serde_json::from_str(raw).map_err(|_| ())?;
    if decoded.version != STORAGE_VERSION || decoded.game_ids.iter().any(|id| *id <= 0) {
        return Err(());
    }
    Ok(ordered_unique(decoded.game_ids))
}

fn ordered_unique(ids: Vec<i64>) -> Vec<i64> {
    let mut unique = Vec::with_capacity(ids.len());
    for id in ids {
        if id > 0 && !unique.contains(&id) {
            unique.push(id);
        }
    }
    unique
}

fn apply_favorite(ids: &mut Vec<i64>, game_id: i64, favorite: bool) {
    ids.retain(|id| *id != game_id);
    if favorite {
        ids.insert(0, game_id);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DeviceFavorites, STORAGE_VERSION, apply_favorite, decode_device, generation_matches,
        guest_generation_matches, ordered_unique, remove_acknowledged,
    };

    #[test]
    fn device_state_preserves_order_and_rejects_unknown_shapes() {
        assert_eq!(ordered_unique(vec![3, 1, 3, 2]), [3, 1, 2]);
        let mut ids = vec![3, 1, 2];
        apply_favorite(&mut ids, 1, false);
        apply_favorite(&mut ids, 1, true);
        assert_eq!(ids, [1, 3, 2]);
        let raw = serde_json::to_string(&DeviceFavorites {
            version: STORAGE_VERSION,
            game_ids: vec![4, 4, 2],
        })
        .unwrap();
        assert_eq!(decode_device(Some(&raw)).unwrap(), [4, 2]);
        assert!(decode_device(Some("not-json")).is_err());
        assert!(decode_device(Some(r#"{"version":2,"game_ids":[1]}"#)).is_err());
    }

    #[test]
    fn merge_acknowledgement_keeps_newer_device_changes_and_stale_results_fail_closed() {
        // 9 was added in another tab after the original [4, 2] import snapshot.
        assert_eq!(remove_acknowledged(vec![9, 4, 2], &[4, 2]), [9]);
        assert_eq!(remove_acknowledged(vec![9, 4, 2], &[4]), [9, 2]);
        assert!(generation_matches(7, 7));
        assert!(!generation_matches(7, 8));
    }

    #[test]
    fn delayed_guest_prune_cannot_publish_after_identity_change() {
        let prune_generation = 12;
        assert!(guest_generation_matches(prune_generation, 12, false));

        // A sign-in while storage is awaiting increments the generation and switches mode.
        assert!(!guest_generation_matches(prune_generation, 13, true));

        // Either signal alone also keeps a stale prune from publishing guest ids.
        assert!(!guest_generation_matches(prune_generation, 13, false));
        assert!(!guest_generation_matches(prune_generation, 12, true));
    }
}
