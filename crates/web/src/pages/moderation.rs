//! Moderation pages: the review queue and the audit log. Both are moderator-only, so
//! their data loads after hydration and never enters the SSR HTML.

use chrono::{DateTime, Utc};
use dioxus::prelude::*;
use recomphub_shared::{GameChoice, LogItem, Page, PortKind, QueueItem, ReviewOverrides};
use serde_json::Value;

use crate::api;
use crate::components::styles::*;
use crate::components::{Breadcrumb, Chip, EmptyState, Pager, RoleGate, Seo, parse_page};
use crate::routes::Route;

#[component]
pub fn ModerationQueue() -> Element {
    rsx! {
        Seo {
            title: "Moderation queue | RecompHub".to_string(),
            description: "Submissions waiting for review.".to_string(),
            path: "/moderation".to_string(),
            noindex: true,
        }
        div { class: "mx-auto max-w-5xl space-y-8",
            Breadcrumb { items: vec![
                ("Home".to_string(), "/".to_string()),
                ("Moderation".to_string(), "/moderation".to_string()),
            ] }
            div { class: "mt-4 flex flex-wrap items-end justify-between gap-4",
                h1 { class: H1, "Moderation queue" }
                Link {
                    class: "text-sm font-semibold underline underline-offset-4",
                    to: Route::ModerationLog { page: "1".to_string() },
                    "Review log"
                }
            }
            RoleGate { admin_only: false, QueueList {} }
        }
    }
}

#[component]
fn QueueList() -> Element {
    let mut queue = use_signal(|| None::<Result<Vec<QueueItem>, String>>);
    let mut reload = use_signal(|| 0u32);
    let mut tab = use_signal(|| "member".to_string());
    use_effect(move || {
        let _ = reload();
        spawn(async move {
            let result = api::moderation_queue().await.map_err(|e| e.to_string());
            queue.set(Some(result));
        });
    });

    let state = queue.read().clone();
    match state {
        None => rsx! {
            div { class: "mt-8 space-y-4",
                for i in 0..3 {
                    div { key: "{i}", class: "skeleton h-40 rounded-xl", "aria-hidden": "true" }
                }
            }
        },
        Some(Err(message)) => rsx! {
            p { class: "mt-8 text-sm text-destructive", "Could not load the queue: {message}" }
        },
        Some(Ok(items)) if items.is_empty() => rsx! {
            div { class: "mt-8",
                EmptyState { text: "Nothing is waiting for review.".to_string() }
            }
        },
        Some(Ok(items)) => {
            let member_count = items.iter().filter(|i| i.source == "user").count();
            let found_count = items.len() - member_count;
            let active = tab();
            let visible: Vec<QueueItem> = items
                .into_iter()
                .filter(|i| (i.source == "user") == (active == "member"))
                .collect();
            rsx! {
                div { role: "tablist", class: "{TABS_LIST} mt-8",
                    button { r#type: "button", role: "tab", "aria-selected": if active == "member" { "true" } else { "false" },
                        class: if active == "member" { "{TAB} {TAB_ACTIVE}" } else { "{TAB}" },
                        onclick: move |_| tab.set("member".to_string()),
                        "Member submissions ({member_count})"
                    }
                    button { r#type: "button", role: "tab", "aria-selected": if active == "found" { "true" } else { "false" },
                        class: if active == "found" { "{TAB} {TAB_ACTIVE}" } else { "{TAB}" },
                        onclick: move |_| tab.set("found".to_string()),
                        "Found automatically ({found_count})"
                    }
                }
                if visible.is_empty() {
                    div { class: "mt-6",
                        EmptyState { text: "Nothing here right now.".to_string() }
                    }
                } else {
                    ul { class: "mt-6 space-y-4",
                        for item in visible {
                            li { key: "{item.id}",
                                QueueCard { item, on_done: move |_| reload.set(reload() + 1) }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn QueueCard(item: QueueItem, on_done: EventHandler<()>) -> Element {
    let diff = field_rows(&item.payload_json, item.before_json.as_deref());
    let submitter = item
        .submitter_name
        .clone()
        .unwrap_or_else(|| "automated".to_string());
    let approved = item.submitter_approved.unwrap_or(0);
    let when = stamp(item.created_at);
    let diff_view = match diff {
        Err(message) => rsx! {
            p { class: "mt-4 text-sm text-destructive", "Could not read the submitted data: {message}" }
        },
        Ok(rows) if rows.is_empty() => rsx! {
            p { class: "mt-4 text-sm text-muted-foreground", "No fields to compare." }
        },
        Ok(rows) => rsx! {
            div { class: "mt-4 overflow-x-auto",
                table { class: "w-full text-left text-sm",
                    thead {
                        tr { class: "text-muted-foreground",
                            th { scope: "col", class: "py-2 pr-4 font-semibold", "Field" }
                            th { scope: "col", class: "py-2 pr-4 font-semibold", "Before" }
                            th { scope: "col", class: "py-2 font-semibold", "After" }
                        }
                    }
                    tbody {
                        for row in rows {
                            tr {
                                key: "{row.key}",
                                class: if row.changed { "border-t font-semibold" } else { "border-t text-muted-foreground" },
                                th { scope: "row", class: "py-2 pr-4 font-normal", "{row.key}" }
                                td { class: "py-2 pr-4 break-all", "{row.before}" }
                                td { class: "py-2 break-all", "{row.after}" }
                            }
                        }
                    }
                }
            }
        },
    };
    let needs_match = item.needs_game_match || item.kind == "candidate";

    rsx! {
        article { class: "{CARD_SURFACE} p-5",
            div { class: "flex flex-wrap items-center gap-2",
                Chip { "{item.kind}" }
                Chip { "{item.source}" }
                if item.submitter_is_author {
                    Chip { class: "text-success", "Submitter is an author" }
                }
                if item.needs_game_match {
                    Chip { class: "text-warning", "Needs a game match" }
                }
            }
            p { class: "mt-3 text-sm text-muted-foreground", "#{item.id} · {submitter} · {approved} approved · {when}" }
            {diff_view}
            Decision {
                item_id: item.id,
                kind: item.kind.clone(),
                needs_match,
                on_done,
            }
        }
    }
}

/// Approve and reject controls. Candidates and unmatched imports need a game first.
#[component]
fn Decision(item_id: i64, kind: String, needs_match: bool, on_done: EventHandler<()>) -> Element {
    let is_candidate = kind == "candidate";
    let mut query = use_signal(String::new);
    let mut choices = use_signal(Vec::<GameChoice>::new);
    let mut picked = use_signal(|| None::<Pick>);
    let mut kind_choice = use_signal(|| PortKind::ALL[0].as_str().to_string());
    let mut note = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    let search = move |remote: bool| {
        let text = query();
        spawn(async move {
            let found = if remote {
                api::search_igdb(text).await
            } else {
                api::search_games(text).await
            };
            match found {
                Ok(list) => choices.set(list),
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };

    let mut approve = move || {
        let chosen = picked();
        if needs_match && chosen.is_none() {
            error.set(Some("Pick a game before approving.".to_string()));
            return;
        }
        let overrides = ReviewOverrides {
            game_id: chosen.as_ref().and_then(|p| p.game_id),
            igdb_id: chosen.as_ref().and_then(|p| p.igdb_id),
            kind: is_candidate.then(|| kind_choice.cloned()),
        };
        busy.set(true);
        error.set(None);
        spawn(async move {
            match api::approve(item_id, overrides).await {
                Ok(()) => on_done.call(()),
                Err(e) => {
                    error.set(Some(e.to_string()));
                    busy.set(false);
                }
            }
        });
    };

    let mut reject = move || {
        let text = note();
        busy.set(true);
        error.set(None);
        spawn(async move {
            match api::reject(item_id, text).await {
                Ok(()) => on_done.call(()),
                Err(e) => {
                    error.set(Some(e.to_string()));
                    busy.set(false);
                }
            }
        });
    };

    let busy_now = busy();
    let error_text = error().unwrap_or_default();
    let note_text = note();
    let query_text = query();
    let current_kind = kind_choice();
    let kind_rows: Vec<KindRow> = PortKind::ALL
        .iter()
        .map(|kind| KindRow {
            value: kind.as_str(),
            label: kind.label(),
        })
        .collect();
    let picked_now = picked();
    let picked_text = picked_now
        .as_ref()
        .map(|p| format!("Selected: {}", p.label))
        .unwrap_or_default();
    let found = choices();

    rsx! {
        div { class: "mt-5 space-y-4 border-t pt-5",
            if needs_match {
                div { class: "space-y-3",
                    p { class: "text-sm font-semibold", "Which game is this for?" }
                    div { class: "flex flex-wrap gap-2",
                        input {
                            r#type: "search",
                            class: INPUT,
                            placeholder: "Game title",
                            "aria-label": "Game title",
                            value: "{query_text}",
                            oninput: move |e| query.set(e.value()),
                        }
                        button { r#type: "button", class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD}", disabled: busy_now, onclick: move |_| search(false), "Search library" }
                        button { r#type: "button", class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD}", disabled: busy_now, onclick: move |_| search(true), "Search all games" }
                    }
                    p { class: "text-sm text-success", "{picked_text}" }
                    ul { class: "grid gap-2 sm:grid-cols-2",
                        for (index, choice) in found.into_iter().enumerate() {
                            li { key: "{index}",
                                ChoiceButton {
                                    choice,
                                    picked: picked_now.clone(),
                                    on_pick: move |p| picked.set(Some(p)),
                                }
                            }
                        }
                    }
                }
            }
            if is_candidate {
                label { class: "block space-y-1 text-sm font-semibold",
                    "Port kind"
                    select { class: INPUT, onchange: move |e| kind_choice.set(e.value()),
                        for entry in kind_rows {
                            option {
                                key: "{entry.value}",
                                value: "{entry.value}",
                                selected: current_kind == entry.value,
                                "{entry.label}"
                            }
                        }
                    }
                }
            }
            label { class: "block space-y-1 text-sm font-semibold",
                "Note for the submitter (used when rejecting)"
                input {
                    r#type: "text",
                    maxlength: "1000",
                    class: INPUT,
                    value: "{note_text}",
                    oninput: move |e| note.set(e.value()),
                }
            }
            if !error_text.is_empty() {
                p { class: "text-sm text-destructive", "{error_text}" }
            }
            p { class: "text-sm text-muted-foreground", "Check the ground rules: no game data, ROMs, firmware or keys in the repo or releases." }
            div { class: "flex flex-wrap gap-3",
                button { r#type: "button", class: "{BUTTON} {BUTTON_PRIMARY} {BUTTON_MD}", disabled: busy_now, onclick: move |_| approve(), "Approve" }
                button { r#type: "button", class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD}", disabled: busy_now, onclick: move |_| reject(), "Reject" }
            }
        }
    }
}

/// A game chosen in the picker. Local games carry `game_id`; IGDB results carry `igdb_id`.
#[derive(Clone, PartialEq)]
struct Pick {
    game_id: Option<i64>,
    igdb_id: Option<i64>,
    label: String,
}

struct KindRow {
    value: &'static str,
    label: &'static str,
}

#[component]
fn ChoiceButton(choice: GameChoice, picked: Option<Pick>, on_pick: EventHandler<Pick>) -> Element {
    let pick = Pick {
        game_id: choice.game_id,
        igdb_id: choice.igdb_id,
        label: choice.name.clone(),
    };
    let selected = picked
        .as_ref()
        .is_some_and(|p| p.game_id == pick.game_id && p.igdb_id == pick.igdb_id);
    let year = choice
        .release_year
        .map(|y| y.to_string())
        .unwrap_or_else(|| "year unknown".to_string());
    let source = if choice.game_id.is_some() {
        "In RecompHub"
    } else {
        "New game"
    };
    let class = if selected {
        "w-full rounded-md border border-primary bg-accent px-3 py-2 text-left text-sm"
    } else {
        "w-full rounded-md border bg-background px-3 py-2 text-left text-sm hover:bg-accent"
    };
    rsx! {
        button {
            r#type: "button",
            class,
            "aria-pressed": if selected { "true" } else { "false" },
            onclick: move |_| on_pick.call(pick.clone()),
            span { class: "block font-semibold", "{choice.name}" }
            span { class: "block text-xs text-muted-foreground", "{year} · {source}" }
        }
    }
}

#[component]
pub fn ModerationLog(page: String) -> Element {
    let page_number = parse_page(&page);
    rsx! {
        Seo {
            title: "Moderation log | RecompHub".to_string(),
            description: "Applied and reverted submissions, newest first.".to_string(),
            path: "/moderation/log".to_string(),
            noindex: true,
        }
        div { class: "mx-auto max-w-5xl space-y-8",
            Breadcrumb { items: vec![
                ("Home".to_string(), "/".to_string()),
                ("Moderation".to_string(), "/moderation".to_string()),
                ("Log".to_string(), "/moderation/log".to_string()),
            ] }
            h1 { class: "mt-4 {H1}", "Moderation log" }
            RoleGate { admin_only: false, LogList { page: page_number } }
        }
    }
}

#[component]
fn LogList(page: i64) -> Element {
    let mut log = use_signal(|| None::<Result<Page<LogItem>, String>>);
    let mut reload = use_signal(|| 0u32);
    let mut notice = use_signal(|| None::<String>);
    use_effect(use_reactive!(|(page,)| {
        let _ = reload();
        spawn(async move {
            let result = api::moderation_log(page).await.map_err(|e| e.to_string());
            log.set(Some(result));
        });
    }));

    let revert = move |id: i64| {
        spawn(async move {
            match api::revert(id).await {
                Ok(()) => {
                    notice.set(None);
                    reload.set(reload() + 1);
                }
                Err(e) => notice.set(Some(e.to_string())),
            }
        });
    };

    let notice_text = notice().unwrap_or_default();
    let state = log.read().clone();
    match state {
        None => rsx! {
            div { class: "mt-8 space-y-3",
                for i in 0..3 {
                    div { key: "{i}", class: "skeleton h-12 rounded-xl", "aria-hidden": "true" }
                }
            }
        },
        Some(Err(message)) => rsx! {
            p { class: "mt-8 text-sm text-destructive", "Could not load the log: {message}" }
        },
        Some(Ok(data)) => {
            let page_no = data.page;
            let pages = data.pages;
            let rows = data.items;
            let empty = rows.is_empty();
            rsx! {
                if !notice_text.is_empty() {
                    p { class: "mt-6 text-sm text-destructive", "{notice_text}" }
                }
                if empty {
                    div { class: "mt-6",
                        EmptyState { text: "No moderation decisions yet.".to_string() }
                    }
                } else {
                    div { class: "mt-6 overflow-x-auto",
                        table { class: "w-full text-left text-sm",
                            thead {
                                tr { class: "text-muted-foreground",
                                    th { scope: "col", class: "py-2 pr-4 font-semibold", "Submission" }
                                    th { scope: "col", class: "py-2 pr-4 font-semibold", "Kind" }
                                    th { scope: "col", class: "py-2 pr-4 font-semibold", "Status" }
                                    th { scope: "col", class: "py-2 pr-4 font-semibold", "Source" }
                                    th { scope: "col", class: "py-2 pr-4 font-semibold", "Port" }
                                    th { scope: "col", class: "py-2 pr-4 font-semibold", "Created" }
                                    th { scope: "col", class: "py-2 pr-4 font-semibold", "Reviewed" }
                                    th { scope: "col", class: "py-2 font-semibold", "Action" }
                                }
                            }
                            tbody {
                                for row in rows {
                                    LogRow {
                                        key: "{row.id}",
                                        row,
                                        on_revert: revert,
                                    }
                                }
                            }
                        }
                    }
                }
                Pager { base: "/moderation/log".to_string(), page: page_no, pages }
            }
        }
    }
}

#[component]
fn LogRow(row: LogItem, on_revert: EventHandler<i64>) -> Element {
    let revertible = row.status == "approved" || row.status == "auto_approved";
    let port = row
        .target_port_id
        .map(|id| format!("#{id}"))
        .unwrap_or_else(|| "—".to_string());
    let reviewed = row
        .reviewed_at
        .map(stamp)
        .unwrap_or_else(|| "—".to_string());
    let created = stamp(row.created_at);
    let id = row.id;
    rsx! {
        tr { class: "border-t",
            td { class: "py-2 pr-4 font-semibold", "#{row.id}" }
            td { class: "py-2 pr-4", "{row.kind}" }
            td { class: "py-2 pr-4", "{row.status}" }
            td { class: "py-2 pr-4", "{row.source}" }
            td { class: "py-2 pr-4", "{port}" }
            td { class: "whitespace-nowrap py-2 pr-4", "{created}" }
            td { class: "whitespace-nowrap py-2 pr-4", "{reviewed}" }
            td { class: "py-2",
                if revertible {
                    button {
                        r#type: "button",
                        class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM}",
                        onclick: move |_| on_revert.call(id),
                        "Revert"
                    }
                }
            }
        }
    }
}

fn stamp(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%d %H:%M UTC").to_string()
}

/// One field of a submission: the value before the change and the value it would take.
struct FieldRow {
    key: String,
    before: String,
    after: String,
    changed: bool,
}

/// Compares the submitted fields with the stored values. A missing stored value is shown as a dash.
fn field_rows(payload_json: &str, before_json: Option<&str>) -> Result<Vec<FieldRow>, String> {
    let payload: Value = serde_json::from_str(payload_json).map_err(|e| e.to_string())?;
    let before: Value = match before_json {
        Some(text) => serde_json::from_str(text).map_err(|e| e.to_string())?,
        None => Value::Null,
    };
    let Some(fields) = payload.as_object() else {
        return Ok(Vec::new());
    };
    let null = Value::Null;
    Ok(fields
        .iter()
        .map(|(key, after)| {
            let previous = before.get(key).unwrap_or(&null);
            FieldRow {
                key: key.clone(),
                before: show_value(previous),
                after: show_value(after),
                changed: previous != after,
            }
        })
        .collect())
}

fn show_value(value: &Value) -> String {
    match value {
        Value::Null => "—".to_string(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}
