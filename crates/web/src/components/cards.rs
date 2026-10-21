use dioxus::prelude::*;
use recomphub_shared::GameCard;

use crate::components::icons::{AndroidIcon, AppleIcon, GlobeIcon, LinuxIcon, WindowsIcon};
use crate::components::styles::{BADGE, BADGE_OUTLINE, BADGE_SECONDARY};
use crate::components::{FavoriteButton, LiveCount, Picture, initials_of, target_label};

/// Targets with an icon on cover cards, in display order. Others are counted as "+N".
const ICON_TARGETS: [&str; 6] = ["windows", "macos", "linux", "android", "ios", "web"];

/// Cover-art card for one game, in the usual media-catalog shape: cover, title, one meta line.
/// Links to the game page. Shows a rank badge on the cover when given, and the platforms its
/// ports run on in the cover's bottom corner.
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
    let icon_targets: Vec<&'static str> = ICON_TARGETS
        .iter()
        .copied()
        .filter(|t| game.targets.iter().any(|x| x == t))
        // macOS and iOS share the Apple icon; show it once.
        .filter(|t| *t != "ios" || !game.targets.iter().any(|x| x == "macos"))
        .collect();
    let other_targets: Vec<String> = game
        .targets
        .iter()
        .filter(|t| !ICON_TARGETS.contains(&t.as_str()))
        .map(|t| target_label(t))
        .collect();
    let platforms_label = game
        .targets
        .iter()
        .map(|t| target_label(t))
        .collect::<Vec<_>>()
        .join(", ");
    let favorite_name = game.name.clone();
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
                    if !game.targets.is_empty() {
                        span {
                            class: "pointer-events-none absolute bottom-2 right-2 flex items-center gap-1.5 rounded-md bg-black/70 px-1.5 py-1 text-white shadow-sm backdrop-blur-sm",
                            title: "Runs natively on {platforms_label}",
                            span { class: "sr-only", "Runs natively on {platforms_label}" }
                            for t in icon_targets {
                                {platform_icon(t)}
                            }
                            if !other_targets.is_empty() {
                                span { class: "text-[10px] font-medium leading-none tabular", "aria-hidden": "true", "+{other_targets.len()}" }
                            }
                        }
                    }
                }
                h3 { class: "line-clamp-2 text-sm font-medium leading-snug group-hover:underline", "{game.name}" }
            }
            div { class: "flex justify-end",
                FavoriteButton { game_id: game.id, game_name: favorite_name }
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

/// White platform glyph for the cover overlay.
fn platform_icon(target: &str) -> Element {
    let class = "size-3.5".to_string();
    match target {
        "windows" => rsx! { WindowsIcon { class } },
        "macos" | "ios" => rsx! { AppleIcon { class } },
        "linux" => rsx! { LinuxIcon { class } },
        "android" => rsx! { AndroidIcon { class } },
        _ => rsx! { GlobeIcon { class } },
    }
}
