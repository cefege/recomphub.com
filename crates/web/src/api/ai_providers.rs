//! AI providers for the describe-ports job: the admin's ordered list and the logins that add to it.
//! Credentials are sealed with `SECRETS_KEY` and never returned to the browser.

use dioxus::prelude::*;
use recomphub_shared::dto::{
    AiDevicePoll, AiDeviceStart, AiOauthStart, AiProviderRow, AiProvidersView,
};

#[cfg(feature = "server")]
use std::time::Duration;

#[cfg(feature = "server")]
use crate::server::session::{db_error, http_error, require_admin};
#[cfg(feature = "server")]
use crate::server::state::pool;
#[cfg(feature = "server")]
use dioxus::server::axum::http::StatusCode;
#[cfg(feature = "server")]
use recomphub_db::PgPool;
#[cfg(feature = "server")]
use recomphub_db::ai_providers::{self as ai, Credential, PendingFlow, ProviderSummary};
#[cfg(feature = "server")]
use recomphub_db::secrets::Sealer;
#[cfg(feature = "server")]
use recomphub_ingest::llm::{self, oauth};
#[cfg(feature = "server")]
use serde_json::{Value, json};

#[cfg(feature = "server")]
const USER_AGENT: &str = "recomphub/1.0 (+https://recomphub.com)";
#[cfg(feature = "server")]
const MODEL_MAX_CHARS: usize = 100;
#[cfg(feature = "server")]
const LOGIN_NOT_FOUND: &str = "This login has expired or was already finished. Start again.";

#[cfg(feature = "server")]
fn internal(e: impl std::fmt::Display) -> ServerFnError {
    http_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

/// Failures talking to a provider or its login service. Shown to the admin as-is.
#[cfg(feature = "server")]
fn upstream(e: anyhow::Error) -> ServerFnError {
    http_error(StatusCode::BAD_GATEWAY, format!("{e:#}"))
}

#[cfg(feature = "server")]
fn bad_request(message: &str) -> ServerFnError {
    http_error(StatusCode::BAD_REQUEST, message.to_string())
}

/// Missing `SECRETS_KEY` means providers cannot be stored, so every write answers 503.
#[cfg(feature = "server")]
fn sealer() -> Result<Sealer, ServerFnError> {
    Sealer::from_env().map_err(|e| http_error(StatusCode::SERVICE_UNAVAILABLE, e.to_string()))
}

#[cfg(feature = "server")]
fn http_client() -> Result<reqwest::Client, ServerFnError> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(internal)
}

#[cfg(feature = "server")]
fn valid_model(model: &str) -> Result<String, ServerFnError> {
    let model = model.trim();
    let well_formed = !model.is_empty()
        && model.chars().count() <= MODEL_MAX_CHARS
        && model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | ':'));
    if well_formed {
        Ok(model.to_string())
    } else {
        Err(bad_request(
            "Model names use letters, digits, and - . _ : only.",
        ))
    }
}

/// Last four characters of a key, for the account label. Enough to tell keys apart, not to use one.
#[cfg(feature = "server")]
fn key_tail(key: &str) -> String {
    let tail: Vec<char> = key.chars().rev().take(4).collect();
    tail.into_iter().rev().collect()
}

#[cfg(feature = "server")]
fn row(p: ProviderSummary) -> AiProviderRow {
    AiProviderRow {
        id: p.id,
        position: p.position,
        kind: p.kind,
        model: p.model,
        enabled: p.enabled,
        account_label: p.account_label,
        last_error: p.last_error,
        last_error_at: p.last_error_at,
        last_used_at: p.last_used_at,
    }
}

#[cfg(feature = "server")]
async fn save_provider(
    pool: &PgPool,
    sealer: &Sealer,
    kind: &str,
    model: &str,
    label: &str,
    credential: &Credential,
) -> Result<AiProviderRow, ServerFnError> {
    let sealed = llm::seal_credential(sealer, credential).map_err(internal)?;
    let id = ai::insert(pool, kind, model, label, &sealed)
        .await
        .map_err(db_error)?;
    ai::list(pool)
        .await
        .map_err(db_error)?
        .into_iter()
        .find(|p| p.id == id)
        .map(row)
        .ok_or_else(|| internal("the saved provider is missing"))
}

#[cfg(feature = "server")]
fn flow_name(flow: &PendingFlow) -> &'static str {
    match flow {
        PendingFlow::Paste { .. } => "paste",
        PendingFlow::Device { .. } => "device",
    }
}

#[cfg(feature = "server")]
async fn store_pending(
    pool: &PgPool,
    sealer: &Sealer,
    id: &str,
    kind: &str,
    model: &str,
    flow: &PendingFlow,
) -> Result<(), ServerFnError> {
    let plain = serde_json::to_vec(flow).map_err(internal)?;
    let sealed = sealer.seal(&plain).map_err(internal)?;
    ai::purge_expired_pending(pool).await.map_err(db_error)?;
    ai::insert_pending(pool, id, kind, flow_name(flow), model, &sealed)
        .await
        .map_err(db_error)
}

/// Returns (kind, model, flow) for an unexpired login. The row stays until the caller finishes.
#[cfg(feature = "server")]
async fn take_pending(
    pool: &PgPool,
    sealer: &Sealer,
    id: &str,
) -> Result<(String, String, PendingFlow), ServerFnError> {
    let row = ai::get_pending(pool, id)
        .await
        .map_err(db_error)?
        .ok_or_else(|| http_error(StatusCode::NOT_FOUND, LOGIN_NOT_FOUND.to_string()))?;
    let plain = sealer.open(&row.sealed).map_err(internal)?;
    let flow: PendingFlow = serde_json::from_slice(&plain).map_err(internal)?;
    Ok((row.kind, row.model, flow))
}

/// The admin's ordered provider list, and whether `SECRETS_KEY` is set.
#[get("/api/admin/ai_providers")]
pub async fn admin_ai_providers() -> Result<AiProvidersView, ServerFnError> {
    require_admin().await?;
    let providers = ai::list(pool()?).await.map_err(db_error)?;
    Ok(AiProvidersView {
        secrets_configured: Sealer::from_env().is_ok(),
        providers: providers.into_iter().map(row).collect(),
    })
}

/// Adds an Anthropic or OpenAI API key at the end of the order.
#[post("/api/admin/ai_providers/add_api_key")]
pub async fn add_ai_api_key(
    kind: String,
    model: String,
    api_key: String,
) -> Result<AiProviderRow, ServerFnError> {
    require_admin().await?;
    if kind != ai::KIND_ANTHROPIC_API && kind != ai::KIND_OPENAI_API {
        return Err(bad_request(
            "Use the login buttons for subscription providers.",
        ));
    }
    let model = valid_model(&model)?;
    let key = api_key.trim();
    if key.is_empty() {
        return Err(bad_request("Enter an API key."));
    }
    let label = format!("API key ending {}", key_tail(key));
    let credential = Credential::ApiKey {
        api_key: key.to_string(),
    };
    save_provider(pool()?, &sealer()?, &kind, &model, &label, &credential).await
}

/// Starts an Anthropic (Claude Pro or Max) login. The admin opens the URL, then pastes the callback value.
#[post("/api/admin/ai_providers/start_anthropic")]
pub async fn start_ai_oauth(model: String) -> Result<AiOauthStart, ServerFnError> {
    require_admin().await?;
    let model = valid_model(&model)?;
    let sealer = sealer()?;
    let pool = pool()?;
    let verifier = oauth::random_urlsafe(32);
    let state = oauth::random_urlsafe(32);
    let authorize_url = oauth::anthropic_authorize_url(&verifier, &state).map_err(upstream)?;
    let pending_id = oauth::random_urlsafe(24);
    let flow = PendingFlow::Paste { verifier, state };
    store_pending(
        pool,
        &sealer,
        &pending_id,
        ai::KIND_ANTHROPIC_OAUTH,
        &model,
        &flow,
    )
    .await?;
    Ok(AiOauthStart {
        pending_id,
        authorize_url,
    })
}

/// Completes an Anthropic login with the value the callback page showed (a URL, or `code#state`).
#[post("/api/admin/ai_providers/finish_anthropic")]
pub async fn finish_ai_oauth(
    pending_id: String,
    pasted: String,
) -> Result<AiProviderRow, ServerFnError> {
    require_admin().await?;
    let sealer = sealer()?;
    let pool = pool()?;
    let (kind, model, flow) = take_pending(pool, &sealer, &pending_id).await?;
    if kind != ai::KIND_ANTHROPIC_OAUTH {
        return Err(bad_request("This login belongs to a different provider."));
    }
    let PendingFlow::Paste { verifier, state } = flow else {
        return Err(bad_request("This login was started another way."));
    };
    let http = http_client()?;
    let (credential, label) = oauth::anthropic_exchange(&http, &pasted, &verifier, &state)
        .await
        .map_err(upstream)?;
    let provider = save_provider(pool, &sealer, &kind, &model, &label, &credential).await?;
    ai::delete_pending(pool, &pending_id)
        .await
        .map_err(db_error)?;
    Ok(provider)
}

/// Starts a ChatGPT (Plus or Pro) device login. The admin enters the code at the verify URL, then checks.
#[post("/api/admin/ai_providers/start_device")]
pub async fn start_ai_device(model: String) -> Result<AiDeviceStart, ServerFnError> {
    require_admin().await?;
    let model = valid_model(&model)?;
    let sealer = sealer()?;
    let pool = pool()?;
    let http = http_client()?;
    let start = oauth::codex_device_start(&http).await.map_err(upstream)?;
    let pending_id = oauth::random_urlsafe(24);
    let flow = PendingFlow::Device {
        device_auth_id: start.device_auth_id,
        user_code: start.user_code.clone(),
        interval_secs: start.interval_secs,
    };
    store_pending(
        pool,
        &sealer,
        &pending_id,
        ai::KIND_OPENAI_CODEX_OAUTH,
        &model,
        &flow,
    )
    .await?;
    Ok(AiDeviceStart {
        pending_id,
        user_code: start.user_code,
        verify_url: oauth::CODEX_DEVICE_VERIFY_URL.to_string(),
    })
}

/// Checks a ChatGPT device login once. The provider is added once the admin has approved the code.
#[post("/api/admin/ai_providers/poll_device")]
pub async fn poll_ai_device(pending_id: String) -> Result<AiDevicePoll, ServerFnError> {
    require_admin().await?;
    let sealer = sealer()?;
    let pool = pool()?;
    let (kind, model, flow) = take_pending(pool, &sealer, &pending_id).await?;
    if kind != ai::KIND_OPENAI_CODEX_OAUTH {
        return Err(bad_request("This login belongs to a different provider."));
    }
    let PendingFlow::Device {
        device_auth_id,
        user_code,
        ..
    } = flow
    else {
        return Err(bad_request("This login was started another way."));
    };
    let http = http_client()?;
    match oauth::codex_device_poll(&http, &device_auth_id, &user_code)
        .await
        .map_err(upstream)?
    {
        oauth::DevicePoll::Pending => Ok(AiDevicePoll { provider: None }),
        oauth::DevicePoll::Ready {
            authorization_code,
            code_verifier,
        } => {
            let (credential, label) =
                oauth::codex_exchange(&http, &authorization_code, &code_verifier)
                    .await
                    .map_err(upstream)?;
            let provider = save_provider(pool, &sealer, &kind, &model, &label, &credential).await?;
            ai::delete_pending(pool, &pending_id)
                .await
                .map_err(db_error)?;
            Ok(AiDevicePoll {
                provider: Some(provider),
            })
        }
    }
}

/// Changes a provider's model and whether the describe job uses it.
#[post("/api/admin/ai_providers/update")]
pub async fn update_ai_provider(
    id: i64,
    model: String,
    enabled: bool,
) -> Result<(), ServerFnError> {
    require_admin().await?;
    let model = valid_model(&model)?;
    ai::update(pool()?, id, &model, enabled)
        .await
        .map_err(db_error)
}

/// Moves a provider one place up (`up`) or down the fallback order.
#[post("/api/admin/ai_providers/move")]
pub async fn move_ai_provider(id: i64, up: bool) -> Result<(), ServerFnError> {
    require_admin().await?;
    ai::move_by(pool()?, id, up).await.map_err(db_error)
}

#[post("/api/admin/ai_providers/delete")]
pub async fn delete_ai_provider(id: i64) -> Result<(), ServerFnError> {
    require_admin().await?;
    ai::delete(pool()?, id).await.map_err(db_error)
}

/// Sends one tiny structured request to an enabled provider and records the outcome.
#[post("/api/admin/ai_providers/test")]
pub async fn test_ai_provider(id: i64) -> Result<String, ServerFnError> {
    require_admin().await?;
    let sealer = sealer()?;
    let pool = pool()?;
    let provider = ai::list_enabled(pool)
        .await
        .map_err(db_error)?
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| bad_request("Enable this provider before testing it."))?;
    let http = http_client()?;
    let schema = json!({
        "type": "object",
        "properties": {"ok": {"type": "boolean"}},
        "required": ["ok"],
        "additionalProperties": false
    });
    let request = llm::Request {
        system: "Call record_ok with ok set to true.",
        user: "Connection test.",
        tool_name: "record_ok",
        tool_description: "Record that the connection works.",
        schema: &schema,
        max_tokens: 64,
    };
    let outcome: anyhow::Result<()> = async {
        let credential = oauth::ensure_fresh(pool, &http, &sealer, &provider).await?;
        let reply = llm::complete(
            &http,
            &provider.kind,
            &provider.model,
            &credential,
            &request,
        )
        .await?;
        anyhow::ensure!(
            reply.get("ok").and_then(Value::as_bool) == Some(true),
            "unexpected reply: {reply}"
        );
        Ok(())
    }
    .await;
    match outcome {
        Ok(()) => {
            ai::record_success(pool, id).await.map_err(db_error)?;
            Ok("The provider answered.".to_string())
        }
        Err(e) => {
            let message = format!("{e:#}");
            ai::record_failure(pool, id, &message)
                .await
                .map_err(db_error)?;
            Err(upstream(e))
        }
    }
}
