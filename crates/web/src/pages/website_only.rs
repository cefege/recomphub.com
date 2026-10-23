//! Native handoff for tools that intentionally remain on the website.

use dioxus::prelude::*;

use crate::routes::{NativeWebsiteOnly, Route};

#[component]
pub fn AccountPage() -> Element {
    rsx! { NativeWebsiteOnly { path: Route::AccountPage {}.to_string(), heading: "RecompHub account" } }
}

#[component]
pub fn AdminPage() -> Element {
    rsx! { NativeWebsiteOnly { path: Route::AdminPage {}.to_string(), heading: "Administration" } }
}

#[component]
pub fn ModerationQueue() -> Element {
    rsx! { NativeWebsiteOnly { path: Route::ModerationQueue {}.to_string(), heading: "Moderation" } }
}

#[component]
pub fn ModerationLog(page: String) -> Element {
    rsx! { NativeWebsiteOnly { path: Route::ModerationLog { page }.to_string(), heading: "Moderation log" } }
}

#[component]
pub fn PrivacyPage() -> Element {
    rsx! { NativeWebsiteOnly { path: Route::PrivacyPage {}.to_string(), heading: "Privacy" } }
}

#[component]
pub fn SubmitPage(game: String, repo: String) -> Element {
    rsx! { NativeWebsiteOnly { path: Route::SubmitPage { game, repo }.to_string(), heading: "Submit a port" } }
}

#[component]
pub fn EditPortPage(port_id: i64) -> Element {
    rsx! { NativeWebsiteOnly { path: Route::EditPortPage { port_id }.to_string(), heading: "Suggest an edit" } }
}
