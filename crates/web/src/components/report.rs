use dioxus::prelude::*;

use crate::api;
use crate::components::styles::*;

/// "Report a problem" link and dialog. Creates a `report` submission for a game, or for one of its ports.
#[component]
pub fn ReportDialog(game_id: i64, #[props(default)] port_id: Option<i64>) -> Element {
    let mut open = use_signal(|| false);
    let mut note = use_signal(String::new);
    let mut status = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    rsx! {
        button {
            r#type: "button",
            class: "text-sm text-muted-foreground underline underline-offset-4 transition-colors hover:text-foreground",
            onclick: move |_| open.set(true),
            "Report a problem"
        }
        if open() {
            div { class: "fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4",
                div {
                    "role": "dialog",
                    "aria-modal": "true",
                    "aria-labelledby": "report-title",
                    class: "{POPOVER} w-full max-w-lg",
                    div { class: "grid gap-6 p-5",
                        div { class: "grid gap-2",
                            h2 { id: "report-title", class: "text-lg font-semibold tracking-tight", "Report a problem" }
                            textarea {
                                class: TEXTAREA,
                                rows: "5",
                                placeholder: "What is wrong? (1000 characters at most)",
                                value: "{note}",
                                oninput: move |event| note.set(event.value()),
                            }
                            p { class: "text-sm text-muted-foreground tabular", "{note().chars().count()} / 1000" }
                        }
                        if let Some(message) = status() {
                            p { class: "text-sm", "{message}" }
                        }
                        div { class: "flex justify-end gap-2",
                            button {
                                r#type: "button",
                                class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD}",
                                onclick: move |_| open.set(false),
                                "Close"
                            }
                            button {
                                r#type: "button",
                                disabled: busy(),
                                class: "{BUTTON} {BUTTON_PRIMARY} {BUTTON_MD}",
                                onclick: move |_| {
                                    let text = note().trim().to_string();
                                    if text.is_empty() {
                                        status.set(Some("Describe the problem first.".to_string()));
                                        return;
                                    }
                                    busy.set(true);
                                    spawn(async move {
                                        match api::submit_report(game_id, port_id, text).await {
                                            Ok(_) => {
                                                status.set(Some("Thanks. A moderator will look at it.".to_string()));
                                                note.set(String::new());
                                            }
                                            Err(err) => status.set(Some(format!("Could not send the report: {err}"))),
                                        }
                                        busy.set(false);
                                    });
                                },
                                "Send report"
                            }
                        }
                    }
                }
            }
        }
    }
}
