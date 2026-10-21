//! Moderation pages: the review queue and the audit log. Both are moderator-only, so
//! their data loads after hydration and never enters the SSR HTML.

use chrono::{DateTime, Utc};
use dioxus::prelude::*;
use recomphub_shared::{
    BatchReviewAction, BatchReviewItem, BatchReviewOutcome, BatchReviewRequest, BatchReviewStatus,
    GameChoice, LogItem, ModerationQueuePage, ModerationQueueQuery, Page, PendingPortImageDto,
    PendingVideoDto, Playability, PortKind, QueueItem, QueueLane, ReviewOverrides, TargetPlatform,
    TriageCategory, TriageStateFilter,
};
use serde_json::Value;
use std::collections::BTreeSet;

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

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Member,
    Found,
    Screenshots,
    Videos,
}

#[component]
fn QueueList() -> Element {
    let mut submissions = use_signal(|| None::<Result<ModerationQueuePage, String>>);
    let mut images = use_signal(|| None::<Result<Vec<PendingPortImageDto>, String>>);
    let mut videos = use_signal(|| None::<Result<Vec<PendingVideoDto>, String>>);
    let mut reload = use_signal(|| 0u32);
    let mut tab = use_signal(|| Tab::Member);
    let mut page = use_signal(|| 1i64);
    let mut selected = use_signal(BTreeSet::<i64>::new);
    let mut request_generation = use_signal(|| 0u64);
    let mut batch_note = use_signal(String::new);
    let mut confirm_rejection = use_signal(|| false);
    let mut batch_busy = use_signal(|| false);
    let mut batch_outcomes = use_signal(Vec::<BatchReviewOutcome>::new);
    use_effect(move || {
        let _ = reload();
        let active_tab = tab();
        let query = ModerationQueueQuery {
            lane: if active_tab == Tab::Found {
                QueueLane::Found
            } else {
                QueueLane::Members
            },
            page: page(),
            category: (active_tab == Tab::Found).then_some(TriageCategory::GameProject),
            confidence: None,
            target: None,
            playability: None,
            flagged_only: false,
            triage_state: TriageStateFilter::All,
        };
        let generation = *request_generation.peek() + 1;
        request_generation.set(generation);
        spawn(async move {
            let result = api::moderation_queue(query)
                .await
                .map_err(|e| e.to_string());
            if request_generation() == generation {
                submissions.set(Some(result));
            }
        });
        spawn(async move {
            images.set(Some(
                api::pending_port_images().await.map_err(|e| e.to_string()),
            ));
        });
        spawn(async move {
            videos.set(Some(
                api::pending_submitted_videos()
                    .await
                    .map_err(|e| e.to_string()),
            ));
        });
    });

    let subs = submissions.read().clone();
    let imgs = images.read().clone();
    let vids = videos.read().clone();
    let Some(Ok(subs)) = subs else {
        return match subs {
            Some(Err(message)) => rsx! {
                p { class: "mt-8 text-sm text-destructive", "Could not load the queue: {message}" }
            },
            _ => rsx! {
                div { class: "mt-8 space-y-4",
                    for i in 0..3 {
                        div { key: "{i}", class: "skeleton h-40 rounded-xl", "aria-hidden": "true" }
                    }
                }
            },
        };
    };
    let counts = subs.counts.clone();
    let queue_items = subs.rows.items;
    let queue_page = subs.rows.page;
    let page_ids: BTreeSet<i64> = queue_items.iter().map(|item| item.id).collect();
    let queue_pages = subs.rows.pages;
    let page_items = queue_items.clone();
    let image_count = imgs
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .map_or(0, Vec::len);
    let video_count = vids
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .map_or(0, Vec::len);
    let active = tab();
    let on_done = move |_| reload.set(reload() + 1);

    let tab_button = |label: String, value: Tab| {
        rsx! {
            button { r#type: "button", role: "tab", "aria-selected": if active == value { "true" } else { "false" },
                class: if active == value { "{TAB} {TAB_ACTIVE}" } else { "{TAB}" },
                onclick: move |_| {
                    selected.set(BTreeSet::new());
                    page.set(1);
                    tab.set(value);
                },
                "{label}"
            }
        }
    };

    rsx! {
        div { role: "tablist", class: "{TABS_LIST} mt-8 flex-wrap",
            {tab_button(format!("Member submissions ({})", counts.members), Tab::Member)}
            {tab_button(format!("Found automatically ({})", counts.found), Tab::Found)}
            {tab_button(format!("Screenshots ({image_count})"), Tab::Screenshots)}
            {tab_button(format!("Videos ({video_count})"), Tab::Videos)}
        }
        div { class: "mt-6",
            match active {
                Tab::Member | Tab::Found => rsx! {
                    SubmissionList { items: queue_items, on_done, selected }
                    p { class: "mb-3 text-sm text-muted-foreground", "{selected.read().len()} selected on this page." }
                    button {
                        r#type: "button",
                        class: "btn btn-destructive mb-4 ml-3",
                        disabled: selected.read().is_empty(),
                        onclick: move |_| confirm_rejection.set(true),
                        "Reject selected"
                    }
                    if confirm_rejection() {
                        div { class: "mb-4 rounded-lg border border-destructive p-4 space-y-3",
                            p { class: "font-semibold", "Confirm rejection for {selected.read().len()} selected submissions" }
                            p { class: "text-sm text-muted-foreground", "This is a human moderation decision. It does not attest to playtesting." }
                            textarea {
                                class: "input w-full",
                                placeholder: "Required rejection reason",
                                value: "{batch_note}",
                                oninput: move |event| batch_note.set(event.value()),
                            }
                            div { class: "flex gap-3",
                                button {
                                    r#type: "button",
                                    class: "btn btn-secondary",
                                    disabled: batch_busy(),
                                    onclick: move |_| confirm_rejection.set(false),
                                    "Cancel"
                                }
                                button {
                                    r#type: "button",
                                    class: "btn btn-destructive",
                                    disabled: batch_busy() || batch_note().trim().is_empty(),
                                    onclick: move |_| {
                                        let note = batch_note().trim().to_string();
                                        let items = page_items
                                            .iter()
                                            .filter(|item| selected.read().contains(&item.id))
                                            .map(|item| BatchReviewItem {
                                                item_id: item.id,
                                                expected_triage_revision: item.triage_revision,
                                                overrides: ReviewOverrides::default(),
                                            })
                                            .collect::<Vec<_>>();
                                        batch_busy.set(true);
                                        spawn(async move {
                                            match api::review_batch(BatchReviewRequest {
                                                action: BatchReviewAction::Reject,
                                                items,
                                                note,
                                            }).await {
                                                Ok(outcomes) => {
                                                    selected.with_mut(|ids| {
                                                        for outcome in &outcomes {
                                                            if matches!(outcome.status, BatchReviewStatus::Rejected) {
                                                                ids.remove(&outcome.item_id);
                                                            }
                                                        }
                                                    });
                                                    batch_outcomes.set(outcomes);
                                                    reload.set(reload() + 1);
                                                }
                                                Err(error) => batch_outcomes.set(vec![BatchReviewOutcome {
                                                    item_id: 0,
                                                    status: BatchReviewStatus::Error,
                                                    error: Some(error.to_string()),
                                                }]),
                                            }
                                            batch_busy.set(false);
                                            confirm_rejection.set(false);
                                        });
                                    },
                                    "Confirm rejection"
                                }
                            }
                        }
                    }
                    if !batch_outcomes().is_empty() {
                        div { class: "mb-4 rounded-lg border p-4",
                            p { class: "font-semibold", "Batch results" }
                            ul { class: "mt-2 space-y-1 text-sm",
                                for outcome in batch_outcomes() {
                                    li { key: "{outcome.item_id}", "#{outcome.item_id}: {outcome.status:?} {outcome.error.as_deref().unwrap_or_default()}" }
                                }
                            }
                        }
                    }
                    button {
                        r#type: "button",
                        class: "btn btn-secondary mb-4",
                        onclick: move |_| selected.set(page_ids.clone()),
                        "Select this page"
                    }
                    div { class: "mt-6 flex items-center justify-between gap-3",
                        button {
                            r#type: "button",
                            class: "btn btn-secondary",
                            disabled: queue_page <= 1,
                            onclick: move |_| {
                                selected.set(BTreeSet::new());
                                page.set((queue_page - 1).max(1));
                            },
                            "Previous"
                        }
                        span { class: "text-sm text-muted-foreground", "Page {queue_page} of {queue_pages}" }
                        button {
                            r#type: "button",
                            class: "btn btn-secondary",
                            disabled: queue_page >= queue_pages,
                            onclick: move |_| {
                                selected.set(BTreeSet::new());
                                page.set((queue_page + 1).min(queue_pages));
                            },
                            "Next"
                        }
                    }
                },
                Tab::Screenshots => rsx! { ImageQueuePanel { state: imgs, on_done } },
                Tab::Videos => rsx! { VideoQueuePanel { state: vids, on_done } },
            }
        }
    }
}

#[component]
fn SubmissionList(
    items: Vec<QueueItem>,
    on_done: EventHandler<()>,
    selected: Signal<BTreeSet<i64>>,
) -> Element {
    if items.is_empty() {
        return rsx! { EmptyState { text: "Nothing here right now.".to_string() } };
    }
    rsx! {
        ul { class: "space-y-4",
            for item in items {
                li { key: "{item.id}", QueueCard { item, on_done, selected } }
            }
        }
    }
}

#[component]
fn ImageQueuePanel(
    state: Option<Result<Vec<PendingPortImageDto>, String>>,
    on_done: EventHandler<()>,
) -> Element {
    match state {
        None => rsx! {
            div { class: "skeleton h-40 rounded-xl", "aria-hidden": "true" }
        },
        Some(Err(message)) => rsx! {
            p { class: "text-sm text-destructive", "Could not load screenshots: {message}" }
        },
        Some(Ok(items)) if items.is_empty() => rsx! {
            EmptyState { text: "No screenshots waiting for review.".to_string() }
        },
        Some(Ok(items)) => rsx! {
            ul { class: "grid gap-4 sm:grid-cols-2 lg:grid-cols-3",
                for item in items {
                    li { key: "{item.id}", ImageQueueCard { item, on_done } }
                }
            }
        },
    }
}

#[component]
fn ImageQueueCard(item: PendingPortImageDto, on_done: EventHandler<()>) -> Element {
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let id = item.id;
    let approve_fn = move || {
        spawn(async move {
            busy.set(true);
            match api::review_port_image(id, true).await {
                Ok(()) => on_done.call(()),
                Err(e) => error.set(Some(e.to_string())),
            }
            busy.set(false);
        });
    };
    let reject_fn = move || {
        spawn(async move {
            busy.set(true);
            match api::review_port_image(id, false).await {
                Ok(()) => on_done.call(()),
                Err(e) => error.set(Some(e.to_string())),
            }
            busy.set(false);
        });
    };
    let error_text = error().unwrap_or_default();
    let port_href = format!("/games/{}/{}", item.game_slug, item.port_slug);
    rsx! {
            p { class: "mt-1 text-xs text-muted-foreground",
                "Source: {item.source}"
                if let Some(alt) = item.source_alt.clone().filter(|alt| !alt.is_empty()) {
                    " · alt: {alt}"
                }
            }
            p { class: "text-xs text-muted-foreground",
                if let (Some(width), Some(height)) = (item.width, item.height) {
                    "{width} × {height}px"
                } else {
                    "Dimensions unavailable while rendering"
                }
            }
            if let Some(reason) = item.decision_reason.clone() {
                p { class: "mt-1 text-xs text-muted-foreground", "{reason}" }
            }
            if let Some(media_error) = item.media_error.clone() {
                p { class: "mt-1 text-xs text-destructive", "{media_error}" }
            }
        article { class: "{CARD_SURFACE} p-3",
            crate::components::Picture {
                media: Some(item.image.clone()),
                alt: format!("Screenshot submitted for {}", item.port_name),
                initials: "".to_string(),
                sizes: "384px".to_string(),
                class: "aspect-video w-full rounded-md object-cover".to_string(),
            }
            p { class: "mt-2 text-sm font-medium",
                a { href: "{port_href}", class: "hover:underline", "{item.port_name}" }
            }
            p { class: "text-xs text-muted-foreground",
                "{item.uploader_name.clone().unwrap_or_else(|| \"Member\".to_string())}"
            }
            div { class: "mt-3 flex gap-2",
                button {
                    r#type: "button",
                    disabled: busy(),
                    class: "{BUTTON} {BUTTON_PRIMARY} {BUTTON_XS}",
                    onclick: move |_| approve_fn(),
                    "Approve"
                }
                button {
                    r#type: "button",
                    disabled: busy(),
                    class: "{BUTTON} {BUTTON_DESTRUCTIVE} {BUTTON_XS}",
                    onclick: move |_| reject_fn(),
                    "Reject"
                }
            }
            if !error_text.is_empty() {
                p { class: "mt-2 text-xs text-destructive", "{error_text}" }
            }
        }
    }
}

#[component]
fn VideoQueuePanel(
    state: Option<Result<Vec<PendingVideoDto>, String>>,
    on_done: EventHandler<()>,
) -> Element {
    match state {
        None => rsx! {
            div { class: "skeleton h-40 rounded-xl", "aria-hidden": "true" }
        },
        Some(Err(message)) => rsx! {
            p { class: "text-sm text-destructive", "Could not load videos: {message}" }
        },
        Some(Ok(items)) if items.is_empty() => rsx! {
            EmptyState { text: "No videos waiting for review.".to_string() }
        },
        Some(Ok(items)) => rsx! {
            ul { class: "grid gap-4 sm:grid-cols-2 lg:grid-cols-3",
                for item in items {
                    li { key: "{item.id}", VideoQueueCard { item, on_done } }
                }
            }
        },
    }
}

#[component]
fn VideoQueueCard(item: PendingVideoDto, on_done: EventHandler<()>) -> Element {
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let id = item.id;
    let approve_source = item.source.clone();
    let reject_source = item.source.clone();
    let approve_done = on_done;
    let reject_done = on_done;
    let error_text = error().unwrap_or_default();
    let watch = format!("https://www.youtube.com/watch?v={}", item.video.youtube_id);
    let subject = item
        .port_name
        .clone()
        .unwrap_or_else(|| item.game_name.clone());
    rsx! {
        article { class: "{CARD_SURFACE} p-3",
            a { href: "{watch}", target: "_blank", rel: "noopener",
                crate::components::Picture {
                    media: item.video.thumb.clone(),
                    alt: item.video.title.clone(),
                    initials: "".to_string(),
                    sizes: "384px".to_string(),
                    class: "aspect-video w-full rounded-md object-cover".to_string(),
                }
            }
            p { class: "mt-2 line-clamp-2 text-sm font-medium", "{item.video.title}" }
            p { class: "text-xs text-muted-foreground", "{subject}" }
            p { class: "text-xs text-muted-foreground", "Source: {item.source}" }
            if let Some(reason) = item.decision_reason.clone() {
                p { class: "text-xs text-muted-foreground", "{reason}" }
            }
            div { class: "mt-3 flex gap-2",
                button {
                    r#type: "button",
                    disabled: busy(),
                    class: "{BUTTON} {BUTTON_PRIMARY} {BUTTON_XS}",
                    onclick: move |_| {
                        let source = approve_source.clone();
                        let on_done = approve_done;
                        spawn(async move {
                            busy.set(true);
                            match api::review_submitted_video(id, source, true).await {
                                Ok(()) => on_done.call(()),
                                Err(e) => error.set(Some(e.to_string())),
                            }
                            busy.set(false);
                        });
                    },
                    "Approve"
                }
                button {
                    r#type: "button",
                    disabled: busy(),
                    class: "{BUTTON} {BUTTON_DESTRUCTIVE} {BUTTON_XS}",
                    onclick: move |_| {
                        let source = reject_source.clone();
                        let on_done = reject_done;
                        spawn(async move {
                            busy.set(true);
                            match api::review_submitted_video(id, source, false).await {
                                Ok(()) => on_done.call(()),
                                Err(e) => error.set(Some(e.to_string())),
                            }
                            busy.set(false);
                        });
                    },
                    "Reject"
                }
            }
            if !error_text.is_empty() {
                p { class: "mt-2 text-xs text-destructive", "{error_text}" }
            }
        }
    }
}

#[component]
fn QueueCard(
    item: QueueItem,
    on_done: EventHandler<()>,
    selected: Signal<BTreeSet<i64>>,
) -> Element {
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
    let triage_view = match (&item.triage, &item.triage_error) {
        (_, Some(error)) => {
            rsx! { p { class: "mt-3 text-sm text-destructive", "Evidence preparation failed: {error}" } }
        }
        (Some(triage), _) => rsx! {
            div { class: "mt-3 text-sm",
                p { class: "font-medium", "{triage.assessment.category:?} · {triage.assessment.confidence:?}" }
                p { class: "text-muted-foreground", "{triage.assessment.summary}" }
                p { class: "mt-1 text-xs text-muted-foreground", "Unfinished and archived projects can qualify. AI triage is not playtesting." }
                if !triage.assessment.citations.is_empty() {
                    ul { class: "mt-2 space-y-1",
                        for citation in &triage.assessment.citations {
                            li { key: "{citation.claim}-{citation.quote}",
                                a { class: "underline", href: "{citation.source_url}", target: "_blank", "\"{citation.quote}\"" }
                            }
                        }
                    }
                }
            }
        },
        (None, None) => {
            rsx! { p { class: "mt-3 text-sm text-muted-foreground", "Untriaged — no evidence-backed recommendation." } }
        }
    };

    let item_id = item.id;
    let is_selected = selected.read().contains(&item_id);
    rsx! {
        article { class: "{CARD_SURFACE} p-5",
            div { class: "flex flex-wrap items-center gap-2",
                label { class: "flex items-center gap-2 text-sm",
                    input {
                        r#type: "checkbox",
                        checked: is_selected,
                        onchange: move |event| {
                            selected.with_mut(|ids| {
                                if event.checked() {
                                    ids.insert(item_id);
                                } else {
                                    ids.remove(&item_id);
                                }
                            });
                        }
                    }
                    "Select"
                }
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
            {triage_view}
            {diff_view}
            Decision {
                item_id: item.id,
                kind: item.kind.clone(),
                revision: item.triage_revision,
                needs_match,
                on_done,
            }
        }
    }
}

/// Approve and reject controls. Candidates and unmatched imports need a game first.
#[component]
fn Decision(
    item_id: i64,
    kind: String,
    revision: i64,
    needs_match: bool,
    on_done: EventHandler<()>,
) -> Element {
    let is_candidate = kind == "candidate";
    let mut query = use_signal(String::new);
    let mut choices = use_signal(Vec::<GameChoice>::new);
    let mut picked = use_signal(|| None::<Pick>);
    let mut kind_choice = use_signal(String::new);
    let mut playability_choice = use_signal(String::new);
    let mut declared_targets = use_signal(Vec::<String>::new);
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
        if is_candidate && kind_choice().is_empty() {
            error.set(Some("Choose a port kind before approving.".to_string()));
            return;
        }
        let overrides = ReviewOverrides {
            game_id: chosen.as_ref().and_then(|p| p.game_id),
            igdb_id: chosen.as_ref().and_then(|p| p.igdb_id),
            kind: is_candidate.then(&*kind_choice),
            playability: (!playability_choice().is_empty()).then(&*playability_choice),
            declared_platforms: Some(declared_targets()),
        };
        busy.set(true);
        error.set(None);
        spawn(async move {
            match api::approve(item_id, overrides, Some(revision)).await {
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
            match api::reject(item_id, text, Some(revision)).await {
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
    let current_playability = playability_choice();
    let current_targets = declared_targets();

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
                        option { value: "", selected: current_kind.is_empty(), disabled: true, "Choose a kind" }
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
                "Playability"
                select { class: INPUT, onchange: move |e| playability_choice.set(e.value()),
                    option { value: "", selected: current_playability.is_empty(), "Keep submitted value" }
                    for value in Playability::ALL {
                        option { value: "{value.as_str()}", selected: current_playability == value.as_str(), "{value.label()}" }
                    }
                }
            }
            div { class: "space-y-2",
                p { class: "text-sm font-semibold", "Declared target platforms" }
                for target in TargetPlatform::ALL {
                    label { class: "mr-4 inline-flex items-center gap-2 text-sm",
                        input {
                            r#type: "checkbox",
                            checked: current_targets.iter().any(|value| value == target.as_str()),
                            onchange: move |event| declared_targets.with_mut(|targets| {
                                if event.checked() {
                                    if !targets.iter().any(|value| value == target.as_str()) {
                                        targets.push(target.as_str().to_string());
                                    }
                                } else {
                                    targets.retain(|value| value != target.as_str());
                                }
                            }),
                        }
                        "{target.label()}"
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
                                    th { scope: "col", class: "py-2 pr-4 font-semibold", "Reviewer" }
                                    th { scope: "col", class: "py-2 pr-4 font-semibold", "Note" }
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
    let reviewer = row
        .reviewer_id
        .map(|id| format!("#{id}"))
        .unwrap_or_else(|| "—".to_string());
    let note = row.review_note.clone().unwrap_or_else(|| "—".to_string());
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
            td { class: "py-2 pr-4", "{reviewer}" }
            td { class: "py-2 pr-4", "{note}" }
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
