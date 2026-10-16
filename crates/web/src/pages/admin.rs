//! The admin page: user roles and bans, and the editable site settings.
//! Admin-only, so everything loads after hydration and never enters the SSR HTML.

use dioxus::prelude::*;
use recomphub_shared::dto::provider_label;
use recomphub_shared::{SettingRow, UserAdminRow};

use super::admin_ai_providers::AiProvidersSection;
use crate::api;
use crate::components::styles::*;
use crate::components::{Breadcrumb, EmptyState, RoleGate, Seo};

/// Smallest value the database accepts for a setting (mirrors `recomphub_db::settings::validate`).
fn min_for(key: &str) -> i64 {
    if key == "live_minutes_per_point" {
        1
    } else {
        0
    }
}

const ROLES: [&str; 3] = ["user", "moderator", "admin"];

#[component]
pub fn AdminPage() -> Element {
    rsx! {
        Seo {
            title: "Admin | RecompHub".to_string(),
            description: "Site administration: users, roles and settings.".to_string(),
            path: "/admin".to_string(),
            noindex: true,
        }
        div { class: "mx-auto max-w-5xl",
            header {
                Breadcrumb { items: vec![
                    ("Home".to_string(), "/".to_string()),
                    ("Admin".to_string(), "/admin".to_string()),
                ] }
                h1 { class: "{H1} mt-4", "Admin" }
            }
            RoleGate { admin_only: true, next: "/admin".to_string(),
                div { class: "mt-8 space-y-12",
                    UsersPanel {}
                    SettingsPanel {}
                    AiProvidersSection {}
                }
            }
        }
    }
}

#[component]
fn UsersPanel() -> Element {
    let mut query = use_signal(String::new);
    let mut submitted = use_signal(String::new);
    let mut users = use_signal(|| None::<Result<Vec<UserAdminRow>, String>>);
    let mut reload = use_signal(|| 0u32);
    use_effect(move || {
        let term = submitted();
        let _ = reload();
        spawn(async move {
            let result = api::admin_users(term).await.map_err(|e| e.to_string());
            users.set(Some(result));
        });
    });

    let query_text = query();
    let state = users.read().clone();
    let body = match state {
        None => rsx! {
            div { class: "skeleton h-40 rounded-xl", "aria-hidden": "true" }
        },
        Some(Err(message)) => rsx! {
            p { class: "text-sm text-destructive", "Could not load users: {message}" }
        },
        Some(Ok(rows)) if rows.is_empty() => rsx! {
            EmptyState { text: "No users match that search.".to_string() }
        },
        Some(Ok(rows)) => rsx! {
            div { class: TABLE_WRAP,
                table { class: TABLE,
                    thead {
                        tr { class: TABLE_HEAD_ROW,
                            th { scope: "col", class: TABLE_TH, "User" }
                            th { scope: "col", class: TABLE_TH, "Approved" }
                            th { scope: "col", class: TABLE_TH, "Role" }
                            th { scope: "col", class: TABLE_TH, "Access" }
                        }
                    }
                    tbody {
                        for row in rows {
                            UserRow {
                                key: "{row.id}",
                                row,
                                on_changed: move |_| reload.set(reload() + 1),
                            }
                        }
                    }
                }
            }
        },
    };

    rsx! {
        section { aria_labelledby: "admin-users",
            h2 { id: "admin-users", class: H2, "Users" }
            form {
                class: "mt-4 flex flex-wrap gap-2",
                onsubmit: move |e| {
                    e.prevent_default();
                    submitted.set(query());
                },
                input {
                    r#type: "search",
                    class: INPUT,
                    placeholder: "Login or name",
                    "aria-label": "Search users",
                    value: "{query_text}",
                    oninput: move |e| query.set(e.value()),
                }
                button { r#type: "submit", class: "{BUTTON} {BUTTON_PRIMARY} {BUTTON_MD}", "Search" }
            }
            div { class: "mt-4", {body} }
        }
    }
}

#[component]
fn UserRow(row: UserAdminRow, on_changed: EventHandler<()>) -> Element {
    let mut error = use_signal(|| None::<String>);
    let user_id = row.id;
    let banned = row.banned;
    let role = row.role.clone();
    let display = row.display_name.clone();
    let github = row.github_login.clone();
    let providers: Vec<(String, &'static str)> = row
        .providers
        .iter()
        .map(|p| (p.clone(), provider_label(p)))
        .collect();
    let aria_role = format!("Role for {display}");
    let ban_label = if banned { "Unban" } else { "Ban" };

    let set_role = move |value: String| {
        spawn(async move {
            match api::set_role(user_id, value).await {
                Ok(()) => on_changed.call(()),
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };

    let toggle_ban = move || {
        let next = !banned;
        spawn(async move {
            match api::set_banned(user_id, next).await {
                Ok(()) => on_changed.call(()),
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };

    let error_text = error().unwrap_or_default();

    rsx! {
        tr { class: TABLE_ROW,
            td { class: TABLE_TD,
                p { class: "font-medium", "{display}" }
                if let Some(login) = github {
                    p { class: "text-xs text-muted-foreground", "@{login}" }
                }
                div { class: "mt-1 flex flex-wrap gap-1",
                    for (key, label) in providers {
                        span { key: "{key}", class: "{BADGE} {BADGE_OUTLINE}", "{label}" }
                    }
                }
                if banned {
                    p { class: "text-xs font-medium text-destructive", "Banned" }
                }
                if !error_text.is_empty() {
                    p { class: "text-xs text-destructive", "{error_text}" }
                }
            }
            td { class: "{TABLE_TD} tabular", "{row.approved_count}" }
            td { class: TABLE_TD,
                select {
                    class: INPUT,
                    "aria-label": "{aria_role}",
                    onchange: move |e| set_role(e.value()),
                    for choice in ROLES {
                        option {
                            key: "{choice}",
                            value: "{choice}",
                            selected: role == choice,
                            "{choice}"
                        }
                    }
                }
            }
            td { class: TABLE_TD,
                button {
                    r#type: "button",
                    class: if banned {
                        "{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM}"
                    } else {
                        "{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM} text-destructive"
                    },
                    onclick: move |_| toggle_ban(),
                    "{ban_label}"
                }
            }
        }
    }
}

#[component]
fn SettingsPanel() -> Element {
    let mut settings = use_signal(|| None::<Result<Vec<SettingRow>, String>>);
    use_effect(move || {
        spawn(async move {
            let result = api::admin_settings().await.map_err(|e| e.to_string());
            settings.set(Some(result));
        });
    });

    let state = settings.read().clone();
    let body = match state {
        None => rsx! {
            div { class: "skeleton h-40 rounded-xl", "aria-hidden": "true" }
        },
        Some(Err(message)) => rsx! {
            p { class: "text-sm text-destructive", "Could not load settings: {message}" }
        },
        Some(Ok(rows)) => rsx! {
            ul { class: "grid gap-4 sm:grid-cols-2",
                for row in rows {
                    li { key: "{row.key}",
                        SettingField { row }
                    }
                }
            }
        },
    };

    rsx! {
        section { aria_labelledby: "admin-settings",
            h2 { id: "admin-settings", class: H2, "Settings" }
            p { class: "mt-2 text-sm text-muted-foreground", "Whole numbers. Zero is allowed, except live minutes per point, which needs at least 1." }
            div { class: "mt-4", {body} }
        }
    }
}

#[component]
fn SettingField(row: SettingRow) -> Element {
    let label = setting_label(&row.key);
    let key = row.key.clone();
    let min = min_for(&row.key);
    let mut text = use_signal(|| row.value.to_string());
    let mut status = use_signal(|| None::<Result<String, String>>);

    let mut save = move || {
        let raw = text();
        let key = key.clone();
        match raw.trim().parse::<i64>() {
            Ok(value) if value >= min => {
                spawn(async move {
                    let outcome = api::update_setting(key, value).await;
                    status.set(Some(match outcome {
                        Ok(()) => Ok("Saved.".to_string()),
                        Err(e) => Err(e.to_string()),
                    }));
                });
            }
            _ => status.set(Some(Err(format!(
                "Enter a whole number of at least {min}."
            )))),
        }
    };

    let text_now = text();
    let status_now = status();

    rsx! {
        div { class: "{CARD_SURFACE} grid gap-4 p-4",
            label { class: "grid gap-2",
                span { class: LABEL, "{label}" }
                div { class: "flex gap-2",
                    input {
                        r#type: "number",
                        min: "{min}",
                        step: "1",
                        class: INPUT,
                        value: "{text_now}",
                        oninput: move |e| text.set(e.value()),
                    }
                    button {
                        r#type: "button",
                        class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD}",
                        onclick: move |_| save(),
                        "Save"
                    }
                }
            }
            match status_now {
                None => rsx! {},
                Some(Ok(message)) => rsx! {
                    p { class: "text-sm text-success", "{message}" }
                },
                Some(Err(message)) => rsx! {
                    p { class: "text-sm text-destructive", "{message}" }
                },
            }
        }
    }
}

fn setting_label(key: &str) -> &str {
    match key {
        "catalog_target_size" => "Catalog target size (games)",
        "trust_threshold" => "Approvals before an account is trusted",
        "youtube_daily_searches" => "YouTube searches per day",
        "submission_daily_limit" => "Submissions per account per day",
        "live_minutes_per_point" => "Live minutes per score point",
        other => other,
    }
}
