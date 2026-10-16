//! `/sign-in`: the member sign-in chooser. Offers each configured provider; the buttons are
//! plain links to `/auth/{provider}`, so they work before hydration.

use dioxus::prelude::*;

use crate::api;
use crate::components::icons::{GithubIcon, GoogleIcon};
use crate::components::styles::*;
use crate::components::{Seo, url_encode};

#[component]
pub fn SignInPage(next: String) -> Element {
    let methods = use_loader(api::sign_in_methods)?;
    let methods = methods();
    let next = if next.starts_with('/') && !next.starts_with("//") {
        next
    } else {
        "/".to_string()
    };
    let next_q = url_encode(&next);
    rsx! {
        Seo {
            title: "Sign in | RecompHub".to_string(),
            description: "Sign in to RecompHub to star ports, rate them, comment and submit new ones.".to_string(),
            path: "/sign-in".to_string(),
            noindex: true,
        }
        div { class: "mx-auto mt-12 max-w-sm",
            div { class: "{CARD_SURFACE} flex flex-col gap-6 p-8",
                div { class: "space-y-1 text-center",
                    h1 { class: "text-xl font-semibold tracking-tight", "Sign in to RecompHub" }
                    p { class: "text-sm text-muted-foreground", "Star ports, rate them, comment and submit new ones." }
                }
                if methods.is_empty() {
                    p { class: "text-center text-sm text-muted-foreground", "Sign-in isn't available yet." }
                } else {
                    div { class: "flex flex-col gap-3",
                        for method in methods {
                            if method == "google" {
                                a { key: "{method}", href: "/auth/google?next={next_q}",
                                    class: "{BUTTON} {BUTTON_PRIMARY} {BUTTON_MD} w-full gap-2",
                                    GoogleIcon { class: "size-4" }
                                    "Continue with Google"
                                }
                            } else if method == "github" {
                                a { key: "{method}", href: "/auth/github?next={next_q}",
                                    class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD} w-full gap-2",
                                    GithubIcon { class: "size-4" }
                                    "Continue with GitHub"
                                }
                            }
                        }
                    }
                }
                p { class: "text-center text-xs text-muted-foreground",
                    "We only use your name and picture. Your email is never shown."
                }
            }
        }
    }
}
