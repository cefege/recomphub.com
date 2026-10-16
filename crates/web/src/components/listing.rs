use dioxus::prelude::*;
use recomphub_shared::{GameCard, HitWindow, PortCard, RankedGame};

use crate::components::live::use_live_watch;
use crate::components::styles::{
    BUTTON, BUTTON_OUTLINE, BUTTON_SM, TABLE, TABLE_HEAD_ROW, TABLE_ROW, TABLE_TD, TABLE_TH,
    TABLE_TH_NUM, TABLE_WRAP,
};
use crate::components::ui::{TrendChip, target_label, url_encode, version_label};

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
        div { class: TABLE_WRAP,
            table { class: "{TABLE} min-w-[36rem]",
                thead {
                    tr { class: TABLE_HEAD_ROW,
                        th { class: TABLE_TH, "Game" }
                        th { class: TABLE_TH, "Year" }
                        th { class: TABLE_TH_NUM, "Ports" }
                        th { class: TABLE_TH, "Runs on" }
                        th { class: TABLE_TH_NUM, "Wanted" }
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
        tr { class: TABLE_ROW,
            td { class: "{TABLE_TD} font-medium", a { href: "{href}", class: "hover:underline", "{name}" } }
            td { class: "{TABLE_TD} tabular text-muted-foreground", "{year}" }
            td { class: "{TABLE_TD} text-right tabular", "{ports}" }
            td { class: "{TABLE_TD} text-muted-foreground", "{runs}" }
            td { class: "{TABLE_TD} text-right tabular", "{wanted}" }
        }
    }
}

/// Classes for one link in a wrapping set of filter pills: filled when selected, outline otherwise.
pub fn chip_class(on: bool) -> String {
    if on {
        format!(
            "{BUTTON} {BUTTON_SM} rounded-full bg-primary text-primary-foreground hover:bg-primary/90"
        )
    } else {
        format!("{BUTTON} {BUTTON_SM} rounded-full {BUTTON_OUTLINE}")
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
        div { class: TABLE_WRAP,
            table { class: "{TABLE} min-w-[36rem]",
                thead {
                    tr { class: TABLE_HEAD_ROW,
                        th { class: "{TABLE_TH_NUM} w-12", "#" }
                        th { class: TABLE_TH, "Game" }
                        th { class: TABLE_TH_NUM, "Score" }
                        th { class: TABLE_TH, "Trend" }
                        th { class: TABLE_TH_NUM, "Ports" }
                        th { class: TABLE_TH_NUM, "Wanted" }
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
        tr { class: TABLE_ROW,
            td { class: "{TABLE_TD} text-right tabular text-muted-foreground", "{rank}" }
            td { class: "{TABLE_TD} font-medium", a { href: "{href}", class: "hover:underline", "{name}" } }
            td { class: "{TABLE_TD} text-right tabular", "{score}" }
            td { class: TABLE_TD, TrendChip { rank: row.rank, prev_rank: row.prev_rank } }
            td { class: "{TABLE_TD} text-right tabular", "{ports}" }
            td { class: "{TABLE_TD} text-right tabular", "{wanted}" }
        }
    }
}

/// Compact table of ports for the `?view=list` pages.
#[component]
pub fn PortTable(ports: Vec<PortCard>) -> Element {
    rsx! {
        div { class: TABLE_WRAP,
            table { class: "{TABLE} min-w-[40rem]",
                thead {
                    tr { class: TABLE_HEAD_ROW,
                        th { class: TABLE_TH, "Port" }
                        th { class: TABLE_TH, "Game" }
                        th { class: TABLE_TH, "Kind" }
                        th { class: TABLE_TH, "Runs on" }
                        th { class: TABLE_TH_NUM, "GitHub stars" }
                        th { class: TABLE_TH, "Release" }
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
        .as_deref()
        .map(version_label)
        .unwrap_or_else(|| port.release_state_label.clone());
    let name = port.name.clone();
    let game = port.game_name.clone();
    let kind = port.kind_label.clone();
    let stars = port.github_stars;
    rsx! {
        tr { class: TABLE_ROW,
            td { class: "{TABLE_TD} font-medium", a { href: "{href}", class: "hover:underline", "{name}" } }
            td { class: TABLE_TD, a { href: "{game_href}", class: "text-muted-foreground hover:text-foreground", "{game}" } }
            td { class: "{TABLE_TD} text-muted-foreground", "{kind}" }
            td { class: "{TABLE_TD} text-muted-foreground", "{runs}" }
            td { class: "{TABLE_TD} text-right tabular", "{stars}" }
            td { class: "{TABLE_TD} text-muted-foreground", "{release}" }
        }
    }
}
