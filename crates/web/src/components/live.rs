use std::collections::{HashMap, VecDeque};

use dioxus::fullstack::{WebSocketOptions, use_websocket};
use dioxus::prelude::*;
use recomphub_shared::{ActivityItem, ClientMsg, LiveGame, ServerMsg};

use crate::components::keyed;
use crate::components::styles::{BADGE, BADGE_SECONDARY};

/// What the current page wants the socket to report: its game (if any) and the cards on screen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Desire {
    pub watching: Option<i64>,
    pub visible: Vec<i64>,
}

/// Client-side live state, filled by the websocket session after hydration.
/// SSR renders from the snapshot embedded at render time; live updates refine it.
#[derive(Clone, Copy)]
pub struct LiveStore {
    pub online: Signal<u32>,
    pub counts: Signal<HashMap<i64, u32>>,
    pub top: Signal<Vec<LiveGame>>,
    pub activity: Signal<VecDeque<ActivityItem>>,
    pub ranking_version: Signal<u64>,
    pub desire: Signal<Desire>,
    pub generation: Signal<u32>,
    pub attempt: Signal<u32>,
}

impl LiveStore {
    /// Create the store and provide it to the subtree. Call once in the app shell.
    pub fn provide() -> Self {
        let store = Self {
            online: use_signal(|| 0u32),
            counts: use_signal(HashMap::new),
            top: use_signal(Vec::new),
            activity: use_signal(VecDeque::new),
            ranking_version: use_signal(|| 0u64),
            desire: use_signal(Desire::default),
            generation: use_signal(|| 0u32),
            attempt: use_signal(|| 0u32),
        };
        use_context_provider(|| store);
        store
    }
}

/// Declare what this page watches. Sends a fresh `Hello` to the socket when the values change.
/// `watching` is the page's game; it is always included in `visible`.
pub fn use_live_watch(watching: Option<i64>, visible: Vec<i64>) {
    let store = use_context::<LiveStore>();
    use_effect(use_reactive!(|(watching, visible)| {
        let mut visible: Vec<i64> = visible.into_iter().take(200).collect();
        if let Some(w) = watching
            && !visible.contains(&w)
        {
            visible.push(w);
        }
        let next = Desire { watching, visible };
        let mut desire = store.desire;
        if *desire.peek() != next {
            desire.set(next);
        }
    }));
}

/// Mounts the websocket session. Render only after hydration, so the socket never runs during SSR.
#[component]
pub fn LiveSocket() -> Element {
    let store = use_context::<LiveStore>();
    let epoch = *store.generation.read();
    keyed(rsx! { LiveSession { key: "{epoch}", epoch } })
}

/// One websocket connection. A new `epoch` remounts it, which reconnects.
#[component]
fn LiveSession(epoch: u32) -> Element {
    let _ = epoch;
    let store = use_context::<LiveStore>();
    let socket = use_websocket(|| crate::api::live(WebSocketOptions::new()));

    // Send Hello on mount and whenever the page's desired watch state changes.
    use_effect(move || {
        let desire = store.desire.read().clone();
        let msg = ClientMsg::Hello {
            watching: desire.watching,
            visible: desire.visible,
        };
        spawn(async move {
            let _ = socket.send(msg).await;
        });
    });

    // Keep-alive below Cloudflare's 100 s idle timeout. Dropped with this component.
    use_effect(move || {
        spawn(async move {
            loop {
                sleep_ms(25_000).await;
                if socket.send(ClientMsg::Ping).await.is_err() {
                    break;
                }
            }
        });
    });

    // Receive loop. On error, back off (1, 2, 5, 10 s) and remount with a new epoch.
    use_effect(move || {
        spawn(async move {
            let mut store = store;
            let mut socket = socket;
            loop {
                match socket.recv().await {
                    Ok(msg) => {
                        if *store.attempt.peek() != 0 {
                            store.attempt.set(0);
                        }
                        apply(store, msg);
                    }
                    Err(_) => {
                        let attempt = *store.attempt.peek();
                        let delays = [1_000u64, 2_000, 5_000, 10_000];
                        let delay = delays[(attempt as usize).min(delays.len() - 1)];
                        store.attempt.set(attempt + 1);
                        sleep_ms(delay).await;
                        let next = *store.generation.peek() + 1;
                        store.generation.set(next);
                        break;
                    }
                }
            }
        });
    });

    rsx! {}
}

fn apply(mut store: LiveStore, msg: ServerMsg) {
    match msg {
        ServerMsg::Presence {
            online,
            counts,
            top,
            ..
        } => {
            store.online.set(online);
            store
                .counts
                .set(counts.into_iter().collect::<HashMap<i64, u32>>());
            store.top.set(top);
        }
        ServerMsg::Activity(item) => {
            let mut queue = store.activity.write();
            queue.push_front(*item);
            queue.truncate(30);
        }
        ServerMsg::RankingUpdated => {
            let next = *store.ranking_version.peek() + 1;
            store.ranking_version.set(next);
        }
    }
}

#[cfg(target_arch = "wasm32")]
async fn sleep_ms(ms: u64) {
    gloo_timers::future::sleep(std::time::Duration::from_millis(ms)).await;
}

#[cfg(not(target_arch = "wasm32"))]
async fn sleep_ms(ms: u64) {
    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
}

/// Live viewer count for one game: "{n} here now", or "{n} waiting right now" when the game has no port.
/// Hidden when the count is zero.
#[component]
pub fn LiveCount(game_id: i64, #[props(default)] portless: bool) -> Element {
    let store = use_context::<LiveStore>();
    let n = store.counts.read().get(&game_id).copied().unwrap_or(0);
    if n == 0 {
        return rsx! {};
    }
    let text = if portless {
        format!("{n} waiting right now")
    } else {
        format!("{n} here now")
    };
    rsx! {
        span { class: "{BADGE} overflow-visible! tabular text-live",
            span { class: "relative flex h-2 w-2",
                span { class: "absolute inline-flex h-full w-full rounded-full bg-live live-ping" }
                span { class: "relative inline-flex h-2 w-2 rounded-full bg-live" }
            }
            // Keyed on the value so the bump animation replays on change.
            {keyed(rsx! { span { key: "{n}", class: "animate-bump", "{text}" } })}
        }
    }
}

/// Global online total for the header pill. Always shown, including zero.
#[component]
pub fn OnlineCount() -> Element {
    let store = use_context::<LiveStore>();
    let n = *store.online.read();
    rsx! {
        span { class: "{BADGE} {BADGE_SECONDARY} overflow-visible! tabular",
            span { class: "relative flex h-2 w-2",
                span { class: "absolute inline-flex h-full w-full rounded-full bg-live live-ping" }
                span { class: "relative inline-flex h-2 w-2 rounded-full bg-live" }
            }
            {keyed(rsx! { span { key: "{n}", class: "animate-bump", "{n} online" } })}
        }
    }
}
