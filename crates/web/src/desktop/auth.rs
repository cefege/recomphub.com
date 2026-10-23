//! Native Google handoff. Only presentation state reaches components; verifier
//! and session credential remain in Rust and the OS credential store.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use dioxus::core::Task;
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::session::{AuthStatus, AuthView, SessionEnvelope, valid_token};
use super::{account, runtime};
use crate::components::{ViewerStore, set_me};

#[derive(Serialize)]
struct StartRequest {
    challenge: String,
}

#[derive(Deserialize)]
struct StartResponse {
    request_id: String,
    authorization_url: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Serialize)]
struct ExchangeRequest<'a> {
    request_id: &'a str,
    verifier: &'a str,
}

#[derive(Deserialize)]
struct ExchangeResponse {
    session_token: String,
    expires_at: DateTime<Utc>,
}

/// One root subscription owns identity presentation, expiry and focus refresh.
/// Device-local routes remain mounted regardless of this optional account state.
pub fn provide(viewer: ViewerStore) -> Signal<AuthView> {
    let mut view = use_signal(|| runtime().account.view());
    use_context_provider(|| view);
    use_future(move || async move {
        let runtime = runtime();
        let mut receiver = runtime.account.subscribe();
        loop {
            view.set(receiver.borrow_and_update().clone());
            if receiver.changed().await.is_err() {
                break;
            }
        }
    });
    use_effect(move || {
        let current = view.read();
        if current.status == AuthStatus::Checking {
            return;
        }
        let user = current.unlocked().then(|| current.user.clone()).flatten();
        if *viewer.me.peek() != user || !*viewer.ready.peek() {
            if viewer.me.peek().as_ref().map(|user| user.id) != user.as_ref().map(|user| user.id) {
                let mut starred = viewer.starred;
                let mut wanted = viewer.wanted;
                starred.write().clear();
                wanted.write().clear();
            }
            set_me(viewer, user);
        }
    });
    use_future(|| async {
        account::restore(runtime(), true).await;
    });
    use_future(|| async {
        let mut timer = tokio::time::interval(Duration::from_secs(1));
        loop {
            timer.tick().await;
            account::enforce_expiry(runtime()).await;
        }
    });
    // Focus and periodic reconnection checks cannot override browser sign-in:
    // refresh only begins while a previously validated account is unlocked.
    dioxus::desktop::use_wry_event_handler(move |event, _| {
        use dioxus::desktop::WindowEvent;
        use dioxus::desktop::tao::event::Event;
        if matches!(
            event,
            Event::WindowEvent {
                event: WindowEvent::Focused(true),
                ..
            }
        ) && runtime().account.view().unlocked()
        {
            spawn(account::restore(runtime(), false));
        }
    });
    use_future(|| async {
        let mut timer = tokio::time::interval(Duration::from_secs(30));
        timer.tick().await;
        loop {
            timer.tick().await;
            if runtime().account.view().status == AuthStatus::AuthenticatedOffline {
                account::restore(runtime(), false).await;
            }
        }
    });
    view
}

#[component]
pub fn ContinueWithGoogle() -> Element {
    let view = use_context::<Signal<AuthView>>();
    let mut task = use_signal(|| None::<Task>);
    let pending = view.read().status == AuthStatus::WaitingForBrowser;
    use_drop(move || {
        if runtime().account.view().status == AuthStatus::WaitingForBrowser {
            if let Some(task) = *task.peek() {
                task.cancel();
            }
            runtime().account.cancel();
            account::clear_cookie(&runtime());
        }
    });
    rsx! {
        button {
            r#type: "button",
            class: "mt-5 rounded-md bg-primary px-4 py-2 text-sm font-medium text-primary-foreground disabled:opacity-60",
            disabled: pending,
            onclick: move |_| {
                let generation = runtime().account.begin_browser();
                let handle = dioxus::core::spawn_forever(async move {
                    if let Err(message) = authorize(generation).await {
                        runtime().account.fail(generation, message);
                    }
                });
                task.set(Some(handle));
            },
            if pending { "Waiting for browser approval…" } else { "Continue with Google" }
        }
        if pending {
            button {
                r#type: "button",
                class: "ml-3 rounded-md border px-4 py-2 text-sm",
                onclick: move |_| {
                    if runtime().account.view().status == AuthStatus::WaitingForBrowser {
                        if let Some(handle) = task.write().take() { handle.cancel(); }
                        runtime().account.cancel();
                        account::clear_cookie(&runtime());
                    }
                },
                "Cancel"
            }
        }
    }
}

fn authorization_url(origin: &str, start: &StartResponse) -> Result<String, String> {
    if !valid_token(&start.request_id) || start.expires_in != 600 || start.interval != 3 {
        return Err("The server returned an invalid browser-approval request.".into());
    }
    let url = reqwest::Url::parse(&start.authorization_url)
        .map_err(|_| "The server returned an invalid browser-approval URL.".to_string())?;
    let parameters: Vec<_> = url.query_pairs().collect();
    if url.origin().ascii_serialization() != origin
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.path() != "/auth/desktop"
        || parameters.len() != 1
        || parameters[0].0 != "request_id"
        || parameters[0].1 != start.request_id
    {
        return Err(
            "The browser-approval URL did not match the configured RecompHub server.".into(),
        );
    }
    Ok(url.into())
}

async fn authorize(generation: u64) -> Result<(), String> {
    // Dropping/cancelling this future drops the sole verifier and all pending requests.
    let envelope = tokio::time::timeout(Duration::from_secs(600), authorize_inner(generation))
        .await
        .map_err(|_| {
            "Browser approval expired. Continue with Google to start again.".to_string()
        })??;
    let runtime = runtime();
    let Some(envelope) = envelope else {
        return Ok(());
    };
    if !runtime.account.current(generation) {
        return Ok(());
    }
    account::install_cookie(&runtime, &envelope)?;
    // Persistence remains root-owned after the gate unlocks. It must not be
    // aborted halfway through an OS write by the handoff timeout or unmount.
    runtime
        .account
        .accept_online(&runtime, generation, envelope)
        .await
}

async fn authorize_inner(generation: u64) -> Result<Option<SessionEnvelope>, String> {
    let runtime = runtime();
    let mut verifier = [0u8; 32];
    rand::fill(&mut verifier);
    let verifier_text = URL_SAFE_NO_PAD.encode(verifier);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier));
    let start: StartResponse = runtime
        .anonymous_api
        .post(format!("{}/api/v1/desktop/auth/start", runtime.origin))
        .json(&StartRequest { challenge })
        .send()
        .await
        .map_err(|_| {
            "Could not reach RecompHub to start sign-in. Retry when the server is available."
                .to_string()
        })?
        .error_for_status()
        .map_err(|_| "RecompHub could not start sign-in. Retry shortly.".to_string())?
        .json()
        .await
        .map_err(|_| "RecompHub returned an invalid sign-in response.".to_string())?;
    if !runtime.account.current(generation) {
        return Ok(None);
    }
    let url = authorization_url(&runtime.origin, &start)?;
    tokio::task::spawn_blocking(move || webbrowser::open(&url))
        .await.map_err(|_| "The system-browser worker stopped unexpectedly.".to_string())?
        .map_err(|_| "Could not open your default browser. Check your system browser setting, then retry sign-in.".to_string())?;
    loop {
        tokio::time::sleep(Duration::from_secs(3)).await;
        if !runtime.account.current(generation) {
            return Ok(None);
        }
        let reply = runtime
            .anonymous_api
            .post(format!("{}/api/v1/desktop/auth/exchange", runtime.origin))
            .json(&ExchangeRequest {
                request_id: &start.request_id,
                verifier: &verifier_text,
            })
            .send()
            .await
            .map_err(|_| {
                "Connection lost while waiting for browser approval. Retry sign-in.".to_string()
            })?;
        if !runtime.account.current(generation) {
            return Ok(None);
        }
        match reply.status() {
            reqwest::StatusCode::ACCEPTED => continue,
            reqwest::StatusCode::GONE => {
                return Err(
                    "Browser approval expired or was already exchanged. Start sign-in again."
                        .into(),
                );
            }
            reqwest::StatusCode::OK => {}
            _ => return Err("RecompHub refused browser approval. Start sign-in again.".into()),
        }
        let reply: ExchangeResponse = reply
            .json()
            .await
            .map_err(|_| "RecompHub returned an invalid desktop credential.".to_string())?;
        if !runtime.account.current(generation) {
            return Ok(None);
        }
        let envelope = SessionEnvelope {
            version: 1,
            origin: runtime.origin.clone(),
            session_token: reply.session_token,
            expires_at: reply.expires_at,
            validated_user: None,
        };
        return Ok(Some(envelope));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_handoff_rejects_changed_origin_extra_queries_and_noncanonical_ids() {
        let id = URL_SAFE_NO_PAD.encode([9; 32]);
        let mut start = StartResponse {
            request_id: id.clone(),
            authorization_url: format!("https://recomphub.com/auth/desktop?request_id={id}"),
            expires_in: 600,
            interval: 3,
        };
        assert!(authorization_url("https://recomphub.com", &start).is_ok());
        for url in [
            format!("https://evil.test/auth/desktop?request_id={id}"),
            format!("https://recomphub.com/auth/desktop?request_id={id}&next=evil"),
            format!("https://user@recomphub.com/auth/desktop?request_id={id}"),
            format!("https://recomphub.com/auth/desktop?request_id={id}#fragment"),
        ] {
            start.authorization_url = url;
            assert!(authorization_url("https://recomphub.com", &start).is_err());
        }
        start.request_id.push('=');
        assert!(authorization_url("https://recomphub.com", &start).is_err());
    }
}
