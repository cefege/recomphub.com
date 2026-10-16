//! Maintenance badge and panel for a port, built from the GitHub maintenance signals.

use dioxus::prelude::*;
use recomphub_shared::dto::MaintenanceDto;
use recomphub_shared::maintenance::MaintenanceLevel;

use super::styles::{
    BADGE, BADGE_DESTRUCTIVE, BADGE_OUTLINE, BADGE_SECONDARY, BADGE_SUCCESS, BADGE_WARNING,
    CARD_SURFACE,
};

/// Height of the sparkline in SVG units; the width is fixed by the bar layout below.
const SPARK_HEIGHT: i32 = 32;
/// Bars per sparkline, one per week.
const SPARK_BAR_WIDTH: i32 = 6;
const SPARK_BAR_STEP: i32 = 10;

fn badge_variant(level: MaintenanceLevel) -> &'static str {
    match level {
        MaintenanceLevel::Active => BADGE_SUCCESS,
        MaintenanceLevel::Maintained => BADGE_SECONDARY,
        MaintenanceLevel::Slowing => BADGE_WARNING,
        MaintenanceLevel::Dormant => BADGE_OUTLINE,
        MaintenanceLevel::Archived => BADGE_DESTRUCTIVE,
    }
}

/// `1 commit` or `12 commits`.
fn counted(n: i64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// Level as a small badge. The description is the tooltip, so the meaning is one hover away.
#[component]
pub fn MaintenanceBadge(level: MaintenanceLevel) -> Element {
    let class = format!("{BADGE} {}", badge_variant(level));
    rsx! {
        span { class: "{class}", title: "{level.description()}",
            span { class: "size-1.5 shrink-0 rounded-full bg-current", "aria-hidden": "true" }
            "{level.label()}"
        }
    }
}

/// Full maintenance block for the port page: level, counts and the 12-week sparkline.
#[component]
pub fn MaintenancePanel(maintenance: MaintenanceDto, weekly_commits: Vec<i32>) -> Element {
    let level = maintenance.level;
    let total: i32 = weekly_commits.iter().sum();
    let weeks_text = format!(
        "{} in the last {}",
        counted(i64::from(total), "commit", "commits"),
        counted(weekly_commits.len() as i64, "week", "weeks"),
    );
    rsx! {
        div { class: "{CARD_SURFACE} grid gap-6 p-6",
            div { class: "flex flex-wrap items-center gap-3",
                MaintenanceBadge { level }
                p { class: "text-sm text-muted-foreground", "{level.description()}" }
            }
            dl { class: "grid grid-cols-2 gap-4 sm:grid-cols-4",
                MaintenanceStat {
                    label: "Commits in 30 days",
                    value: maintenance.commits_30d.to_string(),
                }
                MaintenanceStat {
                    label: "Commits in 90 days",
                    value: maintenance.commits_90d.to_string(),
                }
                MaintenanceStat {
                    label: "Contributors in 6 months",
                    value: maintenance.committers_180d.to_string(),
                }
                MaintenanceStat {
                    label: "Releases in 12 months",
                    value: maintenance.releases_365d.to_string(),
                }
            }
            if !weekly_commits.is_empty() {
                div { class: "grid gap-2",
                    Sparkline { weeks: weekly_commits.clone() }
                    p { class: "text-sm text-muted-foreground",
                        "{weeks_text}"
                    }
                }
            }
            p { class: "text-xs text-muted-foreground",
                "Counts cover the default branch. Contributors are counted from the 100 most recent commits."
            }
        }
    }
}

#[component]
fn MaintenanceStat(label: String, value: String) -> Element {
    rsx! {
        div { class: "grid gap-1",
            dt { class: "text-sm text-muted-foreground", "{label}" }
            dd { class: "tabular text-2xl font-semibold", "{value}" }
        }
    }
}

/// Inline bar chart of weekly commits. Bars use `currentColor`, so the caller sets the colour.
#[component]
fn Sparkline(weeks: Vec<i32>) -> Element {
    let max = weeks.iter().copied().max().unwrap_or(0).max(1);
    let total: i32 = weeks.iter().sum();
    let bars: Vec<(i32, i32)> = weeks
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let h = v.max(0) * SPARK_HEIGHT / max;
            (i as i32 * SPARK_BAR_STEP, h)
        })
        .collect();
    let view_width = weeks.len().saturating_sub(1) as i32 * SPARK_BAR_STEP + SPARK_BAR_WIDTH;
    let view_box = format!("0 0 {view_width} {SPARK_HEIGHT}");
    let label = format!(
        "Commits per week over the last {}: {} in total",
        counted(weeks.len() as i64, "week", "weeks"),
        counted(i64::from(total), "commit", "commits")
    );
    rsx! {
        svg { class: "h-8 w-full max-w-60", view_box: "{view_box}", preserve_aspect_ratio: "none",
            role: "img", "aria-label": "{label}",
            for (x, h) in bars {
                if h > 0 {
                    rect {
                        x: "{x}",
                        y: "{SPARK_HEIGHT - h}",
                        width: "{SPARK_BAR_WIDTH}",
                        height: "{h}",
                        rx: "1",
                        class: "fill-success",
                    }
                } else {
                    rect {
                        x: "{x}",
                        y: "{SPARK_HEIGHT - 1}",
                        width: "{SPARK_BAR_WIDTH}",
                        height: "1",
                        class: "fill-muted-foreground/40",
                    }
                }
            }
        }
    }
}
