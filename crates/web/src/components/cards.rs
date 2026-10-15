use dioxus::prelude::*;
use recomphub_shared::GameCard;

use crate::components::{LiveCount, Picture};

/// Cover-art card for one game. Links to the game page. Shows the rank numeral when given.
#[component]
pub fn GameCardView(game: GameCard, #[props(default)] rank: Option<i64>) -> Element {
    let href = format!("/games/{}", game.slug);
    let portless = game.port_count == 0;
    let initials: String = game
        .name
        .split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .collect();
    let year = game.release_year.map(|y| y.to_string()).unwrap_or_default();
    let cls = if portless { "grayscale-cover" } else { "" };
    rsx! {
        article { class: "card group relative flex flex-col gap-2 rounded-2xl border border-border bg-surface p-2 transition hover:-translate-y-1 hover:shadow-xl hover:shadow-accent-soft",
            a { href: "{href}", class: "block",
                div { class: "cover-tilt relative aspect-[3/4] overflow-hidden rounded-xl",
                    Picture {
                        media: game.cover.clone(),
                        alt: format!("{} cover art", game.name),
                        initials: initials,
                        sizes: "(min-width: 1024px) 16vw, 45vw",
                        class: "h-full w-full object-cover {cls}",
                    }
                }
                if let Some(r) = rank {
                    span { class: "pointer-events-none absolute -left-2 bottom-4 text-6xl font-black text-transparent [-webkit-text-stroke:2px_var(--color-foreground)]", "{r}" }
                }
                h3 { class: "mt-2 line-clamp-2 text-sm font-extrabold leading-tight tracking-tight", "{game.name}" }
            }
            p { class: "text-xs text-muted tabular", "{year}" }
            div { class: "flex flex-wrap items-center gap-2 text-xs",
                if portless {
                    span { class: "rounded-full bg-surface-2 px-2 py-0.5 text-warning", "No port yet" }
                    span { class: "text-muted", "{game.want_count} want it" }
                } else {
                    span { class: "rounded-full bg-surface-2 px-2 py-0.5 text-success", "{game.port_count} port(s)" }
                }
                LiveCount { game_id: game.id, portless: portless }
            }
        }
    }
}
