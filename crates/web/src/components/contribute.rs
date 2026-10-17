//! "Contribute" strip on a port page: upload a screenshot or submit a gameplay video link.
//! Both are moderated before they show publicly, even from a signed-in account.

use base64::Engine;
use dioxus::prelude::*;

use crate::api;
use crate::components::SignedIn;
use crate::components::styles::*;

/// Largest file the browser will read before giving up and telling the uploader why.
const MAX_UPLOAD_MB: u64 = 10;

#[component]
pub fn ContributeSection(port_id: i64, port_name: String) -> Element {
    rsx! {
        section { class: "{CARD_SURFACE} p-6", "aria-labelledby": "contribute",
            h2 { id: "contribute", class: H2, "Help this listing" }
            p { class: "mt-1 text-sm text-muted-foreground",
                "Own a screenshot, a longplay or a review of "
                strong { "{port_name}" }
                "? Add it below, rate it and leave a comment. Shout out your channel in the comment; we're glad to have it."
            }
            SignedIn {
                prompt: "Sign in to add a screenshot, submit a video or leave a comment.".to_string(),
                div { class: "mt-6 grid gap-6 sm:grid-cols-2",
                    ScreenshotUpload { port_id }
                    VideoSubmit { port_id }
                }
            }
        }
    }
}

#[component]
fn ScreenshotUpload(port_id: i64) -> Element {
    let mut busy = use_signal(|| false);
    let mut message = use_signal(|| None::<Result<(), String>>);

    let onchange = move |evt: FormEvent| {
        let Some(file) = evt.files().into_iter().next() else {
            return;
        };
        let name = file.name();
        busy.set(true);
        message.set(None);
        spawn(async move {
            let result = async {
                let bytes = file
                    .read_bytes()
                    .await
                    .map_err(|e| format!("could not read {name}: {e}"))?;
                if bytes.len() as u64 > MAX_UPLOAD_MB * 1024 * 1024 {
                    return Err(format!("{name} is larger than {MAX_UPLOAD_MB} MB"));
                }
                let data_base64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                api::upload_port_image(port_id, name.clone(), data_base64)
                    .await
                    .map_err(|e| e.to_string())
            }
            .await;
            message.set(Some(result));
            busy.set(false);
        });
    };

    let status = match message() {
        Some(Ok(())) => Some((
            false,
            "Thanks! Your screenshot is waiting for review.".to_string(),
        )),
        Some(Err(e)) => Some((true, e)),
        None => None,
    };

    rsx! {
        div { class: "flex flex-col gap-2",
            p { class: "text-sm font-medium", "Add a screenshot" }
            label {
                class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD} w-fit cursor-pointer",
                if busy() { "Uploading…" } else { "Choose an image" }
                input {
                    r#type: "file",
                    accept: "image/*",
                    class: "sr-only",
                    disabled: busy(),
                    onchange,
                }
            }
            p { class: "text-xs text-muted-foreground", "Any image format, up to {MAX_UPLOAD_MB} MB. Converted and reviewed before it's shown." }
            if let Some((is_error, text)) = status {
                p { class: if is_error { "text-sm text-destructive" } else { "text-sm text-success" }, "{text}" }
            }
        }
    }
}

#[component]
fn VideoSubmit(port_id: i64) -> Element {
    let mut url = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut message = use_signal(|| None::<Result<(), String>>);

    let submit = move |e: FormEvent| {
        e.prevent_default();
        let link = url();
        if link.trim().is_empty() {
            return;
        }
        busy.set(true);
        message.set(None);
        spawn(async move {
            let result = api::submit_port_video(port_id, link)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string());
            if result.is_ok() {
                url.set(String::new());
            }
            message.set(Some(result));
            busy.set(false);
        });
    };

    let status = match message() {
        Some(Ok(())) => Some((
            false,
            "Thanks! Your video is waiting for review.".to_string(),
        )),
        Some(Err(e)) => Some((true, e)),
        None => None,
    };

    rsx! {
        form { class: "flex flex-col gap-2", onsubmit: submit,
            p { class: "text-sm font-medium", "Add a gameplay video" }
            div { class: "flex flex-wrap gap-2",
                input {
                    r#type: "url",
                    class: "{INPUT} min-w-0 flex-1",
                    placeholder: "https://www.youtube.com/watch?v=…",
                    value: "{url}",
                    disabled: busy(),
                    oninput: move |e| url.set(e.value()),
                }
                button {
                    r#type: "submit",
                    class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD}",
                    disabled: busy(),
                    if busy() { "Adding…" } else { "Add" }
                }
            }
            p { class: "text-xs text-muted-foreground", "A public YouTube link. Reviewed before it's shown; your channel is credited." }
            if let Some((is_error, text)) = status {
                p { class: if is_error { "text-sm text-destructive" } else { "text-sm text-success" }, "{text}" }
            }
        }
    }
}
