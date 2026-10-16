//! The account page: the signed-in viewer's profile, role-dependent links and sign-out.
//! Viewer state loads after hydration, so this page is only a shell in SSR HTML.

use dioxus::prelude::*;
use recomphub_shared::Role;

use crate::components::styles::*;
use crate::components::{Breadcrumb, Chip, EmptyState, Picture, Seo, SignedIn, ViewerStore};
use crate::routes::Route;

#[component]
pub fn AccountPage() -> Element {
    rsx! {
        Seo {
            title: "Account | RecompHub".to_string(),
            description: "Your RecompHub account: profile, role and the moderation tools open to you.".to_string(),
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
