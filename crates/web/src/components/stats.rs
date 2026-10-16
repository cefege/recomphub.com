//! Platform coverage widgets: the home teaser, the `/stats` hub and author achievements.
//! Charts are inline SVG drawn in `currentColor` or theme variables. No chart library.

use chrono::NaiveDate;
use dioxus::prelude::*;
use recomphub_shared::dto::{
    AchievementDto, HistoryPointDto, MilestoneDto, ParityDto, ParityTargetDto, UnlockDto,
};

use crate::components::styles::{
    BADGE, BADGE_OUTLINE, BUTTON, BUTTON_OUTLINE, BUTTON_SM, H2, TABLE, TABLE_HEAD_ROW, TABLE_ROW,
    TABLE_TD, TABLE_TH, TABLE_TH_NUM,
};
use crate::components::ui::TargetIcon;

fn is_major_slug(slug: &str) -> bool {
    recomphub_shared::MAJOR_TARGETS.contains(&slug)
}

/// A count with its noun, pluralised: `1 game`, `3 games`.
fn pl(n: i64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("{n} {one}")
    } else {
        format!("{n} {many}")
    }
}

fn day_label(d: NaiveDate) -> String {
    d.format("%-d %b %Y").to_string()
}

/// A horizontal bar. `pct` is clamped to 0..=100.
#[component]
pub fn ProgressBar(pct: f64, label: String, #[props(default)] class: String) -> Element {
    let w = pct.clamp(0.0, 100.0);
    let width = format!("width: {w:.2}%");
    let now = format!("{w:.0}");
    rsx! {
        div {
            class: "h-2 w-full overflow-hidden rounded-full bg-muted {class}",
            role: "progressbar",
            "aria-label": "{label}",
            "aria-valuemin": "0",
            "aria-valuemax": "100",
            "aria-valuenow": "{now}",
            div { class: "h-full rounded-full bg-success", style: "{width}" }
        }
    }
}

/// A ring filled clockwise from 12 o'clock to `pct` (0..=100), with the percentage in the middle.
/// `size` is the diameter in pixels.
#[component]
pub fn Donut(pct: f64, label: String, #[props(default = 64)] size: u32) -> Element {
    let p = pct.clamp(0.0, 100.0);
    // r = 15.915… makes the circumference 100, so the dash length is the percentage.
    let dash = format!("{p:.2} {:.2}", 100.0 - p);
    let now = format!("{p:.0}");
    let text = if size >= 96 { "text-2xl" } else { "text-sm" };
    rsx! {
        div {
            class: "relative shrink-0",
            style: "width: {size}px; height: {size}px",
            role: "progressbar",
            "aria-label": "{label}",
            "aria-valuemin": "0",
            "aria-valuemax": "100",
            "aria-valuenow": "{now}",
            svg { class: "size-full -rotate-90", view_box: "0 0 36 36", "aria-hidden": "true",
                circle { cx: "18", cy: "18", r: "15.9155", fill: "none", stroke_width: "3.5", class: "stroke-muted" }
                if p > 0.0 {
                    circle {
                        cx: "18", cy: "18", r: "15.9155", fill: "none", stroke_width: "3.5",
                        stroke_linecap: "round", stroke_dasharray: "{dash}", class: "stroke-success",
                    }
                }
            }
            span { class: "absolute inset-0 flex items-center justify-center font-semibold tabular {text}", "{now}%" }
        }
    }
}

/// Headline parity: the percentage, the slots behind it and the change over 30 days.
#[component]
pub fn ParityMeter(parity: ParityDto) -> Element {
    let trend = parity.pct_30d.map(|old| {
        let delta = parity.pct - old;
        if delta.abs() < 0.05 {
            "No change in the last 30 days".to_string()
        } else {
            format!("{delta:+.1} points in the last 30 days")
        }
    });
    let slots = format!(
        "{} of {} platform slots filled across {}",
        parity.slots_covered,
        parity.slots_total,
        pl(parity.ported_games, "ported game", "ported games"),
    );
    rsx! {
        div { class: "flex flex-wrap items-center gap-5",
            Donut { pct: parity.pct, label: "Platform parity across the six major platforms", size: 112 }
            div { class: "space-y-1",
                p { class: "text-lg font-semibold", "Platform parity" }
                p { class: "text-sm text-muted-foreground tabular", "{slots}" }
                if let Some(t) = trend {
                    p { class: "text-sm tabular text-muted-foreground", "{t}" }
                }
            }
        }
    }
}

/// One major platform, precomputed for rendering.
struct MajorRow {
    slug: String,
    label: String,
    pct: f64,
    count: String,
}

/// Home teaser: headline parity, the next milestone and the major platforms, with a link to `/stats`.
#[component]
pub fn PlatformCoverage(coverage: ParityDto, #[props(default = 6)] limit: usize) -> Element {
    let ported = coverage.ported_games;
    let games_word = if ported == 1 { "game" } else { "games" };
    let rows: Vec<MajorRow> = coverage
        .targets
        .iter()
        .filter(|t| is_major_slug(&t.slug))
        .take(limit)
        .map(|t: &ParityTargetDto| MajorRow {
            slug: t.slug.clone(),
            label: t.label.clone(),
            pct: t.pct,
            count: format!("{} of {ported} {games_word}", t.games),
        })
        .collect();
    let to_next = coverage.next_parity.map(|target| {
        let need = (target as f64 * coverage.slots_total as f64 / 100.0).ceil() as i64;
        let to_go = (need - coverage.slots_covered).max(0);
        format!(
            "Next milestone: {target}% platform parity, {} to go",
            pl(to_go, "slot", "slots"),
        )
    });
    rsx! {
        section { class: "space-y-4", "aria-labelledby": "platform-coverage",
            div { class: "flex flex-wrap items-end justify-between gap-4",
                div { class: "space-y-1",
                    h2 { id: "platform-coverage", class: H2, "Platform coverage" }
                    p { class: "text-sm text-muted-foreground",
                        "How many of our ported games run on each major platform."
                    }
                }
                a { href: "/stats", class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM}", "Full platform board" }
            }
            ParityMeter { parity: coverage.clone() }
            if let Some(text) = to_next {
                p { class: "text-sm font-medium", "{text}" }
            }
            ul { class: "grid grid-cols-2 gap-4 sm:grid-cols-3 lg:grid-cols-6",
                for row in rows {
                    li { key: "{row.slug}", class: "flex flex-col items-center gap-2 rounded-xl border bg-card p-4 text-center shadow-sm",
                        Donut { pct: row.pct, label: "{row.label} coverage", size: 72 }
                        span { class: "flex items-center gap-1.5 text-sm font-medium",
                            TargetIcon { target: row.slug.clone() }
                            "{row.label}"
                        }
                        span { class: "text-xs tabular text-muted-foreground", "{row.count}" }
                    }
                }
            }
        }
    }
}

/// Line chart of parity over time, with a data table behind a disclosure.
#[component]
pub fn ParityChart(history: Vec<HistoryPointDto>) -> Element {
    if history.len() < 2 {
        return rsx! {
            p { class: "text-sm text-muted-foreground", "The chart appears once two days have been recorded." }
        };
    }
    const W: f64 = 640.0;
    const H: f64 = 220.0;
    const PAD: f64 = 28.0;
    let first = history[0].day;
    let last_day = history[history.len() - 1].day;
    let span = (last_day - first).num_days().max(1) as f64;
    let x_of = |d: NaiveDate| PAD + (d - first).num_days() as f64 / span * (W - 2.0 * PAD);
    let y_of = |pct: f64| H - PAD - pct.clamp(0.0, 100.0) / 100.0 * (H - 2.0 * PAD);
    let pts: Vec<(f64, f64)> = history.iter().map(|p| (x_of(p.day), y_of(p.pct))).collect();
    let points = pts
        .iter()
        .map(|(x, y)| format!("{x:.1},{y:.1}"))
        .collect::<Vec<_>>()
        .join(" ");
    let (lx, ly) = pts[pts.len() - 1];
    let end_x = format!("{lx:.1}");
    let end_y = format!("{ly:.1}");
    let view = format!("0 0 {W} {H}");
    let grid_x2 = format!("{:.1}", W - PAD);
    let grid_x1 = format!("{PAD:.1}");
    let y100 = format!("{:.1}", y_of(100.0));
    let y50 = format!("{:.1}", y_of(50.0));
    let y0 = format!("{:.1}", y_of(0.0));
    let label_x = format!("{:.1}", PAD + 4.0);
    let first_label = day_label(first);
    let last_label = day_label(last_day);
    let aria = format!(
        "Platform parity from {first_label} to {last_label}, now {:.0}%",
        history[history.len() - 1].pct
    );
    let table_rows: Vec<(String, String, String, String)> = history
        .iter()
        .rev()
        .take(60)
        .map(|p| {
            (
                day_label(p.day),
                format!("{:.1}%", p.pct),
                p.slots.to_string(),
                p.ported_games.to_string(),
            )
        })
        .collect();
    rsx! {
        figure { class: "space-y-3",
            svg {
                class: "w-full text-muted-foreground",
                view_box: "{view}",
                role: "img",
                "aria-label": "{aria}",
                line { x1: "{grid_x1}", y1: "{y100}", x2: "{grid_x2}", y2: "{y100}", stroke: "currentColor", stroke_opacity: "0.2" }
                line { x1: "{grid_x1}", y1: "{y50}", x2: "{grid_x2}", y2: "{y50}", stroke: "currentColor", stroke_opacity: "0.2" }
                line { x1: "{grid_x1}", y1: "{y0}", x2: "{grid_x2}", y2: "{y0}", stroke: "currentColor", stroke_opacity: "0.2" }
                text { x: "{label_x}", y: "{y100}", style: "font-size: 11px; fill: currentColor", "100%" }
                text { x: "{label_x}", y: "{y50}", style: "font-size: 11px; fill: currentColor", "50%" }
                text { x: "{label_x}", y: "{y0}", style: "font-size: 11px; fill: currentColor", "0%" }
                polyline {
                    points: "{points}",
                    fill: "none",
                    stroke_width: "2",
                    stroke_linejoin: "round",
                    style: "stroke: var(--success)",
                }
                circle { cx: "{end_x}", cy: "{end_y}", r: "4", style: "fill: var(--success)" }
            }
            div { class: "flex justify-between text-xs tabular text-muted-foreground",
                span { "{first_label}" }
                span { "{last_label}" }
            }
            details { class: "text-sm",
                summary { class: "cursor-pointer text-muted-foreground hover:text-foreground", "Chart data, last 60 days" }
                div { class: "mt-3 overflow-x-auto",
                    table { class: TABLE,
                        thead {
                            tr { class: TABLE_HEAD_ROW,
                                th { class: TABLE_TH, scope: "col", "Day" }
                                th { class: TABLE_TH_NUM, scope: "col", "Parity" }
                                th { class: TABLE_TH_NUM, scope: "col", "Slots filled" }
                                th { class: TABLE_TH_NUM, scope: "col", "Ported games" }
                            }
                        }
                        tbody {
                            for (day, pct, slots, games) in table_rows {
                                tr { key: "{day}", class: TABLE_ROW,
                                    td { class: TABLE_TD, "{day}" }
                                    td { class: "{TABLE_TD} text-right tabular", "{pct}" }
                                    td { class: "{TABLE_TD} text-right tabular", "{slots}" }
                                    td { class: "{TABLE_TD} text-right tabular", "{games}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One counter on the "what unlocks next" strip, precomputed for rendering.
struct UnlockCard {
    key: String,
    label: String,
    slug: Option<String>,
    pct: f64,
    to_go: String,
    next: String,
}

/// The counters closest to their next milestone.
#[component]
pub fn UnlockStrip(unlocks: Vec<UnlockDto>) -> Element {
    let cards: Vec<UnlockCard> = unlocks
        .into_iter()
        .map(|u| {
            let (label, noun, key) = match (&u.target, u.kind.as_str()) {
                (Some(t), _) => (format!("{} ports", t.label), "ports", t.slug.clone()),
                (None, "slots") => (
                    "Platform slots".to_string(),
                    "platform slots",
                    "slots".to_string(),
                ),
                _ => ("All ports".to_string(), "ports", "ports".to_string()),
            };
            UnlockCard {
                key,
                label,
                slug: u.target.as_ref().map(|t| t.slug.clone()),
                pct: u.current as f64 / u.next as f64 * 100.0,
                to_go: format!("{} to go", u.to_go),
                next: format!("Next milestone: {} {noun}", u.next),
            }
        })
        .collect();
    if cards.is_empty() {
        return rsx! {};
    }
    rsx! {
        ul { class: "grid gap-4 md:grid-cols-3",
            for card in cards {
                li { key: "{card.key}", class: "space-y-3 rounded-xl border bg-card p-4 shadow-sm",
                    div { class: "flex items-center gap-2 text-sm font-medium",
                        if let Some(slug) = card.slug.clone() {
                            TargetIcon { target: slug }
                        }
                        "{card.label}"
                    }
                    ProgressBar { pct: card.pct, label: "{card.label} towards the next milestone" }
                    div { class: "flex items-baseline justify-between gap-3 text-sm",
                        span { class: "font-semibold tabular", "{card.to_go}" }
                        span { class: "text-muted-foreground", "{card.next}" }
                    }
                }
            }
        }
    }
}

/// One crossed milestone, precomputed for rendering.
struct MilestoneRow {
    key: String,
    text: String,
    day: String,
    iso: String,
}

/// Milestones reached, newest first.
#[component]
pub fn MilestoneList(milestones: Vec<MilestoneDto>) -> Element {
    let rows: Vec<MilestoneRow> = milestones
        .into_iter()
        .map(|m| {
            let slug = m
                .target
                .as_ref()
                .map(|t| t.slug.clone())
                .unwrap_or_default();
            let text = match (&m.target, m.kind.as_str()) {
                (Some(t), _) => format!("{} {} ports", m.threshold, t.label),
                (None, "slots") => format!("{} platform slots", m.threshold),
                _ => format!("{} ports", m.threshold),
            };
            MilestoneRow {
                key: format!("{}-{}-{}", m.kind, slug, m.threshold),
                text,
                day: day_label(m.day),
                iso: m.day.to_string(),
            }
        })
        .collect();
    if rows.is_empty() {
        return rsx! {
            p { class: "text-sm text-muted-foreground", "No milestones yet. The first ones appear as ports land." }
        };
    }
    rsx! {
        ol { class: "space-y-2 text-sm",
            for row in rows {
                li { key: "{row.key}", class: "flex items-baseline justify-between gap-4 border-b pb-2",
                    span { class: "font-medium", "{row.text}" }
                    time { class: "shrink-0 text-muted-foreground tabular", datetime: "{row.iso}", "{row.day}" }
                }
            }
        }
    }
}

/// Small award glyph for achievement badges.
#[component]
fn AwardIcon() -> Element {
    rsx! {
        svg { class: "h-3.5 w-3.5", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round", "aria-hidden": "true", "focusable": "false",
            circle { cx: "12", cy: "8", r: "6" }
            path { d: "M15.477 12.89 17 22l-5-3-5 3 1.523-9.11" }
        }
    }
}

/// Achievement badges for an author. Renders nothing when the author has none.
#[component]
pub fn AuthorAchievements(achievements: Vec<AchievementDto>) -> Element {
    if achievements.is_empty() {
        return rsx! {};
    }
    rsx! {
        section { class: "space-y-3", "aria-labelledby": "author-achievements",
            h2 { id: "author-achievements", class: "text-sm font-semibold", "Achievements" }
            ul { class: "flex flex-wrap gap-2",
                for a in achievements {
                    li { key: "{a.key}", title: "{a.description}", class: "{BADGE} {BADGE_OUTLINE} gap-1.5 py-1",
                        AwardIcon {}
                        "{a.title}"
                    }
                }
            }
        }
    }
}
