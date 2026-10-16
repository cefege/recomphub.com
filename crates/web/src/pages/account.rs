//! The account page: the signed-in viewer's profile, connected sign-in accounts,
//! role-dependent links and sign-out.
//! Viewer state loads after hydration, so this page is only a shell in SSR HTML.

use dioxus::prelude::*;
use recomphub_shared::Role;
use recomphub_shared::dto::{AuthProviders, ConnectedAccount, provider_label};

use crate::api;
use crate::components::styles::*;
use crate::components::{
    Breadcrumb, Chip, EmptyState, Picture, Seo, SignedIn, ViewerStore, set_me,
};
use crate::routes::Route;

/// Sign-in providers in the order the account page lists them.
const PROVIDERS: [&str; 2] = ["github", "google"];

#[component]
pub fn AccountPage() -> Element {
    rsx! {
        Seo {
            title: "Account | RecompHub".to_string(),
            description: "Your RecompHub account: profile, connected sign-in accounts, role and the moderation tools open to you.".to_string(),
            path: "/account".to_string(),
            noindex: true,
        }
        div { class: "mx-auto max-w-3xl",
            header {
                Breadcrumb { items: vec![
                    ("Home".to_string(), "/".to_string()),
                    ("Account".to_string(), "/account".to_string()),
                ] }
                h1 { class: "{H1} mt-4", "Account" }
            }
            SignedIn {
                prompt: "Sign in to see your account.".to_string(),
                AccountDetails {}
            }
        }
    }
}

#[component]
fn AccountDetails() -> Element {
    let viewer = use_context::<ViewerStore>();
    let Some(user) = viewer.me.read().clone() else {
        return rsx! { EmptyState { text: "Your session has ended. Sign in again to continue.".to_string() } };
    };
    let role = user.role.parse::<Role>().ok();
    let role_label = match role {
        Some(Role::Admin) => "Admin",
        Some(Role::Moderator) => "Moderator",
        _ => "Member",
    };
    let is_admin = matches!(role, Some(Role::Admin));
    let can_moderate = viewer.can_moderate();
    let display_name = user.display_name.clone();
    let avatar_initials = crate::components::initials_of(&display_name);

    rsx! {
        div { class: "mt-8 space-y-12",
            section { class: "{CARD_SURFACE} flex items-center gap-4 p-6",
                Picture {
                    media: user.avatar.clone(),
                    alt: format!("{display_name}'s avatar"),
                    initials: avatar_initials,
                    sizes: "72px",
                    class: "h-18 w-18 rounded-full object-cover",
                }
                div { class: "min-w-0",
                    p { class: "truncate text-lg font-semibold", "{display_name}" }
                    if let Some(login) = user.github_login.clone() {
                        p { class: "truncate text-sm text-muted-foreground", "@{login}" }
                    }
                    div { class: "mt-2",
                        Chip { "{role_label}" }
                    }
                }
            }
            section { class: "grid gap-4 sm:grid-cols-2",
                if let Some(login) = user.github_login.clone() {
                    Link {
                        class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD}",
                        to: Route::AuthorPage { login: login.to_lowercase() },
                        "Your ports"
                    }
                }
                a { class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD}", href: "/submit", "Submit a port" }
                if can_moderate {
                    Link {
                        class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD}",
                        to: Route::ModerationQueue {},
                        "Moderation queue"
                    }
                    Link {
                        class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD}",
                        to: Route::ModerationLog { page: "1".to_string() },
                        "Moderation log"
                    }
                }
                if is_admin {
                    Link {
                        class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD}",
                        to: Route::AdminPage {},
                        "Admin"
                    }
                }
            }
            ConnectedAccounts {}
            section { class: "{CARD_SURFACE} p-6",
                h2 { class: "text-base font-semibold", "Session" }
                p { class: "mt-1 text-sm text-muted-foreground", "Signing out ends this session on this device." }
                form { method: "post", action: "/auth/logout", class: "mt-4",
                    button {
                        r#type: "submit",
                        class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD}",
                        "Sign out"
                    }
                }
            }
        }
    }
}

/// Whether the server has this provider configured, so the page only offers real connects.
fn offered(configured: Option<AuthProviders>, provider: &str) -> bool {
    match (configured, provider) {
        (Some(c), "github") => c.github,
        (Some(c), "google") => c.google,
        _ => false,
    }
}

/// Sign-in accounts linked to this RecompHub account.
///
/// Accounts are never merged by email address. An address can be recycled, or its domain
/// can change hands, so merging on it would let whoever controls the address take over
/// the member account. Members link each provider explicitly while signed in instead.
#[component]
fn ConnectedAccounts() -> Element {
    let mut providers = use_signal(|| None::<AuthProviders>);
    let mut connections = use_signal(|| None::<Result<Vec<ConnectedAccount>, String>>);
    let mut version = use_signal(|| 0u32);
    let mut notice = use_signal(|| None::<String>);
    let viewer = use_context::<ViewerStore>();

    use_effect(move || {
        spawn(async move {
            if let Ok(configured) = api::auth_providers().await {
                providers.set(Some(configured));
            }
        });
    });
    use_effect(move || {
        let _ = version();
        spawn(async move {
            let result = api::connected_accounts().await.map_err(|e| e.to_string());
            connections.set(Some(result));
        });
    });

    let mut unlink = move |provider: String| {
        notice.set(None);
        spawn(async move {
            match api::unlink_account(provider).await {
                Ok(()) => {
                    version.set(version() + 1);
                    // Disconnecting GitHub clears `github_login` on the server. Refresh the
                    // viewer so the handle and "Your ports" link go away without a reload.
                    set_me(viewer, api::me().await.ok().flatten());
                }
                Err(e) => notice.set(Some(e.to_string())),
            }
        });
    };

    let notice_text = notice();
    let configured = providers();
    let state = connections.read().clone();
    let body = match state {
        None => rsx! {
            div { class: "skeleton h-24 rounded-xl", "aria-hidden": "true" }
        },
        Some(Err(message)) => rsx! {
            p { class: "text-sm text-destructive", "Could not load connected accounts: {message}" }
        },
        Some(Ok(rows)) => {
            let only_one = rows.len() <= 1;
            let connected: Vec<String> = rows.iter().map(|r| r.provider.clone()).collect();
            let missing: Vec<&'static str> = PROVIDERS
                .iter()
                .copied()
                .filter(|p| !connected.iter().any(|c| c.as_str() == *p))
                .filter(|p| offered(configured, p))
                .collect();
            rsx! {
                if !rows.is_empty() {
                    ul { class: "divide-y",
                        for row in rows {
                            ConnectionRow {
                                key: "{row.provider}",
                                row,
                                only_one,
                                on_disconnect: move |provider: String| unlink(provider),
                            }
                        }
                    }
                }
                if only_one {
                    p { class: "mt-2 text-xs text-muted-foreground",
                        "Connect another account before disconnecting this one. You need at least one to sign in."
                    }
                }
                if !missing.is_empty() {
                    div { class: "mt-4 flex flex-wrap gap-2",
                        for key in missing {
                            ConnectLink { key: "{key}", provider: key }
                        }
                    }
                }
            }
        }
    };

    rsx! {
        section { id: "connected-accounts", class: "{CARD_SURFACE} p-6",
            h2 { class: "text-base font-semibold", "Connected accounts" }
            p { class: "mt-1 text-sm text-muted-foreground",
                "Each connected account can sign you in to this RecompHub account. Accounts are never merged by email address, so connect only accounts you control."
            }
            if let Some(text) = notice_text {
                p { class: "mt-4 rounded-lg border p-4 text-sm text-destructive", "{text}" }
            }
            div { class: "mt-4", {body} }
        }
    }
}

#[component]
fn ConnectionRow(
    row: ConnectedAccount,
    only_one: bool,
    on_disconnect: EventHandler<String>,
) -> Element {
    let provider = row.provider.clone();
    let label = provider_label(&row.provider);
    rsx! {
        li { class: "flex flex-wrap items-center justify-between gap-3 py-3",
            div { class: "min-w-0",
                p { class: "text-sm font-medium", "{label}" }
                if let Some(handle) = row.handle.clone() {
                    p { class: "truncate text-xs text-muted-foreground", "{handle}" }
                }
            }
            button {
                r#type: "button",
                class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM}",
                disabled: only_one,
                onclick: move |_| on_disconnect.call(provider.clone()),
                "Disconnect"
            }
        }
    }
}

#[component]
fn ConnectLink(provider: &'static str) -> Element {
    let label = provider_label(provider);
    rsx! {
        a {
            class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD}",
            href: "/auth/{provider}/login?link=1&next=/account",
            "Connect {label}"
        }
    }
}
