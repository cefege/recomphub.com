//! Shared pieces for the "Join Discord" CTAs scattered across the site. Callers compose their
//! own button classes around `DiscordLink`'s icon+label and drop in `DiscordCount` where there's
//! room for social proof. Each CTA should link through its own placement invite (see
//! `app::DISCORD_INVITE_*`) so Discord's per-invite use count shows which spot converts.

use dioxus::prelude::*;

use crate::api;
use crate::components::icons::DiscordIcon;
use crate::components::styles::{BADGE, BADGE_SECONDARY};

/// Below this, a member count reads as embarrassing rather than persuasive, so it stays hidden
/// until the server has enough people to be worth bragging about.
const MIN_COUNT_TO_SHOW: i64 = 25;

/// Discord icon and label, for composing into a button, menu item, or plain link. `href` should
/// be a placement-specific invite link, not the shared default.
#[component]
pub fn DiscordLink(
    href: &'static str,
    label: &'static str,
    #[props(default)] class: String,
) -> Element {
    rsx! {
        crate::components::ExternalLink {
            url: href.to_string(),
            class,
            DiscordIcon { class: "size-4" }
            "{label}"
        }
    }
}

/// Live "N members · M online" badge. Renders nothing while loading, on a failed fetch, or while
/// the member count is below `MIN_COUNT_TO_SHOW`, so it never leaves a stale or thin placeholder.
#[component]
pub fn DiscordCount() -> Element {
    let stats = use_resource(api::discord_stats);
    match &*stats.read() {
        Some(Ok(s)) if s.member_count >= MIN_COUNT_TO_SHOW => rsx! {
            span { class: "{BADGE} {BADGE_SECONDARY} overflow-visible! tabular",
                span { class: "relative flex h-2 w-2",
                    span { class: "absolute inline-flex h-full w-full rounded-full bg-live live-ping" }
                    span { class: "relative inline-flex h-2 w-2 rounded-full bg-live" }
                }
                "{compact(s.member_count)} members"
                if s.online_count > 0 {
                    " · {compact(s.online_count)} online"
                }
            }
        },
        _ => rsx! {},
    }
}

/// `284` as-is, `12.3k` from 10,000 up.
fn compact(n: i64) -> String {
    if n < 10_000 {
        n.to_string()
    } else {
        format!("{:.1}k", n as f64 / 1000.0)
    }
}
