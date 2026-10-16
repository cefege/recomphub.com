use dioxus::prelude::*;
use recomphub_shared::dto::FundingLink;

use crate::components::icons::HeartIcon;
use crate::components::styles::{BADGE, BADGE_OUTLINE};

/// Donation links for a creator, one outline badge per platform. Renders nothing when empty.
#[component]
pub fn FundingLinks(links: Vec<FundingLink>) -> Element {
    if links.is_empty() {
        return rsx! {};
    }
    rsx! {
        div { class: "flex flex-wrap items-center gap-2",
            for (i, link) in links.iter().enumerate() {
                a {
                    key: "{i}",
                    href: "{link.url}",
                    title: "{link.url}",
                    target: "_blank",
                    rel: "noopener sponsored",
                    class: "{BADGE} {BADGE_OUTLINE}",
                    HeartIcon {}
                    "{link.platform.label()}"
                }
            }
        }
    }
}
