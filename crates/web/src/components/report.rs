use dioxus::prelude::*;

use crate::api;

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
            class: "text-sm text-muted underline underline-offset-4 hover:text-foreground",
            onclick: move |_| open.set(true),
            "Report a problem"
        }
        if open() {
            div { class: "fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4",
                div {
                    "role": "dialog",
                    "aria-modal": "true",
                    "aria-labelledby": "report-title",
                    class: "w-full max-w-lg rounded-2xl border border-border bg-surface p-6 shadow-2xl",
                    h2 { id: "report-title", class: "text-xl font-extrabold", "Report a problem" }
                    textarea {
                        class: "mt-4 w-full rounded-xl border border-border bg-background p-3 text-sm",
                        rows: "5",
                        placeholder: "What is wrong? (1000 characters at most)",
                        value: "{note}",
                        oninput: move |event| note.set(event.value()),
                    }
                    p { class: "mt-1 text-xs text-muted tabular", "{note().chars().count()} / 1000" }
                    if let Some(message) = status() {
                        p { class: "mt-3 text-sm", "{message}" }
                    }
                    div { class: "mt-5 flex justify-end gap-3",
                        button {
                            r#type: "button",
                            class: "rounded-full border border-border px-4 py-2 text-sm font-bold hover:bg-surface-2",
                            onclick: move |_| open.set(false),
                            "Close"
                        }
                        button {
                            r#type: "button",
                            disabled: busy(),
                            class: "rounded-full bg-accent px-4 py-2 text-sm font-bold text-white disabled:opacity-50",
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
