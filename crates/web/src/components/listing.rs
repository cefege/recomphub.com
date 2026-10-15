use dioxus::prelude::*;
use recomphub_shared::{GameCard, HitWindow, PortCard, RankedGame};

use crate::components::live::use_live_watch;
use crate::components::ui::{TrendChip, target_label, url_encode};

/// Primary targets, in the order they appear as availability chips.
pub const PRIMARY_TARGETS: [&str; 4] = ["windows", "macos", "linux", "web"];

/// Window chips in display order: (window, query value, chip label).
pub const WINDOWS: [(HitWindow, &str, &str); 4] = [
    (HitWindow::Today, "today", "Today"),
    (HitWindow::Week, "week", "This week"),
    (HitWindow::Month, "month", "This month"),
    (HitWindow::Year, "year", "This year"),
];

/// Parses a `page` query value. Anything below 1 or unparseable is page 1.
pub fn parse_page(raw: &str) -> i64 {
    raw.trim()
        .parse::<i64>()
        .ok()
        .filter(|page| *page >= 1)
        .unwrap_or(1)
}

/// Builds `?k=v&...` from the non-empty pairs. Returns an empty string when none are set.
pub fn query_string(pairs: &[(&str, String)]) -> String {
    let parts: Vec<String> = pairs
        .iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(key, value)| format!("{key}={}", url_encode(value)))
        .collect();
    if parts.is_empty() {
        String::new()
    } else {
        format!("?{}", parts.join("&"))
    }
}

/// Compact table of games for the `?view=list` pages.
#[component]
pub fn GameListTable(games: Vec<GameCard>) -> Element {
    rsx! {
        div { class: "overflow-x-auto rounded-2xl border border-border",
            table { class: "w-full min-w-[36rem] text-left text-sm",
                thead { class: "bg-surface-2 text-xs uppercase tracking-wide text-muted",
                    tr {
                        th { class: "px-4 py-3 font-bold", "Game" }
                        th { class: "px-4 py-3 font-bold", "Year" }
                        th { class: "px-4 py-3 font-bold", "Ports" }
                        th { class: "px-4 py-3 font-bold", "Runs on" }
                        th { class: "px-4 py-3 font-bold", "Wanted" }
                    }
                }
                tbody {
                    for game in games {
                        {
                            let key = game.id.to_string();
                            rsx! { GameRow { key: "{key}", game } }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn GameRow(game: GameCard) -> Element {
    let href = format!("/games/{}", game.slug);
    let year = game
        .release_year
        .map(|y| y.to_string())
        .unwrap_or_else(|| "—".to_string());
    let runs = game
        .targets
        .iter()
        .map(|t| target_label(t))
        .collect::<Vec<_>>()
        .join(", ");
    let runs = if runs.is_empty() {
        "None yet".to_string()
    } else {
        runs
    };
    let name = game.name.clone();
    let ports = game.port_count;
    let wanted = game.want_count;
    rsx! {
        tr { class: "border-t border-border hover:bg-surface-2",
            td { class: "px-4 py-3 font-semibold", a { href: "{href}", class: "hover:text-accent", "{name}" } }
            td { class: "px-4 py-3 tabular text-muted", "{year}" }
            td { class: "px-4 py-3 tabular", "{ports}" }
            td { class: "px-4 py-3 text-muted", "{runs}" }
            td { class: "px-4 py-3 tabular", "{wanted}" }
        }
    }
}

/// Chip classes for filter and window links.
pub fn chip_class(on: bool) -> String {
    if on {
        "border-accent bg-accent/15 text-foreground".to_string()
    } else {
        "border-border text-muted hover:text-foreground".to_string()
    }
}

/// Declares the games on this list to the live socket, so their live counts update.
#[component]
pub fn WatchVisible(ids: Vec<i64>) -> Element {
    use_live_watch(None, ids);
    rsx! {}
}

/// Compact ranking table for `/trending?view=list`.
#[component]
pub fn RankingTable(rows: Vec<RankedGame>) -> Element {
    rsx! {
        div { class: "overflow-x-auto rounded-2xl border border-border",
            table { class: "w-full min-w-[36rem] text-left text-sm",
                thead { class: "bg-surface-2 text-xs uppercase tracking-wide text-muted",
                    tr {
                        th { class: "px-4 py-3 font-bold", "#" }
                        th { class: "px-4 py-3 font-bold", "Game" }
                        th { class: "px-4 py-3 font-bold", "Score" }
                        th { class: "px-4 py-3 font-bold", "Trend" }
                        th { class: "px-4 py-3 font-bold", "Ports" }
                        th { class: "px-4 py-3 font-bold", "Wanted" }
                    }
                }
                tbody {
                    for row in rows {
                        {
                            let key = row.game.id.to_string();
                            rsx! { RankingRow { key: "{key}", row } }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn RankingRow(row: RankedGame) -> Element {
    let href = format!("/games/{}", row.game.slug);
    let name = row.game.name.clone();
    let score = format!("{:.1}", row.score);
    let rank = row.rank.to_string();
    let ports = row.game.port_count;
    let wanted = row.game.want_count;
    rsx! {
        tr { class: "border-t border-border hover:bg-surface-2",
            td { class: "px-4 py-3 tabular font-extrabold", "{rank}" }
            td { class: "px-4 py-3 font-semibold", a { href: "{href}", class: "hover:text-accent", "{name}" } }
            td { class: "px-4 py-3 tabular", "{score}" }
            td { class: "px-4 py-3", TrendChip { rank: row.rank, prev_rank: row.prev_rank } }
            td { class: "px-4 py-3 tabular", "{ports}" }
            td { class: "px-4 py-3 tabular", "{wanted}" }
        }
    }
}

/// Compact table of ports for the `?view=list` pages.
#[component]
pub fn PortTable(ports: Vec<PortCard>) -> Element {
    rsx! {
        div { class: "overflow-x-auto rounded-2xl border border-border",
            table { class: "w-full min-w-[40rem] text-left text-sm",
                thead { class: "bg-surface-2 text-xs uppercase tracking-wide text-muted",
                    tr {
                        th { class: "px-4 py-3 font-bold", "Port" }
                        th { class: "px-4 py-3 font-bold", "Game" }
                        th { class: "px-4 py-3 font-bold", "Kind" }
                        th { class: "px-4 py-3 font-bold", "Runs on" }
                        th { class: "px-4 py-3 font-bold", "GitHub ★" }
                        th { class: "px-4 py-3 font-bold", "Release" }
                    }
                }
                tbody {
                    for port in ports {
                        {
                            let key = port.id.to_string();
                            rsx! { PortRow { key: "{key}", port } }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn PortRow(port: PortCard) -> Element {
    let href = format!("/games/{}/{}", port.game_slug, port.slug);
    let game_href = format!("/games/{}", port.game_slug);
    let runs = port
        .targets
        .iter()
        .map(|t| target_label(t))
        .collect::<Vec<_>>()
        .join(", ");
    let runs = if runs.is_empty() {
        "None yet".to_string()
    } else {
        runs
    };
    let release = port
        .latest_version
        .clone()
        .unwrap_or_else(|| port.release_state_label.clone());
    let name = port.name.clone();
    let game = port.game_name.clone();
    let kind = port.kind_label.clone();
    let stars = port.github_stars;
    rsx! {
        tr { class: "border-t border-border hover:bg-surface-2",
            td { class: "px-4 py-3 font-semibold", a { href: "{href}", class: "hover:text-accent", "{name}" } }
            td { class: "px-4 py-3", a { href: "{game_href}", class: "text-muted hover:text-foreground", "{game}" } }
            td { class: "px-4 py-3 text-muted", "{kind}" }
            td { class: "px-4 py-3 text-muted", "{runs}" }
            td { class: "px-4 py-3 tabular", "{stars}" }
            td { class: "px-4 py-3 text-muted", "{release}" }
        }
    }
}
