//! Star ratings, the rating summary, and comments at the foot of a port page.

use dioxus::prelude::*;
use recomphub_shared::TargetPlatform;
use recomphub_shared::dto::{CommentDto, CommentQuery, CommentSort, RatingSummary};

use super::icons::StarIcon;
use super::picture::Picture;
use super::styles::{
    BADGE, BADGE_DESTRUCTIVE, BADGE_OUTLINE, BUTTON, BUTTON_LINK, BUTTON_MD, BUTTON_OUTLINE,
    BUTTON_SECONDARY, BUTTON_SM, CARD_SURFACE, H2, INPUT, LABEL, LEAD, TAB, TAB_ACTIVE, TABS_LIST,
    TEXTAREA,
};
use super::time::RelativeTime;
use super::ui::{EmptyState, LoginPrompt, initials_of, target_label};
use super::viewer::{ViewerStore, login_redirect};
use crate::api;

/// Maximum comment length, matched by the server.
const MAX_BODY: usize = 2000;

/// Ratings and comments for one port. The summary arrives with the page; the comment feed loads
/// after hydration so signed-in state never reaches the server-rendered HTML.
#[component]
pub fn PortComments(port_id: i64, port_name: String, ratings: RatingSummary) -> Element {
    let viewer = use_context::<ViewerStore>();
    let signed_in = viewer.logged_in();
    let ready = *viewer.ready.read();

    let mut summary = use_signal(|| ratings.clone());
    let mut my_stars = use_signal(|| None::<i16>);
    let mut rating_error = use_signal(|| None::<String>);

    let mut sort = use_signal(CommentSort::default);
    let mut stars = use_signal(|| None::<i16>);
    let mut show_hidden = use_signal(|| false);
    let mut items = use_signal(Vec::<CommentDto>::new);
    let mut page = use_signal(|| 1i64);
    let mut pages = use_signal(|| 1i64);
    let mut total = use_signal(|| 0i64);
    let mut loaded = use_signal(|| false);
    let mut feed_error = use_signal(|| None::<String>);
    let mut generation = use_signal(|| 0u64);

    let mut draft = use_signal(String::new);
    let mut target = use_signal(String::new);
    let mut posting = use_signal(|| false);
    let mut post_error = use_signal(|| None::<String>);

    // Load the viewer's own rating once the viewer is known.
    use_effect(move || {
        if viewer.logged_in() {
            spawn(async move {
                if let Ok(value) = api::my_rating(port_id).await {
                    my_stars.set(value);
                }
            });
        } else {
            my_stars.set(None);
        }
    });

    // Fetches a page of comments for the current sort and filters. Responses from superseded
    // requests are dropped.
    let mut fetch = move |requested: i64, append: bool| {
        let query = CommentQuery {
            port_id,
            page: requested,
            sort: sort(),
            stars: stars(),
            include_hidden: show_hidden() && viewer.can_moderate(),
        };
        let token = *generation.peek() + 1;
        generation.set(token);
        spawn(async move {
            match api::list_comments(query).await {
                Ok(result) => {
                    if *generation.peek() != token {
                        return;
                    }
                    total.set(result.total);
                    pages.set(result.pages);
                    page.set(result.page);
                    if append {
                        items.write().extend(result.items);
                    } else {
                        items.set(result.items);
                    }
                    feed_error.set(None);
                    loaded.set(true);
                }
                Err(e) => {
                    if *generation.peek() == token {
                        feed_error.set(Some(e.to_string()));
                        loaded.set(true);
                    }
                }
            }
        });
    };

    // Re-runs whenever the sort or a filter changes, including the first load after hydration.
    use_effect(move || {
        let _ = (sort(), stars(), show_hidden(), viewer.me.read().is_some());
        fetch(1, false);
    });

    let mut rate = move |value: i16| {
        rating_error.set(None);
        spawn(async move {
            match api::rate_port(port_id, value).await {
                Ok(next) => {
                    summary.set(next);
                    my_stars.set(Some(value));
                }
                Err(e) => rating_error.set(Some(e.to_string())),
            }
        });
    };

    let clear_rating = move |_| {
        rating_error.set(None);
        spawn(async move {
            match api::clear_rating(port_id).await {
                Ok(next) => {
                    summary.set(next);
                    my_stars.set(None);
                }
                Err(e) => rating_error.set(Some(e.to_string())),
            }
        });
    };

    let post = move |evt: FormEvent| {
        evt.prevent_default();
        let body = draft();
        let tested = target();
        let tested = if tested.is_empty() {
            None
        } else {
            Some(tested)
        };
        posting.set(true);
        post_error.set(None);
        spawn(async move {
            match api::create_comment(port_id, body, tested).await {
                Ok(_) => {
                    draft.set(String::new());
                    target.set(String::new());
                    fetch(1, false);
                }
                Err(e) => post_error.set(Some(e.to_string())),
            }
            posting.set(false);
        });
    };

    let summary_now = summary();
    let count = summary_now.count;
    let distribution = summary_now.distribution;
    // Same rounding as the JSON-LD `ratingValue`, so the page and search results agree.
    let average_value = summary_now.average.map(|a| (a * 10.0).round() / 10.0);
    let average_text = average_value
        .map(|a| format!("{a:.1}"))
        .unwrap_or_else(|| "No ratings yet".to_string());
    let average_rounded = average_value.map(|a| a.round() as i64).unwrap_or(0);
    let mine = my_stars();
    let draft_len = draft().chars().count();
    let can_post = !posting() && draft_len > 0 && draft_len <= MAX_BODY;
    let sort_now = sort();
    let stars_now = stars();

    rsx! {
        section { class: "flex flex-col gap-6", "aria-labelledby": "ratings-heading",
            h2 { id: "ratings-heading", class: "{H2}", "Ratings and comments" }

            div { class: "{CARD_SURFACE} grid gap-6 p-6 md:grid-cols-[auto_minmax(0,1fr)] md:items-center",
                div { class: "flex flex-col gap-2",
                    if count > 0 {
                        div { class: "flex items-baseline gap-2",
                            span { class: "text-4xl font-semibold tabular-nums", "{average_text}" }
                            span { class: "text-sm text-muted-foreground", "out of 5" }
                        }
                        div { class: "flex items-center gap-0.5 text-warning",
                            role: "img",
                            "aria-label": "Average {average_text} out of 5",
                            for n in 1..=5i64 {
                                StarIcon { key: "{n}", filled: n <= average_rounded, class: "size-4" }
                            }
                        }
                        p { class: "text-sm text-muted-foreground",
                            if count == 1 { "1 rating" } else { "{count} ratings" }
                        }
                    } else {
                        p { class: "text-sm text-muted-foreground", "{average_text}" }
                    }
                }
                div { class: "flex flex-col gap-1.5",
                    for star in (1..=5i16).rev() {
                        {
                            let index = (star - 1) as usize;
                            let n = distribution[index];
                            let pct = if count > 0 { n * 100 / count } else { 0 };
                            let active = stars_now == Some(star);
                            rsx! {
                                button {
                                    key: "{star}",
                                    r#type: "button",
                                    class: "flex w-full items-center gap-3 rounded-sm text-sm outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:opacity-50",
                                    disabled: n == 0,
                                    "aria-pressed": "{active}",
                                    "aria-label": "Show {star} star comments ({n})",
                                    onclick: move |_| {
                                        stars.set(if active { None } else { Some(star) });
                                    },
                                    span { class: "w-12 shrink-0 text-left text-muted-foreground", "{star} star" }
                                    span { class: "h-2 flex-1 overflow-hidden rounded-full bg-muted",
                                        span { class: "block h-full rounded-full bg-warning", style: "width: {pct}%" }
                                    }
                                    span { class: "w-8 shrink-0 text-right tabular-nums text-muted-foreground", "{n}" }
                                }
                            }
                        }
                    }
                }
            }

            if ready && signed_in {
                div { class: "flex flex-col gap-2",
                    div { class: "flex flex-wrap items-center gap-3",
                        span { id: "rate-label", class: "{LABEL}", "Your rating" }
                        div { role: "radiogroup", "aria-labelledby": "rate-label",
                            class: "flex items-center gap-0.5",
                            for n in 1..=5i16 {
                                label {
                                    key: "{n}",
                                    class: "inline-flex cursor-pointer rounded-sm p-0.5 has-[:focus-visible]:ring-[3px] has-[:focus-visible]:ring-ring/50",
                                    input {
                                        r#type: "radio",
                                        class: "sr-only",
                                        name: "rate-{port_id}",
                                        value: "{n}",
                                        checked: mine == Some(n),
                                        "aria-label": "Rate {n} out of 5",
                                        onchange: move |_| rate(n),
                                    }
                                    StarIcon {
                                        filled: mine.is_some_and(|m| m >= n),
                                        class: if mine.is_some_and(|m| m >= n) { "size-6 text-warning" } else { "size-6 text-muted-foreground/30" },
                                    }
                                }
                            }
                        }
                        if mine.is_some() {
                            button {
                                r#type: "button",
                                class: "{BUTTON} {BUTTON_LINK} {BUTTON_SM}",
                                onclick: clear_rating,
                                "Remove rating"
                            }
                        }
                    }
                    if let Some(msg) = rating_error() {
                        p { class: "text-xs text-destructive", "{msg}" }
                    }
                }
            } else if ready {
                div { class: "flex flex-wrap items-center gap-3",
                    span { class: "text-sm text-muted-foreground", "Rate {port_name} from one to five stars." }
                    button {
                        r#type: "button",
                        class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM}",
                        onclick: move |_| login_redirect(),
                        "Sign in to rate"
                    }
                }
            }

            div { class: "flex flex-col gap-4",
                h3 { class: "text-base font-semibold", "Share your experience" }
                if ready && signed_in {
                    p { class: "{LEAD} text-sm",
                        "How does {port_name} run for you? Share your platform, hardware, settings and any issues."
                    }
                    form { class: "flex flex-col gap-3", onsubmit: post,
                        label { class: "{LABEL}", r#for: "comment-body-{port_id}", "Your comment" }
                        textarea {
                            id: "comment-body-{port_id}",
                            class: "{TEXTAREA}",
                            rows: "4",
                            placeholder: "What works, what does not, and the settings you used",
                            value: "{draft}",
                            oninput: move |e| draft.set(e.value()),
                        }
                        div { class: "flex flex-wrap items-end gap-3",
                            div { class: "flex w-56 flex-col gap-1.5",
                                label { class: "{LABEL}", r#for: "comment-target-{port_id}", "Tested on" }
                                select {
                                    id: "comment-target-{port_id}",
                                    class: "{INPUT}",
                                    value: "{target}",
                                    onchange: move |e| target.set(e.value()),
                                    option { value: "", "Not specified" }
                                    for (slug, label) in TargetPlatform::ALL.iter().map(|p| (p.slug(), p.label())) {
                                        option { key: "{slug}", value: "{slug}", "{label}" }
                                    }
                                }
                            }
                            button {
                                r#type: "submit",
                                class: "{BUTTON} {BUTTON_SECONDARY} {BUTTON_MD}",
                                disabled: !can_post,
                                if posting() { "Posting" } else { "Post comment" }
                            }
                        }
                        p { class: "text-xs text-muted-foreground", "{draft_len} / {MAX_BODY}" }
                        if let Some(msg) = post_error() {
                            p { class: "text-xs text-destructive", "{msg}" }
                        }
                    }
                } else if ready {
                    LoginPrompt { text: "Sign in to share how {port_name} runs for you." }
                }
            }

            div { class: "flex flex-col gap-4",
                div { class: "flex flex-wrap items-center justify-between gap-3",
                    div { class: "{TABS_LIST}", role: "group", "aria-label": "Sort comments",
                        for (value, label) in [
                            (CommentSort::Newest, "Newest"),
                            (CommentSort::Highest, "Highest rated"),
                            (CommentSort::Lowest, "Lowest rated"),
                        ] {
                            button {
                                key: "{label}",
                                r#type: "button",
                                class: if sort_now == value { format!("{TAB} {TAB_ACTIVE}") } else { TAB.to_string() },
                                "aria-pressed": if sort_now == value { "true" } else { "false" },
                                onclick: move |_| sort.set(value),
                                "{label}"
                            }
                        }
                    }
                    if ready && viewer.can_moderate() {
                        label { class: "{LABEL}",
                            input {
                                r#type: "checkbox",
                                checked: show_hidden(),
                                onchange: move |e| show_hidden.set(e.checked()),
                            }
                            "Show hidden"
                        }
                    }
                }

                if let Some(value) = stars_now {
                    div { class: "flex flex-wrap items-center gap-3 text-sm",
                        span { class: "text-muted-foreground", "Showing {value} star comments" }
                        button {
                            r#type: "button",
                            class: "{BUTTON} {BUTTON_LINK} {BUTTON_SM}",
                            onclick: move |_| stars.set(None),
                            "Clear filter"
                        }
                    }
                }

                if let Some(msg) = feed_error() {
                    p { class: "text-sm text-destructive", "{msg}" }
                }

                if !loaded() {
                    div { class: "skeleton h-24 rounded-xl", "aria-hidden": "true" }
                } else if items().is_empty() {
                    if stars_now.is_some() {
                        EmptyState { text: "No comments from reviewers with this rating yet." }
                    } else {
                        EmptyState { text: "No comments yet. Be the first to share how {port_name} runs." }
                    }
                } else {
                    div { class: "flex flex-col gap-4",
                        for (id, comment) in items().into_iter().map(|c| (c.id, c)) {
                            CommentCard {
                                key: "{id}",
                                comment: comment,
                                on_change: move |updated: CommentDto| {
                                    let mut list = items.write();
                                    if let Some(slot) = list.iter_mut().find(|c| c.id == updated.id) {
                                        *slot = updated;
                                    }
                                },
                                on_delete: move |id: i64| {
                                    items.write().retain(|c| c.id != id);
                                    total.with_mut(|t| *t = (*t - 1).max(0));
                                },
                            }
                        }
                    }
                    if page() < pages() {
                        div { class: "flex justify-center",
                            button {
                                r#type: "button",
                                class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD}",
                                onclick: move |_| fetch(page() + 1, true),
                                "Load more"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One comment with the viewer's available actions: edit and delete for the author, hide and
/// unhide for moderators.
#[component]
fn CommentCard(
    comment: CommentDto,
    on_change: EventHandler<CommentDto>,
    on_delete: EventHandler<i64>,
) -> Element {
    let viewer = use_context::<ViewerStore>();
    let is_author = viewer
        .me
        .read()
        .as_ref()
        .is_some_and(|me| me.id == comment.author_id);
    let can_moderate = viewer.can_moderate();

    let mut editing = use_signal(|| false);
    let mut edit_body = use_signal(String::new);
    let mut edit_target = use_signal(String::new);
    let mut confirming_delete = use_signal(|| false);
    let mut hiding = use_signal(|| false);
    let mut hide_reason = use_signal(String::new);
    let mut hide_remove_rating = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    let id = comment.id;
    let display = comment.name.clone();
    let has_rating = comment.rating.is_some();
    let tested_label = comment.tested_target.as_deref().map(target_label);
    let body = comment.body.clone();

    let start_edit = {
        let body = comment.body.clone();
        let target_slug = comment.tested_target.clone().unwrap_or_default();
        move |_| {
            edit_body.set(body.clone());
            edit_target.set(target_slug.clone());
            error.set(None);
            editing.set(true);
        }
    };

    let save_edit = move |evt: FormEvent| {
        evt.prevent_default();
        busy.set(true);
        error.set(None);
        let body = edit_body();
        let tested = edit_target();
        let tested = if tested.is_empty() {
            None
        } else {
            Some(tested)
        };
        spawn(async move {
            match api::edit_comment(id, body, tested).await {
                Ok(updated) => {
                    on_change.call(updated);
                    editing.set(false);
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            busy.set(false);
        });
    };

    let delete = move |_| {
        busy.set(true);
        error.set(None);
        spawn(async move {
            match api::delete_comment(id).await {
                Ok(()) => on_delete.call(id),
                Err(e) => error.set(Some(e.to_string())),
            }
            busy.set(false);
        });
    };

    let hide = move |hidden: bool| {
        move |_| {
            busy.set(true);
            error.set(None);
            let reason = hide_reason();
            let reason = if reason.trim().is_empty() {
                None
            } else {
                Some(reason)
            };
            let remove = hidden && hide_remove_rating();
            spawn(async move {
                match api::set_comment_hidden(id, hidden, reason, remove).await {
                    Ok(updated) => {
                        on_change.call(updated);
                        hiding.set(false);
                        hide_reason.set(String::new());
                        hide_remove_rating.set(false);
                    }
                    Err(e) => error.set(Some(e.to_string())),
                }
                busy.set(false);
            });
        }
    };

    rsx! {
        article { class: "{CARD_SURFACE} flex flex-col gap-3 p-5",
            header { class: "flex flex-wrap items-center gap-3",
                div { class: "size-8 shrink-0 overflow-hidden rounded-full bg-muted",
                    Picture {
                        media: comment.avatar.clone(),
                        alt: "",
                        initials: initials_of(&display),
                        sizes: "32px",
                        class: "size-full object-cover",
                    }
                }
                if let Some(login) = comment.github_login.clone() {
                    a {
                        class: "text-sm font-medium hover:underline",
                        href: "https://github.com/{login}",
                        target: "_blank",
                        rel: "noopener noreferrer",
                        "{display}"
                    }
                } else {
                    span { class: "text-sm font-medium", "{display}" }
                }
                RelativeTime { at: comment.created_at }
                if let Some(label) = tested_label.clone() {
                    span { class: "{BADGE} {BADGE_OUTLINE}", "Tested on {label}" }
                }
                if let Some(value) = comment.rating {
                    span {
                        class: "inline-flex items-center gap-0.5 text-warning",
                        role: "img",
                        "aria-label": "Rated {value} out of 5",
                        for n in 1..=5i16 {
                            StarIcon { key: "{n}", filled: n <= value, class: "size-3.5" }
                        }
                    }
                }
                if comment.hidden {
                    span { class: "{BADGE} {BADGE_DESTRUCTIVE}", "Hidden" }
                }
            }

            if editing() {
                form { class: "flex flex-col gap-3", onsubmit: save_edit,
                    label { class: "{LABEL}", "Edit comment" }
                    textarea {
                        class: "{TEXTAREA}",
                        rows: "4",
                        value: "{edit_body}",
                        oninput: move |e| edit_body.set(e.value()),
                    }
                    div { class: "flex flex-wrap items-end gap-3",
                        div { class: "flex w-56 flex-col gap-1.5",
                            label { class: "{LABEL}", "Tested on" }
                            select {
                                class: "{INPUT}",
                                value: "{edit_target}",
                                onchange: move |e| edit_target.set(e.value()),
                                option { value: "", "Not specified" }
                                for (slug, label) in TargetPlatform::ALL.iter().map(|p| (p.slug(), p.label())) {
                                    option { key: "{slug}", value: "{slug}", "{label}" }
                                }
                            }
                        }
                        button {
                            r#type: "submit",
                            class: "{BUTTON} {BUTTON_SECONDARY} {BUTTON_SM}",
                            disabled: busy() || edit_body().trim().is_empty(),
                            "Save"
                        }
                        button {
                            r#type: "button",
                            class: "{BUTTON} {BUTTON_LINK} {BUTTON_SM}",
                            onclick: move |_| editing.set(false),
                            "Cancel"
                        }
                    }
                }
            } else {
                p { class: "whitespace-pre-line text-sm", "{body}" }
                if comment.edited_at.is_some() {
                    p { class: "text-xs text-muted-foreground", "Edited" }
                }
            }

            if comment.hidden {
                if let Some(reason) = comment.hidden_reason.clone() {
                    p { class: "text-xs text-muted-foreground", "Hidden by a moderator: {reason}" }
                } else {
                    p { class: "text-xs text-muted-foreground", "Hidden by a moderator." }
                }
            }

            if let Some(msg) = error() {
                p { class: "text-xs text-destructive", "{msg}" }
            }

            if !editing() {
                div { class: "flex flex-wrap items-center gap-3 text-xs",
                    if is_author && !comment.hidden {
                        button {
                            r#type: "button",
                            class: "{BUTTON} {BUTTON_LINK} {BUTTON_SM}",
                            onclick: start_edit,
                            "Edit"
                        }
                        if confirming_delete() {
                            span { class: "text-muted-foreground", "Delete this comment?" }
                            button {
                                r#type: "button",
                                class: "{BUTTON} {BUTTON_LINK} {BUTTON_SM} text-destructive",
                                disabled: busy(),
                                onclick: delete,
                                "Delete"
                            }
                            button {
                                r#type: "button",
                                class: "{BUTTON} {BUTTON_LINK} {BUTTON_SM}",
                                onclick: move |_| confirming_delete.set(false),
                                "Keep"
                            }
                        } else {
                            button {
                                r#type: "button",
                                class: "{BUTTON} {BUTTON_LINK} {BUTTON_SM}",
                                onclick: move |_| confirming_delete.set(true),
                                "Delete"
                            }
                        }
                    }
                    if can_moderate {
                        if comment.hidden {
                            button {
                                r#type: "button",
                                class: "{BUTTON} {BUTTON_LINK} {BUTTON_SM}",
                                disabled: busy(),
                                onclick: hide(false),
                                "Unhide"
                            }
                        } else if hiding() {
                            div { class: "flex w-full flex-wrap items-end gap-3",
                                input {
                                    class: "{INPUT} max-w-xs",
                                    placeholder: "Reason (optional)",
                                    value: "{hide_reason}",
                                    oninput: move |e| hide_reason.set(e.value()),
                                }
                                if has_rating {
                                    label { class: "{LABEL}",
                                        input {
                                            r#type: "checkbox",
                                            checked: hide_remove_rating(),
                                            onchange: move |e| hide_remove_rating.set(e.checked()),
                                        }
                                        "Also remove the rating"
                                    }
                                }
                                button {
                                    r#type: "button",
                                    class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM}",
                                    disabled: busy(),
                                    onclick: hide(true),
                                    "Hide comment"
                                }
                                button {
                                    r#type: "button",
                                    class: "{BUTTON} {BUTTON_LINK} {BUTTON_SM}",
                                    onclick: move |_| hiding.set(false),
                                    "Cancel"
                                }
                            }
                        } else {
                            button {
                                r#type: "button",
                                class: "{BUTTON} {BUTTON_LINK} {BUTTON_SM}",
                                onclick: move |_| hiding.set(true),
                                "Hide"
                            }
                        }
                    }
                }
            }
        }
    }
}
