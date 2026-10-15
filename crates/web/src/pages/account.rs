//! The account page: the signed-in viewer's profile, role-dependent links and sign-out.
//! Viewer state loads after hydration, so this page is only a shell in SSR HTML.

use dioxus::prelude::*;
use recomphub_shared::Role;

use crate::components::{Breadcrumb, Chip, EmptyState, Picture, Seo, SignedIn, ViewerStore};
use crate::routes::Route;

const LINK: &str = "rounded-xl border border-border bg-surface-2 px-4 py-2 text-sm font-semibold hover:border-accent";

#[component]
pub fn AccountPage() -> Element {
    rsx! {
        Seo {
            title: "Account | RecompHub".to_string(),
            description: "Your RecompHub account: profile, role and the moderation tools open to you.".to_string(),
            path: "/account".to_string(),
            noindex: true,
        }
        div { class: "mx-auto max-w-3xl px-4 py-10",
            Breadcrumb { items: vec![
                ("Home".to_string(), "/".to_string()),
                ("Account".to_string(), "/account".to_string()),
            ] }
            h1 { class: "mt-4 text-3xl font-extrabold tracking-tight", "Account" }
            SignedIn {
                prompt: "Sign in with GitHub to see your account.".to_string(),
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
    let display_name = user.name.clone().unwrap_or_else(|| user.login.clone());
    let avatar_initials = crate::components::initials_of(&display_name);

    rsx! {
        section { class: "mt-8 flex items-center gap-5 rounded-2xl border border-border bg-surface p-6",
            Picture {
                media: user.avatar.clone(),
                alt: format!("{}'s avatar", user.login),
                initials: avatar_initials,
                sizes: "72px",
                class: "h-18 w-18 rounded-full object-cover",
            }
            div { class: "min-w-0",
                p { class: "truncate text-xl font-bold", "{display_name}" }
                p { class: "truncate text-sm text-muted", "@{user.login}" }
                div { class: "mt-2",
                    Chip { "{role_label}" }
                }
            }
        }
        section { class: "mt-8 grid gap-3 sm:grid-cols-2",
            Link { class: LINK, to: Route::AuthorPage { login: user.login.to_lowercase() }, "Your ports" }
            a { class: LINK, href: "/submit", "Submit a port" }
            if can_moderate {
                Link { class: LINK, to: Route::ModerationQueue {}, "Moderation queue" }
                Link { class: LINK, to: Route::ModerationLog { page: "1".to_string() }, "Moderation log" }
            }
            if is_admin {
                Link { class: LINK, to: Route::AdminPage {}, "Admin" }
            }
        }
        section { class: "mt-10 rounded-2xl border border-border bg-surface p-6",
            h2 { class: "text-lg font-bold", "Session" }
            p { class: "mt-1 text-sm text-muted", "Signing out ends this session on this device." }
            form { method: "post", action: "/auth/logout", class: "mt-4",
                button { r#type: "submit", class: "rounded-xl border border-border bg-surface-2 px-4 py-2 text-sm font-semibold hover:border-accent",
                    "Sign out"
                }
            }
        }
    }
}
