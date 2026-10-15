use dioxus::fullstack::FullstackContext;
use dioxus::fullstack::http::StatusCode;
use dioxus::prelude::*;

use crate::components::Seo;

/// Catch-all. Commits a 404 status during SSR and renders a noindex page.
#[component]
pub fn NotFound(segments: Vec<String>) -> Element {
    let _ = segments;
    FullstackContext::commit_http_status(StatusCode::NOT_FOUND, None);
    rsx! { RouteError { status: 404 } }
}

/// Body for route errors: the `NotFound` route and loader failures caught by `Shell`'s error boundary.
#[component]
pub fn RouteError(status: u16) -> Element {
    let (title, description, heading, text) = if status == 404 {
        (
            "Page not found | RecompHub",
            "This page does not exist on RecompHub.",
            "Page not found",
            "The page you are looking for does not exist.",
        )
    } else {
        (
            "Something went wrong | RecompHub",
            "This page could not be loaded.",
            "Something went wrong",
            "This page could not be loaded. Try again in a moment.",
        )
    };
    rsx! {
        Seo {
            title: title.to_string(),
            description: description.to_string(),
            path: "/404".to_string(),
            noindex: true,
        }
        section { class: "py-24 text-center",
            h1 { class: "text-5xl font-black", "{heading}" }
            p { class: "mt-4 text-muted", "{text}" }
            a { href: "/", class: "mt-8 inline-block font-semibold accent-text", "Back to the front page" }
        }
    }
}
