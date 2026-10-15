//! The admin page: user roles and bans, and the editable site settings.
//! Admin-only, so everything loads after hydration and never enters the SSR HTML.

use dioxus::prelude::*;
use recomphub_shared::{SettingRow, UserAdminRow};

use crate::api;
use crate::components::{Breadcrumb, EmptyState, RoleGate, Seo};

const INPUT: &str = "rounded-xl border border-border bg-surface-2 px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-accent";
const BTN: &str = "rounded-xl border border-border bg-surface-2 px-3 py-2 text-sm font-semibold hover:border-accent";
const BTN_DANGER: &str = "rounded-xl border border-error bg-surface-2 px-3 py-2 text-sm font-semibold text-error hover:bg-surface";

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
        div { class: "mx-auto max-w-5xl px-4 py-10",
            Breadcrumb { items: vec![
                ("Home".to_string(), "/".to_string()),
                ("Admin".to_string(), "/admin".to_string()),
            ] }
            h1 { class: "mt-4 text-3xl font-extrabold tracking-tight", "Admin" }
            RoleGate { admin_only: true,
                div { class: "mt-8 grid gap-10",
                    UsersPanel {}
                    SettingsPanel {}
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
            div { class: "skeleton h-40 rounded-2xl", "aria-hidden": "true" }
        },
        Some(Err(message)) => rsx! {
            p { class: "text-sm text-error", "Could not load users: {message}" }
        },
        Some(Ok(rows)) if rows.is_empty() => rsx! {
            EmptyState { text: "No users match that search.".to_string() }
        },
        Some(Ok(rows)) => rsx! {
            div { class: "overflow-x-auto",
                table { class: "w-full text-left text-sm",
                    thead {
                        tr { class: "text-muted",
                            th { scope: "col", class: "py-2 pr-4 font-semibold", "User" }
                            th { scope: "col", class: "py-2 pr-4 font-semibold", "Approved" }
                            th { scope: "col", class: "py-2 pr-4 font-semibold", "Role" }
                            th { scope: "col", class: "py-2 font-semibold", "Access" }
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
            h2 { id: "admin-users", class: "text-xl font-bold", "Users" }
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
                button { r#type: "submit", class: BTN, "Search" }
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
    let display = row.name.clone().unwrap_or_else(|| row.login.clone());
    let aria_role = format!("Role for {}", row.login);
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
        tr { class: "border-t border-border",
            td { class: "py-3 pr-4",
                p { class: "font-semibold", "{display}" }
                p { class: "text-xs text-muted", "@{row.login}" }
                if banned {
                    p { class: "text-xs font-semibold text-error", "Banned" }
                }
                if !error_text.is_empty() {
                    p { class: "text-xs text-error", "{error_text}" }
                }
            }
            td { class: "py-3 pr-4", "{row.approved_count}" }
            td { class: "py-3 pr-4",
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
            td { class: "py-3",
                button {
                    r#type: "button",
                    class: if banned { BTN } else { BTN_DANGER },
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
            div { class: "skeleton h-40 rounded-2xl", "aria-hidden": "true" }
        },
        Some(Err(message)) => rsx! {
            p { class: "text-sm text-error", "Could not load settings: {message}" }
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
            h2 { id: "admin-settings", class: "text-xl font-bold", "Settings" }
            p { class: "mt-1 text-sm text-muted", "Whole numbers. Zero is allowed, except live minutes per point, which needs at least 1." }
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
        div { class: "rounded-2xl border border-border bg-surface p-4",
            label { class: "block space-y-2 text-sm font-semibold",
                "{label}"
                div { class: "flex gap-2",
                    input {
                        r#type: "number",
                        min: "{min}",
                        step: "1",
                        class: INPUT,
                        value: "{text_now}",
                        oninput: move |e| text.set(e.value()),
                    }
                    button { r#type: "button", class: BTN, onclick: move |_| save(), "Save" }
                }
            }
            match status_now {
                None => rsx! {},
                Some(Ok(message)) => rsx! {
                    p { class: "mt-2 text-xs text-success", "{message}" }
                },
                Some(Err(message)) => rsx! {
                    p { class: "mt-2 text-xs text-error", "{message}" }
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
