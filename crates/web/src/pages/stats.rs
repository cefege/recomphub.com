//! `/stats`: platform parity. How close our ported games are to running on every major
//! platform, with milestones, a 30-day view and leaderboards.

use dioxus::prelude::*;
use recomphub_shared::dto::{ParityTargetDto, StatsPageDto};
use recomphub_shared::seo::meta_description;

use crate::api;
use crate::components::stats::{MilestoneList, ParityChart, ParityMeter, ProgressBar, UnlockStrip};
use crate::components::styles::{
    BUTTON_OUTLINE, BUTTON_XS, H1, H2, LEAD, TABLE, TABLE_HEAD_ROW, TABLE_ROW, TABLE_TD, TABLE_TH,
    TABLE_TH_NUM, TABLE_WRAP,
};
use crate::components::{Breadcrumb, EmptyState, InternalLink, Seo, TargetIcon, TrendChip};
use crate::pages::browse::count_label;

/// Pluralised count: `1 game`, `3 games`.
fn pl(n: i64, one: &str, many: &str) -> String {
    count_label(n, one, many)
}

fn pct_text(pct: f64) -> String {
    format!("{pct:.0}%")
}

/// One row of the platform board, precomputed for rendering.
struct BoardRow {
    slug: String,
    label: String,
    rank: i64,
    prev_rank: Option<i64>,
    games: i64,
    pct: f64,
    pct_text: String,
    change: Option<String>,
    gap_link: bool,
}

fn board_row(t: &ParityTargetDto) -> BoardRow {
    BoardRow {
        slug: t.slug.clone(),
        label: t.label.clone(),
        rank: t.rank,
        prev_rank: t.rank_30d,
        games: t.games,
        pct: t.pct,
        pct_text: pct_text(t.pct),
        change: t.pct_30d.map(|old| format!("{:+.1} pts", t.pct - old)),
        gap_link: t.gap_link,
    }
}

/// `/stats`: the parity hub.
#[component]
pub fn StatsPage() -> Element {
    let loaded = use_loader(api::stats_page)?;
    let data: StatsPageDto = loaded();

    let title = "Platform parity | RecompHub".to_string();
    let description = meta_description(
        "How many of our ported games run on Windows, macOS, Linux, web, Android and iOS, the milestones we have hit, and which games to port next.",
    );

    let parity = data.parity.clone();
    let board: Vec<BoardRow> = parity.targets.iter().map(board_row).collect();
    let has_board = !board.is_empty();

    let ports_period = (parity.ports_in_period > 0).then(|| {
        format!(
            "+{} in the last 30 days",
            pl(parity.ports_in_period, "port", "ports")
        )
    });
    let slots_period = (parity.slots_in_period > 0).then(|| {
        format!(
            "+{} in the last 30 days",
            pl(parity.slots_in_period, "slot", "slots")
        )
    });
    let catalog_sub = format!(
        "{} of the {}",
        pct_text(parity.catalog_pct),
        pl(parity.catalog_games, "catalog game", "catalog games")
    );
    let next_parity = parity
        .next_parity
        .map(|n| format!("Next: {n}% of platform slots covered"));

    let contributors: Vec<(String, String, String)> = data
        .parity
        .contributors
        .iter()
        .map(|c| {
            (
                c.login.clone(),
                c.name.clone(),
                pl(c.count, "platform slot added", "platform slots added"),
            )
        })
        .collect();

    let gaps: Vec<(String, String, String, String)> = data
        .gaps
        .iter()
        .map(|g| {
            let missing = g
                .missing
                .iter()
                .map(|m| m.label.clone())
                .collect::<Vec<_>>()
                .join(", ");
            (
                g.slug.clone(),
                g.name.clone(),
                pl(g.ports, "port", "ports"),
                missing,
            )
        })
        .collect();

    let origins: Vec<(String, String, String, String, f64, String)> = data
        .origins
        .iter()
        .map(|o| {
            (
                o.slug.clone(),
                o.name.clone(),
                o.game_count.to_string(),
                o.ported_count.to_string(),
                o.pct,
                pct_text(o.pct),
            )
        })
        .collect();

    let covered_text: Vec<(String, String, String)> = data
        .most_covered
        .iter()
        .map(|m| {
            (
                m.slug.clone(),
                m.name.clone(),
                format!("{} of 6 major platforms", m.covered),
            )
        })
        .collect();

    rsx! {
        Seo { title, description, path: "/stats".to_string() }
        div { class: "space-y-12",
            div { class: "space-y-8",
                Breadcrumb { items: vec![("Home".to_string(), "/".to_string()), ("Stats".to_string(), "/stats".to_string())] }
                div { class: "space-y-4",
                    h1 { class: H1, "Platform parity" }
                    p { class: "{LEAD} max-w-2xl",
                        "Every ported game should run on every major platform. Here is how close we are, what just changed, and which game to port next."
                    }
                }
                ParityMeter { parity: parity.clone() }
            }

            section { class: "grid gap-4 md:grid-cols-3", "aria-label": "Totals",
                div { class: "space-y-2 rounded-xl border bg-card p-4 shadow-sm",
                    p { class: "text-sm text-muted-foreground", "Native ports" }
                    p { class: "text-3xl font-semibold tabular", "{parity.ports}" }
                    if let Some(text) = ports_period.clone() {
                        p { class: "text-sm text-muted-foreground", "{text}" }
                    }
                }
                div { class: "space-y-2 rounded-xl border bg-card p-4 shadow-sm",
                    p { class: "text-sm text-muted-foreground", "Ported games" }
                    p { class: "text-3xl font-semibold tabular", "{parity.ported_games}" }
                    ProgressBar { pct: parity.catalog_pct, label: "Share of the catalog with a port".to_string() }
                    p { class: "text-sm text-muted-foreground", "{catalog_sub}" }
                }
                div { class: "space-y-2 rounded-xl border bg-card p-4 shadow-sm",
                    p { class: "text-sm text-muted-foreground", "Platform slots covered" }
                    p { class: "text-3xl font-semibold tabular",
                        "{parity.slots_covered} of {parity.slots_total}"
                    }
                    if let Some(text) = slots_period.clone() {
                        p { class: "text-sm text-muted-foreground", "{text}" }
                    }
                    if let Some(text) = next_parity.clone() {
                        p { class: "text-sm text-muted-foreground", "{text}" }
                    }
                }
            }

            section { class: "space-y-4", "aria-labelledby": "unlocks-heading",
                div { class: "space-y-1",
                    h2 { id: "unlocks-heading", class: H2, "What unlocks next" }
                    p { class: "text-sm text-muted-foreground",
                        "The counters closest to their next milestone."
                    }
                }
                UnlockStrip { unlocks: data.unlocks.clone() }
            }

            div { class: "grid gap-8 lg:grid-cols-3",
                section { class: "space-y-4 lg:col-span-2", "aria-labelledby": "progress-heading",
                    h2 { id: "progress-heading", class: H2, "Progress over time" }
                    ParityChart { history: data.history.clone() }
                }
                section { class: "space-y-4", "aria-labelledby": "milestones-heading",
                    h2 { id: "milestones-heading", class: H2, "Milestones" }
                    MilestoneList { milestones: data.milestones.clone() }
                }
            }

            section { class: "space-y-4", "aria-labelledby": "contributors-heading",
                div { class: "space-y-1",
                    h2 { id: "contributors-heading", class: H2, "Top contributors, last 30 days" }
                    p { class: "text-sm text-muted-foreground",
                        "Authors credited with the most platform slots they added. Each slot is a game running on a platform for the first time."
                    }
                }
                if contributors.is_empty() {
                    EmptyState { text: "No platform slots were added in the last 30 days yet.".to_string() }
                } else {
                    ol { class: "grid gap-2 md:grid-cols-2",
                        for (login, name, count) in contributors {
                            li { key: "{login}", class: "flex items-baseline justify-between gap-3 rounded-lg border bg-card px-4 py-3",
                                InternalLink { href: format!("/authors/{login}"), class: "font-medium hover:underline".to_string(), "{name}" }
                                span { class: "text-sm text-muted-foreground tabular", "{count}" }
                            }
                        }
                    }
                }
            }

            section { class: "space-y-4", "aria-labelledby": "board-heading",
                div { class: "space-y-1",
                    h2 { id: "board-heading", class: H2, "Platform board" }
                    p { class: "text-sm text-muted-foreground",
                        "Every platform ranked by how many ported games run on it. Change compares with 30 days ago."
                    }
                }
                if has_board {
                    div { class: TABLE_WRAP,
                        table { class: TABLE,
                            thead {
                                tr { class: TABLE_HEAD_ROW,
                                    th { class: TABLE_TH_NUM, scope: "col", "Rank" }
                                    th { class: TABLE_TH, scope: "col", "Platform" }
                                    th { class: TABLE_TH_NUM, scope: "col", "Games" }
                                    th { class: TABLE_TH, scope: "col", "Share" }
                                    th { class: TABLE_TH, scope: "col", "30 days" }
                                    th { class: TABLE_TH, scope: "col", "Action" }
                                }
                            }
                            tbody {
                                for row in board {
                                    tr { key: "{row.slug}", class: TABLE_ROW,
                                        td { class: "{TABLE_TD} text-right tabular", "{row.rank}" }
                                        td { class: TABLE_TD,
                                            span { class: "inline-flex items-center gap-2",
                                                TargetIcon { target: row.slug.clone() }
                                                "{row.label}"
                                            }
                                        }
                                        td { class: "{TABLE_TD} text-right tabular", "{row.games}" }
                                        td { class: TABLE_TD,
                                            div { class: "flex items-center gap-3",
                                                div { class: "w-24",
                                                    ProgressBar { pct: row.pct, label: "{row.label} share" }
                                                }
                                                span { class: "tabular", "{row.pct_text}" }
                                            }
                                        }
                                        td { class: TABLE_TD,
                                            div { class: "flex flex-wrap items-center gap-2",
                                                if let Some(prev) = row.prev_rank {
                                                    TrendChip { rank: row.rank, prev_rank: Some(prev) }
                                                } else {
                                                    span { class: "text-sm text-muted-foreground", "No history yet" }
                                                }
                                                if let Some(change) = row.change.clone() {
                                                    span { class: "text-sm text-muted-foreground tabular", "{change}" }
                                                }
                                            }
                                        }
                                        td { class: TABLE_TD,
                                            if row.gap_link {
                                                InternalLink { href: format!("/creators/platforms?missing={}", row.slug), class: format!("{BUTTON_OUTLINE} {BUTTON_XS}"), "Ports missing it" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else {
                    EmptyState { text: "No platform data yet.".to_string() }
                }
            }

            div { class: "grid gap-8 lg:grid-cols-2",
                section { class: "space-y-4", "aria-labelledby": "gaps-heading",
                    div { class: "space-y-1",
                        h2 { id: "gaps-heading", class: H2, "Closest to full parity" }
                        p { class: "text-sm text-muted-foreground",
                            "Ported games still missing a major platform. The most important games come first."
                        }
                    }
                    if gaps.is_empty() {
                        EmptyState { text: "Every ported game runs on all six major platforms.".to_string() }
                    } else {
                        ul { class: "space-y-2",
                            for (slug, name, ports, missing) in gaps {
                                li { key: "{slug}", class: "flex flex-wrap items-baseline justify-between gap-3 rounded-lg border bg-card px-4 py-3",
                                    div { class: "space-y-1",
                                        InternalLink { href: format!("/games/{slug}"), class: "font-medium hover:underline".to_string(), "{name}" }
                                        p { class: "text-sm text-muted-foreground", "Missing: {missing}" }
                                    }
                                    span { class: "text-sm text-muted-foreground tabular", "{ports}" }
                                }
                            }
                        }
                        InternalLink { href: "/creators/platforms".to_string(), class: format!("{BUTTON_OUTLINE} inline-flex"), "See every port missing a platform" }
                    }
                }

                section { class: "space-y-4", "aria-labelledby": "wanted-heading",
                    div { class: "space-y-1",
                        h2 { id: "wanted-heading", class: H2, "Most wanted, not yet ported" }
                        p { class: "text-sm text-muted-foreground",
                            "Catalog games with no native port. A port here adds a game to the parity count."
                        }
                    }
                    if data.wanted.is_empty() {
                        EmptyState { text: "Every catalog game has a port.".to_string() }
                    } else {
                        ul { class: "space-y-2",
                            for w in data.wanted.clone() {
                                li { key: "{w.slug}", class: "flex items-center justify-between gap-3 rounded-lg border bg-card px-4 py-3",
                                    InternalLink { href: format!("/games/{}", w.slug), class: "font-medium hover:underline".to_string(), "{w.name}" }
                                    InternalLink { href: format!("/submit?game={}", w.slug), class: format!("{BUTTON_OUTLINE} {BUTTON_XS}"), "Port it" }
                                }
                            }
                        }
                        InternalLink { href: "/creators/recompile".to_string(), class: format!("{BUTTON_OUTLINE} inline-flex"), "See every game to recompile" }
                    }
                }
            }

            div { class: "grid gap-8 lg:grid-cols-2",
                section { class: "space-y-4", "aria-labelledby": "covered-heading",
                    h2 { id: "covered-heading", class: H2, "Most covered" }
                    if covered_text.is_empty() {
                        EmptyState { text: "No ported game covers a major platform yet.".to_string() }
                    } else {
                        ul { class: "space-y-2",
                            for (slug, name, text) in covered_text {
                                li { key: "{slug}", class: "flex items-baseline justify-between gap-3 rounded-lg border bg-card px-4 py-3",
                                    InternalLink { href: format!("/games/{slug}"), class: "font-medium hover:underline".to_string(), "{name}" }
                                    span { class: "text-sm text-muted-foreground tabular", "{text}" }
                                }
                            }
                        }
                    }
                }

                section { class: "space-y-4", "aria-labelledby": "origins-heading",
                    div { class: "space-y-1",
                        h2 { id: "origins-heading", class: H2, "Original platforms, by share ported" }
                        p { class: "text-sm text-muted-foreground",
                            "Where a platform's own games have been recompiled, from the platform they came from."
                        }
                    }
                    if origins.is_empty() {
                        EmptyState { text: "No original platform has enough games to rank yet.".to_string() }
                    } else {
                        div { class: TABLE_WRAP,
                            table { class: TABLE,
                                thead {
                                    tr { class: TABLE_HEAD_ROW,
                                        th { class: TABLE_TH, scope: "col", "Platform" }
                                        th { class: TABLE_TH_NUM, scope: "col", "Games" }
                                        th { class: TABLE_TH_NUM, scope: "col", "Ported" }
                                        th { class: TABLE_TH, scope: "col", "Share" }
                                    }
                                }
                                tbody {
                                    for (slug, name, games, ported, pct, text) in origins {
                                        tr { key: "{slug}", class: TABLE_ROW,
                                            td { class: TABLE_TD,
                                                InternalLink { href: format!("/platforms/{slug}"), class: "font-medium hover:underline".to_string(), "{name}" }
                                            }
                                            td { class: "{TABLE_TD} text-right tabular", "{games}" }
                                            td { class: "{TABLE_TD} text-right tabular", "{ported}" }
                                            td { class: TABLE_TD,
                                                div { class: "flex items-center gap-3",
                                                    div { class: "w-24",
                                                        ProgressBar { pct, label: "{name} share" }
                                                    }
                                                    span { class: "tabular", "{text}" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            div { class: "grid gap-8 lg:grid-cols-2",
                section { class: "space-y-4", "aria-labelledby": "ports-leaders-heading",
                    h2 { id: "ports-leaders-heading", class: H2, "Most ports" }
                    if data.top_authors.is_empty() {
                        EmptyState { text: "No authors have published a port yet.".to_string() }
                    } else {
                        ol { class: "space-y-2",
                            for a in data.top_authors.clone() {
                                li { key: "{a.login}", class: "flex items-baseline justify-between gap-3 rounded-lg border bg-card px-4 py-3",
                                    InternalLink { href: format!("/authors/{}", a.login), class: "font-medium hover:underline".to_string(), "{a.name}" }
                                    span { class: "text-sm text-muted-foreground tabular", {pl(a.ports, "port", "ports")}, ", ", {pl(a.platforms, "platform", "platforms")} }
                                }
                            }
                        }
                    }
                }

                section { class: "space-y-4", "aria-labelledby": "platform-leaders-heading",
                    h2 { id: "platform-leaders-heading", class: H2, "Most platforms" }
                    if data.top_platform_authors.is_empty() {
                        EmptyState { text: "No author has covered more than one platform yet.".to_string() }
                    } else {
                        ol { class: "space-y-2",
                            for a in data.top_platform_authors.clone() {
                                li { key: "{a.login}", class: "flex items-baseline justify-between gap-3 rounded-lg border bg-card px-4 py-3",
                                    InternalLink { href: format!("/authors/{}", a.login), class: "font-medium hover:underline".to_string(), "{a.name}" }
                                    span { class: "text-sm text-muted-foreground tabular", {pl(a.platforms, "platform", "platforms")}, ", ", {pl(a.ports, "port", "ports")} }
                                }
                            }
                        }
                    }
                }
            }

            section { class: "space-y-2 text-sm text-muted-foreground", "aria-labelledby": "method-heading",
                h2 { id: "method-heading", class: H2, "How this is counted" }
                p {
                    "Parity counts the six major platforms (Windows, macOS, Linux, web, Android and iOS) across every game with at least one native port. Decompilations are not counted. A game counts on a platform when one of its ports runs there."
                }
                p {
                    "Trends compare each day with the stored snapshot from 30 days earlier. Dates are when each project's GitHub repository was created; projects without a GitHub date use the day they were added. Days before snapshots began are rebuilt from those dates using current platform lists, so ports that were later hidden or retargeted are not reflected in those days."
                }
            }
        }
    }
}
