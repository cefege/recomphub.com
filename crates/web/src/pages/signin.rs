//! `/signin?next=`: choose a sign-in provider. Each button opens that provider's start
//! route with `next` attached; the server validates `next` again before redirecting.

use dioxus::prelude::*;

use crate::api;
use crate::components::icons::{GithubIcon, GoogleIcon};
use crate::components::styles::*;
use crate::components::{Breadcrumb, Seo};

/// Percent-encodes a value for use inside a query string.
fn query_value(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[component]
pub fn SignInPage(next: String) -> Element {
    let configured = use_loader(api::auth_providers)?();
    let github_href = format!("/auth/github/login?next={}", query_value(&next));
    let google_href = format!("/auth/google/login?next={}", query_value(&next));

    rsx! {
        Seo {
            title: "Sign in | RecompHub".to_string(),
            description: "Sign in to RecompHub with GitHub or Google to comment, rate, star and submit ports.".to_string(),
            path: "/signin".to_string(),
            noindex: true,
        }
        div { class: "mx-auto max-w-md",
            header {
                Breadcrumb { items: vec![
                    ("Home".to_string(), "/".to_string()),
                    ("Sign in".to_string(), "/signin".to_string()),
                ] }
                h1 { class: "{H1} mt-4", "Sign in" }
                p { class: "{LEAD} mt-2",
                    "Use the provider you signed up with. Picking a different one creates a separate account. To use both, connect the second from your account page."
                }
            }
            section { class: "{CARD_SURFACE} mt-8 space-y-3 p-6",
                if configured.github {
                    a {
                        class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_LG} w-full",
                        href: "{github_href}",
                        GithubIcon { class: "size-4" }
                        "Continue with GitHub"
                    }
                }
                if configured.google {
                    a {
                        class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_LG} w-full",
                        href: "{google_href}",
                        GoogleIcon { class: "size-4" }
                        "Continue with Google"
                    }
                }
                if !configured.github && !configured.google {
                    p { class: "text-sm text-muted-foreground", "No sign-in provider is set up on this server yet." }
                }
            }
        }
    }
}
