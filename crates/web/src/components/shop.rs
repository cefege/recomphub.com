//! "Get the original game" affiliate shelf on game pages.

#![cfg_attr(feature = "desktop", allow(unused_imports))]
use dioxus::prelude::*;
use recomphub_shared::shop::AMAZON_DISCLOSURE;
use recomphub_shared::{BuyOffer, MediaRef, OfferKind};

use crate::components::Picture;
use crate::components::styles::{
    BADGE, BADGE_SECONDARY, BUTTON, BUTTON_MD, BUTTON_OUTLINE, BUTTON_SM, CARD_SURFACE,
};
use crate::components::ui::{ExternalLink, Section, initials_of};

/// Swipeable rail of Amazon search links. Renders nothing without offers, and never in the
/// desktop app (Associate links inside apps need separate Amazon approval).
#[component]
pub fn BuyShelf(
    offers: Vec<BuyOffer>,
    game_name: String,
    cover: Option<MediaRef>,
    has_ports: bool,
) -> Element {
    #[cfg(feature = "desktop")]
    {
        let _ = (&offers, &game_name, &cover, has_ports);
        return rsx! {};
    }
    #[cfg(not(feature = "desktop"))]
    {
        if offers.is_empty() {
            return rsx! {};
        }
        let intro = if has_ports {
            "Find a copy on Amazon. Check the port's requirements before buying: not every edition supplies compatible game files."
        } else {
            "Search Amazon for a copy of this game. Listings may include used copies and third-party sellers."
        };
        let initials = initials_of(&game_name);
        rsx! {
            div { id: "buy-original",
                Section { heading: "Get the original game".to_string(), class: "mt-12".to_string(),
                    p { class: "mb-3 text-sm text-muted-foreground", "{intro}" }
                    p { class: "mb-3 text-sm text-muted-foreground", "{AMAZON_DISCLOSURE}" }
                    if offers.len() > 1 {
                        div { id: "buy-original-navigation", class: "mb-3 flex justify-end gap-2",
                            button {
                                r#type: "button",
                                class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM}",
                                aria_label: "Previous purchase options",
                                aria_controls: "buy-original-offers",
                                onclick: move |_| scroll_offers(-1),
                                "Previous"
                            }
                            button {
                                r#type: "button",
                                class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM}",
                                aria_label: "Next purchase options",
                                aria_controls: "buy-original-offers",
                                onclick: move |_| scroll_offers(1),
                                "Next"
                            }
                        }
                    }
                    ul {
                        key: "{game_name}",
                        id: "buy-original-offers",
                        class: "rail flex snap-x gap-4 overflow-x-auto pb-4",
                        tabindex: "0",
                        aria_label: format!("Purchase options for {game_name}"),
                        onmounted: move |_| async move {
                            let _ = document::eval(
                                r#"
const rail = document.getElementById("buy-original-offers");
const navigation = document.getElementById("buy-original-navigation");
if (navigation) {
    const [previous, next] = navigation.querySelectorAll("button");
    const observer = new ResizeObserver(update);
    function update() {
        if (!rail.isConnected) {
            observer.disconnect();
            return;
        }
        const end = rail.scrollWidth - rail.clientWidth;
        navigation.style.display = end > 1 ? "" : "none";
        previous.disabled = rail.scrollLeft <= 1;
        next.disabled = rail.scrollLeft >= end - 1;
    }
    observer.observe(rail);
    rail.addEventListener("scroll", update, {passive: true});
    update();
}
"#,
                            ).join::<()>().await;
                        },
                        onkeydown: move |event: KeyboardEvent| {
                            let direction = match event.key() {
                                Key::ArrowLeft => -1,
                                Key::ArrowRight => 1,
                                Key::Home => -2,
                                Key::End => 2,
                                _ => return,
                            };
                            event.prevent_default();
                            spawn(scroll_offers(direction));
                        },
                        for (index, offer) in offers.into_iter().enumerate() {
                            li { key: "{index}", class: "w-72 shrink-0 snap-start",
                                div { class: "{CARD_SURFACE} flex h-full flex-col overflow-hidden",
                                    div { class: "relative aspect-video shrink-0 overflow-hidden",
                                        if offer.kind == OfferKind::Original {
                                            Picture {
                                                media: cover.clone(),
                                                alt: format!("Cover art for {game_name}"),
                                                initials: initials.clone(),
                                                sizes: "240px".to_string(),
                                                class: "h-full w-full object-cover".to_string(),
                                            }
                                            span { class: "{BADGE} {BADGE_SECONDARY} absolute left-2 top-2", "{offer.badge}" }
                                        } else {
                                            div { class: "flex h-full w-full items-center justify-center bg-muted",
                                                span { class: "tabular text-3xl font-semibold text-muted-foreground", "{offer.badge}" }
                                            }
                                        }
                                    }
                                    div { class: "flex flex-1 flex-col p-4",
                                        p { class: "font-medium", "{offer.title}" }
                                        p { class: "mb-3 text-sm text-muted-foreground", "{offer.subtitle}" }
                                        ExternalLink {
                                            url: offer.url.clone(),
                                            sponsored: true,
                                            class: format!("{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD} mt-auto w-full"),
                                            "Search Amazon"
                                        }
                                    }
                                }
                            }
                        }
                    }
                    p { class: "text-xs text-muted-foreground",
                        "Links open Amazon.com search results, not a specific product. Check the edition, region, condition and seller before buying. Prices and availability are set by Amazon."
                    }
                }
            }
        }
    }
}

#[cfg(not(feature = "desktop"))]
async fn scroll_offers(direction: i32) {
    let eval = document::eval(
        r#"
const direction = await dioxus.recv();
const rail = document.getElementById("buy-original-offers");
if (rail) {
    const behavior = matchMedia("(prefers-reduced-motion: reduce)").matches ? "instant" : "smooth";
    if (Math.abs(direction) === 2) {
        rail.scrollTo({left: direction < 0 ? 0 : rail.scrollWidth, behavior});
    } else {
        const card = rail.firstElementChild;
        const step = card.getBoundingClientRect().width + parseFloat(getComputedStyle(rail).columnGap);
        rail.scrollBy({left: direction * step, behavior});
    }
}
"#,
    );
    if eval.send(direction).is_ok() {
        let _ = eval.join::<()>().await;
    }
}
