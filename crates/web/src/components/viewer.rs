use std::collections::HashSet;

use dioxus::prelude::*;
use recomphub_shared::{Role, SessionUser};

use super::ui::{EmptyState, LoginPrompt};
use crate::api;

/// Signed-in state, loaded after hydration so viewer markup never enters the SSR HTML.
#[derive(Clone, Copy)]
pub struct ViewerStore {
    pub me: Signal<Option<SessionUser>>,
    pub ready: Signal<bool>,
    pub starred: Signal<HashSet<i64>>,
    pub wanted: Signal<HashSet<i64>>,
}

impl ViewerStore {
    /// Create the store and provide it to the subtree. Call once in the app shell.
    pub fn provide() -> Self {
        let store = Self {
            me: use_signal(|| None),
            ready: use_signal(|| false),
            starred: use_signal(HashSet::new),
            wanted: use_signal(HashSet::new),
        };
        use_context_provider(|| store);
        store
    }

    pub fn logged_in(&self) -> bool {
        self.me.read().is_some()
    }

    /// The signed-in viewer's role, if any.
    pub fn role(&self) -> Option<Role> {
        self.me
            .read()
            .as_ref()
            .and_then(|user| user.role.parse::<Role>().ok())
    }

    /// True for moderators and admins.
    pub fn can_moderate(&self) -> bool {
        matches!(self.role(), Some(Role::Moderator | Role::Admin))
    }
}

/// Send the visitor through GitHub sign-in, returning to the current page.
pub fn login_redirect() {
    let _ = document::eval(
        "location.href='/auth/github/login?next='+encodeURIComponent(location.pathname+location.search)",
    );
}

/// Load the viewer's stars and wants for the given ids once the viewer is known.
/// Ids are batched per page, so one request covers every card on the page.
pub fn use_marks(port_ids: Vec<i64>, game_ids: Vec<i64>) {
    let viewer = use_context::<ViewerStore>();
    use_effect(use_reactive!(|(port_ids, game_ids)| {
        if !viewer.logged_in() {
            return;
        }
        if port_ids.is_empty() && game_ids.is_empty() {
            return;
        }
        let mut starred = viewer.starred;
        let mut wanted = viewer.wanted;
        spawn(async move {
            if let Ok(marks) = api::my_marks(port_ids, game_ids).await {
                starred.write().extend(marks.starred);
                wanted.write().extend(marks.wanted);
            }
        });
    }));
}

/// Stars a port. Counts update locally, then the server confirms the new total.
#[component]
pub fn StarButton(port_id: i64, count: i64) -> Element {
    let viewer = use_context::<ViewerStore>();
    let mut total = use_signal(|| count);
    let mut burst = use_signal(|| 0u32);
    let mut error = use_signal(|| None::<String>);
    let starred = viewer.starred.read().contains(&port_id);
    let label = if starred { "Starred" } else { "Star" };
    let classes = if starred {
        "border-warning/60 bg-warning/15 text-warning"
    } else {
        "border-border bg-surface-2 text-foreground hover:border-accent"
    };
    rsx! {
        span { class: "relative inline-flex flex-col items-start gap-1",
            button {
                r#type: "button",
                class: "inline-flex items-center gap-1.5 rounded-full border px-3 py-1 text-sm font-semibold transition {classes}",
                "aria-pressed": "{starred}",
                onclick: move |_| {
                    if !viewer.logged_in() {
                        login_redirect();
                        return;
                    }
                    let next = *burst.peek() + 1;
                    burst.set(next);
                    spawn(async move {
                        match api::toggle_star(port_id).await {
                            Ok(state) => {
                                let mut starred_set = viewer.starred;
                                if state.starred {
                                    starred_set.write().insert(port_id);
                                } else {
                                    starred_set.write().remove(&port_id);
                                }
                                total.set(state.count);
                                error.set(None);
                            }
                            Err(e) => error.set(Some(e.to_string())),
                        }
                    });
                },
                span { "★" }
                span { class: "tabular", "{total}" }
                span { class: "sr-only", "{label}" }
            }
            if burst() > 0 {
                Burst { key: "{burst}", seed: burst() }
            }
            if let Some(msg) = error() {
                span { class: "text-xs text-error", "{msg}" }
            }
        }
    }
}

/// Wants a game that has no port yet.
#[component]
pub fn WantButton(game_id: i64, count: i64) -> Element {
    let viewer = use_context::<ViewerStore>();
    let mut total = use_signal(|| count);
    let mut burst = use_signal(|| 0u32);
    let mut error = use_signal(|| None::<String>);
    let wanted = viewer.wanted.read().contains(&game_id);
    let classes = if wanted {
        "border-accent/60 bg-accent/15 text-foreground"
    } else {
        "accent-bg text-white hover:brightness-110"
    };
    let label = if wanted { "Wanted" } else { "Want a port" };
    rsx! {
        span { class: "relative inline-flex flex-col items-start gap-1",
            button {
                r#type: "button",
                class: "inline-flex items-center gap-2 rounded-full px-4 py-2 text-sm font-semibold transition {classes}",
                "aria-pressed": "{wanted}",
                onclick: move |_| {
                    if !viewer.logged_in() {
                        login_redirect();
                        return;
                    }
                    let next = *burst.peek() + 1;
                    burst.set(next);
                    spawn(async move {
                        match api::toggle_want(game_id).await {
                            Ok(state) => {
                                let mut wanted_set = viewer.wanted;
                                if state.wanted {
                                    wanted_set.write().insert(game_id);
                                } else {
                                    wanted_set.write().remove(&game_id);
                                }
                                total.set(state.count);
                                error.set(None);
                            }
                            Err(e) => error.set(Some(e.to_string())),
                        }
                    });
                },
                span { "{label}" }
                span { class: "tabular rounded-full bg-black/20 px-2 py-0.5 text-xs", "{total}" }
            }
            if burst() > 0 {
                Burst { key: "{burst}", seed: burst() }
            }
            if let Some(msg) = error() {
                span { class: "text-xs text-error", "{msg}" }
            }
        }
    }
}

/// Twelve confetti spans that fly out once and fade. Positions come from a small seeded generator.
#[component]
pub fn Burst(seed: u32) -> Element {
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    let spans: Vec<(String, String)> = (0..12)
        .map(|i| {
            let angle = (i as f64) * 30.0 + f64::from(next() % 12);
            let radians = angle.to_radians();
            let dist = 36.0 + f64::from(next() % 28);
            let dx = radians.cos() * dist;
            let dy = radians.sin() * dist;
            let hue = match next() % 3 {
                0 => "bg-accent",
                1 => "bg-live",
                _ => "bg-warning",
            };
            (format!("--dx:{dx:.1}px;--dy:{dy:.1}px;"), hue.to_string())
        })
        .collect();
    rsx! {
        span { class: "pointer-events-none absolute left-1/2 top-1/2 z-10 h-0 w-0",
            for (style, hue) in spans {
                span {
                    class: "absolute left-0 top-0 h-1.5 w-1.5 -translate-x-1/2 -translate-y-1/2 rounded-full {hue}",
                    style: "{style} animation: burst 0.7s ease-out forwards;",
                }
            }
        }
    }
}

/// Applies a fetched session user to the viewer store.
pub fn set_me(viewer: ViewerStore, me: Option<SessionUser>) {
    let mut signal = viewer.me;
    signal.set(me);
    let mut ready = viewer.ready;
    ready.set(true);
}

/// Renders its children for a signed-in viewer and a sign-in prompt otherwise.
/// Viewer-specific pages use this, so no viewer data reaches the SSR HTML.
#[component]
pub fn SignedIn(prompt: String, children: Element) -> Element {
    let viewer = use_context::<ViewerStore>();
    if !*viewer.ready.read() {
        return rsx! { div { class: "skeleton h-96 rounded-2xl", "aria-hidden": "true" } };
    }
    if !viewer.logged_in() {
        return rsx! { LoginPrompt { text: prompt } };
    }
    children
}

/// Renders its children when the signed-in viewer's role is allowed. `admin_only` pages refuse
/// moderators. The server enforces the same rule on every API call.
#[component]
pub fn RoleGate(admin_only: bool, children: Element) -> Element {
    let viewer = use_context::<ViewerStore>();
    if !*viewer.ready.read() {
        return rsx! { div { class: "skeleton h-96 rounded-2xl", "aria-hidden": "true" } };
    }
    if !viewer.logged_in() {
        return rsx! {
            LoginPrompt { text: "Sign in with GitHub to continue.".to_string() }
        };
    }
    let role = viewer
        .me
        .read()
        .as_ref()
        .and_then(|u| u.role.parse::<Role>().ok());
    let allowed = match role {
        Some(Role::Admin) => true,
        Some(Role::Moderator) => !admin_only,
        _ => false,
    };
    if allowed {
        children
    } else {
        rsx! { EmptyState { text: "You do not have access to this page.".to_string() } }
    }
}
