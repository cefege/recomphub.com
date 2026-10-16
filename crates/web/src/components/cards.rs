use dioxus::prelude::*;
use recomphub_shared::GameCard;

use crate::components::styles::{BADGE, BADGE_OUTLINE, BADGE_SECONDARY};
use crate::components::{LiveCount, Picture, initials_of};

/// Cover-art card for one game, in the usual media-catalog shape: cover, title, one meta line.
/// Links to the game page. Shows a rank badge on the cover when given.
#[component]
pub fn GameCardView(game: GameCard, #[props(default)] rank: Option<i64>) -> Element {
    let href = format!("/games/{}", game.slug);
    let portless = game.port_count == 0;
    let initials = initials_of(&game.name);
    let year = game.release_year.map(|y| y.to_string());
    let ports = match game.port_count {
        0 => None,
        1 => Some("1 port".to_string()),
        n => Some(format!("{n} ports")),
    };
    let wants = match game.want_count {
        0 => None,
        1 => Some("1 wants it".to_string()),
        n => Some(format!("{n} want it")),
    };
    rsx! {
        article { class: "group relative flex flex-col gap-2",
            a { href: "{href}", class: "flex flex-col gap-2 rounded-lg outline-offset-4",
                div { class: "relative aspect-[3/4] overflow-hidden rounded-lg border bg-muted",
                    Picture {
                        media: game.cover.clone(),
                        alt: format!("{} cover art", game.name),
                        initials: initials,
                        sizes: "(min-width: 1024px) 16vw, 45vw",
                        class: "h-full w-full object-cover transition-opacity group-hover:opacity-90",
                    }
                    if let Some(r) = rank {
                        span { class: "{BADGE} absolute left-2 top-2 min-w-7 bg-background/90 text-foreground shadow-sm tabular backdrop-blur-sm", "#{r}" }
                    }
                }
                h3 { class: "line-clamp-2 text-sm font-medium leading-snug group-hover:underline", "{game.name}" }
            }
            div { class: "flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground",
                if let Some(y) = year {
                    span { class: "tabular", "{y}" }
                }
                match ports {
                    Some(p) => rsx! { span { class: "{BADGE} {BADGE_SECONDARY}", "{p}" } },
                    None => rsx! { span { class: "{BADGE} {BADGE_OUTLINE}", "No port yet" } },
                }
                if portless {
                    if let Some(w) = wants {
                        span { class: "tabular", "{w}" }
                    }
                }
                LiveCount { game_id: game.id, portless: portless }
            }
        }
    }
}
