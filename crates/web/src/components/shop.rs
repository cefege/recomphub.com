//! "Get the original game" purchase module on game pages.

#![cfg_attr(feature = "desktop", allow(unused_imports))]
use dioxus::prelude::*;
use recomphub_shared::shop::AMAZON_DISCLOSURE;
use recomphub_shared::{BuyOffer, MediaRef, OfferKind};

use crate::components::Picture;
use crate::components::styles::{BUTTON, BUTTON_AMAZON, BUTTON_LG, CARD_SURFACE, TEXT_LINK};
use crate::components::ui::{ExternalLink, Section, initials_of};

/// Original-game Amazon searches with optional hardware offers. Renders nothing without offers,
/// and never in the desktop app (Associate links inside apps need separate Amazon approval).
#[component]
pub fn BuyShelf(
    offers: Vec<BuyOffer>,
    game_name: String,
    cover: Option<MediaRef>,
    has_ports: bool,
    #[props(default)] embedded: bool,
) -> Element {
    #[cfg(feature = "desktop")]
    {
        let _ = (&offers, &game_name, &cover, has_ports, embedded);
        return rsx! {};
    }
    #[cfg(not(feature = "desktop"))]
    {
        if offers.is_empty() {
            return rsx! {};
        }
        let (originals, optional) = offers
            .into_iter()
            .partition::<Vec<_>, _>(|offer| offer.kind == OfferKind::Original);
        if originals.is_empty() {
            return rsx! {};
        }
        let intro = if embedded {
            "Need a copy? Choose an original edition below."
        } else if has_ports {
            "Already own a compatible copy? You don't need to buy it again. Otherwise, choose an original edition below."
        } else {
            "Copies for the original system. No native port is listed yet."
        };
        rsx! {
            div { key: "{game_name}", id: "buy-original", class: if embedded { "mt-4" } else { "" },
                if embedded {
                    BuyShelfContent { originals, optional, game_name, cover, intro, has_ports, embedded }
                } else {
                    Section { heading: "Get the original game".to_string(), class: "mt-8".to_string(),
                        BuyShelfContent { originals, optional, game_name, cover, intro, has_ports, embedded }
                    }
                }
            }
        }
    }
}

#[cfg(not(feature = "desktop"))]
#[component]
fn BuyShelfContent(
    originals: Vec<BuyOffer>,
    optional: Vec<BuyOffer>,
    game_name: String,
    cover: Option<MediaRef>,
    intro: &'static str,
    has_ports: bool,
    embedded: bool,
) -> Element {
    let initials = initials_of(&game_name);
    rsx! {
        div { class: if embedded { "border-t pt-4".to_string() } else { format!("{CARD_SURFACE} p-5") },
            div { class: "flex items-start gap-4",
                if let Some(media) = cover {
                    Picture {
                        media: Some(media),
                        alt: format!("Cover art for {game_name}"),
                        initials,
                        sizes: "80px".to_string(),
                        class: "aspect-[3/4] w-20 shrink-0 rounded-md object-cover".to_string(),
                    }
                }
                div { class: "min-w-0",
                    h3 { class: "font-medium", "{game_name}" }
                    p { class: "mt-1 text-sm leading-relaxed text-muted-foreground", "{intro}" }
                    p { class: "mt-3 text-sm text-muted-foreground", "{AMAZON_DISCLOSURE}" }
                }
            }
            ul { class: "mt-4 divide-y", aria_label: format!("Original editions of {game_name} on Amazon"),
                for (index, offer) in originals.into_iter().enumerate() {
                    li { key: "{index}", class: "flex flex-col gap-3 py-3 first:pt-0 last:pb-0 sm:flex-row sm:items-center sm:justify-between",
                        span { class: "text-sm font-medium", "{offer.title}" }
                        ExternalLink {
                            url: offer.url,
                            sponsored: true,
                            class: format!("{BUTTON} {BUTTON_AMAZON} {BUTTON_LG} min-h-11 w-full sm:w-auto"),
                            "Buy on Amazon ↗"
                            span { class: "sr-only", " for {offer.title}" }
                        }
                    }
                }
            }
            p { class: "mt-4 border-t pt-3 text-xs leading-relaxed text-muted-foreground",
                if has_ports {
                    "Search results, not verified compatible copies. Match the edition and region to your port's requirements. Physical copies may need separate file extraction."
                } else {
                    "Links open Amazon.com search results. Check the edition, region, condition and seller before buying."
                }
            }
            if !optional.is_empty() {
                details { class: "mt-4 border-t pt-3",
                    summary { class: "cursor-pointer text-sm text-muted-foreground hover:text-foreground",
                        "Optional hardware and accessories"
                    }
                    p { class: "mt-3 text-sm text-muted-foreground", "These are optional, not requirements for running a native port." }
                    ul { class: "mt-3 divide-y",
                        for (index, offer) in optional.into_iter().enumerate() {
                            li { key: "{index}", class: "flex flex-col gap-2 py-3 sm:flex-row sm:items-center sm:justify-between",
                                div {
                                    p { class: "text-sm font-medium", "{offer.title}" }
                                    p { class: "text-xs text-muted-foreground", "{offer.subtitle}" }
                                }
                                ExternalLink {
                                    url: offer.url,
                                    sponsored: true,
                                    class: format!("{TEXT_LINK} shrink-0 text-sm"),
                                    "Browse on Amazon"
                                    span { class: "sr-only", " for {offer.title}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
