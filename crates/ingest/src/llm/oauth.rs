//! Connecting and refreshing the two subscription logins.
//!
//! Anthropic (Claude Pro/Max) uses the authorization-code flow with PKCE. The admin opens a URL,
//! signs in, and pastes back the code shown on the callback page. ChatGPT (Plus/Pro) uses the
//! Codex device flow: the admin enters a short code at a URL, and the server polls until it
//! completes. Client ids, endpoints, and scopes are OMP's (`packages/catalog/src/compat/rules/auth/`,
//! `packages/ai/src/registry/oauth/`).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::Rng;
use recomphub_db::ai_providers::{
    Credential, EnabledProvider, KIND_ANTHROPIC_OAUTH, KIND_OPENAI_CODEX_OAUTH,
};
use recomphub_db::secrets::Sealer;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use super::{open_credential, seal_credential};

const ANTHROPIC_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const ANTHROPIC_AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
const ANTHROPIC_TOKEN_URL: &str = "https://api.anthropic.com/v1/oauth/token";
const ANTHROPIC_REDIRECT_URI: &str = "http://localhost:54545/callback";
const ANTHROPIC_SCOPES: &str = "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";
/// Claude Code SDK version in the refresh User-Agent (OMP `claudeCodeSdkVersion`).
const CLAUDE_CODE_SDK_VERSION: &str = "0.112.1";
/// Anthropic access tokens are treated as expired this long before the server says so.
const ANTHROPIC_SKEW_MS: i64 = 300_000;

const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const CODEX_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const CODEX_DEVICE_USERCODE_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/usercode";
const CODEX_DEVICE_TOKEN_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/token";
const CODEX_DEVICE_REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";
/// Page the admin opens to enter the device code.
pub const CODEX_DEVICE_VERIFY_URL: &str = "https://auth.openai.com/codex/device";
const CODEX_AUTH_CLAIM: &str = "https://api.openai.com/auth";
const CODEX_PROFILE_CLAIM: &str = "https://api.openai.com/profile";

const TOKEN_TIMEOUT: Duration = Duration::from_secs(15);
/// Access tokens are refreshed when they expire within this margin.
const REFRESH_MARGIN_MS: i64 = 60_000;

/// What the admin needs to finish a device login.
pub struct DeviceStart {
    pub device_auth_id: String,
    pub user_code: String,
    pub interval_secs: u64,
}

/// Result of one device-flow poll.
pub enum DevicePoll {
    Pending,
    Ready {
        authorization_code: String,
        code_verifier: String,
    },
}

/// Random URL-safe string from `bytes` random bytes, without padding.
pub fn random_urlsafe(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::rng().fill(&mut buf[..]);
    URL_SAFE_NO_PAD.encode(buf)
}

/// PKCE S256 challenge for a verifier.
pub fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// URL the admin opens to start the Anthropic login.
pub fn anthropic_authorize_url(verifier: &str, state: &str) -> Result<String> {
    let challenge = pkce_challenge(verifier);
    let url = reqwest::Url::parse_with_params(
        ANTHROPIC_AUTHORIZE_URL,
        &[
            ("code", "true"),
            ("client_id", ANTHROPIC_CLIENT_ID),
            ("response_type", "code"),
            ("redirect_uri", ANTHROPIC_REDIRECT_URI),
            ("scope", ANTHROPIC_SCOPES),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", state),
        ],
    )?;
    Ok(url.into())
}

/// Splits what the admin pasted into `(code, state)`. Accepts the full callback URL, or the
/// `code#state` form the Anthropic callback page shows.
pub fn parse_pasted_code(pasted: &str) -> Result<(String, Option<String>)> {
    let trimmed = pasted.trim();
    let (code, state) = if trimmed.contains("://") {
        let url = reqwest::Url::parse(trimmed).context("pasted value is not a URL")?;
        let mut code = None;
        let mut state = None;
        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                "code" => code = Some(value.into_owned()),
                "state" => state = Some(value.into_owned()),
                _ => {}
            }
        }
        (code, state)
    } else if let Some((code, state)) = trimmed.split_once('#') {
        (Some(code.to_string()), Some(state.to_string()))
    } else {
        (Some(trimmed.to_string()), None)
    };
    let code = code
        .filter(|c| !c.is_empty())
        .context("no authorization code found in the pasted value")?;
    Ok((code, state.filter(|s| !s.is_empty())))
}

/// Completes the Anthropic login from the pasted callback value.
pub async fn anthropic_exchange(
    http: &reqwest::Client,
    pasted: &str,
    verifier: &str,
    expected_state: &str,
) -> Result<(Credential, String)> {
    let (code, state) = parse_pasted_code(pasted)?;
    if let Some(state) = state.as_deref()
        && state != expected_state
    {
        bail!("the pasted code belongs to a different sign-in; start the connection again");
    }
    let body = json!({
        "grant_type": "authorization_code",
        "client_id": ANTHROPIC_CLIENT_ID,
        "code": code,
        "redirect_uri": ANTHROPIC_REDIRECT_URI,
        "code_verifier": verifier,
        "state": expected_state,
    });
    let reply = http
        .post(ANTHROPIC_TOKEN_URL)
        .timeout(TOKEN_TIMEOUT)
        .json(&body)
        .send()
        .await
        .context("exchanging the Anthropic authorization code")?;
    let status = reply.status();
    let text = reply.text().await.context("reading the token response")?;
    if !status.is_success() {
        bail!("token exchange failed: {status} {text}");
    }
    let tokens: Value = serde_json::from_str(&text).context("parsing the token response")?;
    let label = tokens["account"]["email_address"]
        .as_str()
        .or_else(|| tokens["organization"]["name"].as_str())
        .unwrap_or("Claude account")
        .to_string();
    let credential = Credential::OAuth {
        access_token: required(&tokens, "access_token")?,
        refresh_token: required(&tokens, "refresh_token")?,
        expires_at_ms: anthropic_expiry(&tokens),
        account_id: None,
    };
    Ok((credential, label))
}

/// Refreshes an Anthropic OAuth credential.
async fn anthropic_refresh(http: &reqwest::Client, refresh_token: &str) -> Result<Credential> {
    let body = json!({
        "grant_type": "refresh_token",
        "client_id": ANTHROPIC_CLIENT_ID,
        "refresh_token": refresh_token,
    });
    let reply = http
        .post(ANTHROPIC_TOKEN_URL)
        .timeout(TOKEN_TIMEOUT)
        .header("anthropic-beta", "oauth-2025-04-20")
        .header(
            reqwest::header::USER_AGENT,
            format!("anthropic-sdk-typescript/{CLAUDE_CODE_SDK_VERSION} userOAuthProvider"),
        )
        .json(&body)
        .send()
        .await
        .context("refreshing the Anthropic token")?;
    let status = reply.status();
    let text = reply.text().await.context("reading the refresh response")?;
    if status == reqwest::StatusCode::BAD_REQUEST && text.contains("invalid_grant") {
        bail!("Refresh token expired; reconnect this provider");
    }
    if !status.is_success() {
        bail!("token refresh failed: {status} {text}");
    }
    let tokens: Value = serde_json::from_str(&text).context("parsing the refresh response")?;
    Ok(Credential::OAuth {
        access_token: required(&tokens, "access_token")?,
        refresh_token: tokens["refresh_token"]
            .as_str()
            .unwrap_or(refresh_token)
            .to_string(),
        expires_at_ms: anthropic_expiry(&tokens),
        account_id: None,
    })
}

fn anthropic_expiry(tokens: &Value) -> i64 {
    let expires_in = tokens["expires_in"].as_i64().unwrap_or(3600);
    now_ms() + expires_in * 1000 - ANTHROPIC_SKEW_MS
}

/// Starts a Codex device login.
pub async fn codex_device_start(http: &reqwest::Client) -> Result<DeviceStart> {
    let reply = http
        .post(CODEX_DEVICE_USERCODE_URL)
        .timeout(TOKEN_TIMEOUT)
        .json(&json!({"client_id": CODEX_CLIENT_ID}))
        .send()
        .await
        .context("requesting a ChatGPT device code")?;
    let status = reply.status();
    let text = reply
        .text()
        .await
        .context("reading the device code response")?;
    if !status.is_success() {
        bail!("device code request failed: {status} {text}");
    }
    let body: Value = serde_json::from_str(&text).context("parsing the device code response")?;
    let interval_secs = match &body["interval"] {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
    .unwrap_or(5)
    .max(1);
    Ok(DeviceStart {
        device_auth_id: required(&body, "device_auth_id")?,
        user_code: required(&body, "user_code")?,
        interval_secs,
    })
}

/// Polls a Codex device login once. 403 and 404 mean the admin has not finished yet.
pub async fn codex_device_poll(
    http: &reqwest::Client,
    device_auth_id: &str,
    user_code: &str,
) -> Result<DevicePoll> {
    let reply = http
        .post(CODEX_DEVICE_TOKEN_URL)
        .timeout(TOKEN_TIMEOUT)
        .json(&json!({"device_auth_id": device_auth_id, "user_code": user_code}))
        .send()
        .await
        .context("polling the ChatGPT device login")?;
    let status = reply.status();
    if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::NOT_FOUND {
        return Ok(DevicePoll::Pending);
    }
    let text = reply
        .text()
        .await
        .context("reading the device login response")?;
    if !status.is_success() {
        bail!("device login failed: {status} {text}");
    }
    let body: Value = serde_json::from_str(&text).context("parsing the device login response")?;
    Ok(DevicePoll::Ready {
        authorization_code: required(&body, "authorization_code")?,
        code_verifier: required(&body, "code_verifier")?,
    })
}

/// Completes a Codex device login. Returns the credential and a label like "name@example.com (plus)".
pub async fn codex_exchange(
    http: &reqwest::Client,
    authorization_code: &str,
    code_verifier: &str,
) -> Result<(Credential, String)> {
    let reply = http
        .post(CODEX_TOKEN_URL)
        .timeout(TOKEN_TIMEOUT)
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CODEX_CLIENT_ID),
            ("code", authorization_code),
            ("code_verifier", code_verifier),
            ("redirect_uri", CODEX_DEVICE_REDIRECT_URI),
        ])
        .send()
        .await
        .context("exchanging the ChatGPT authorization code")?;
    let status = reply.status();
    let text = reply.text().await.context("reading the token response")?;
    if !status.is_success() {
        bail!("token exchange failed: {status} {text}");
    }
    let tokens: Value = serde_json::from_str(&text).context("parsing the token response")?;
    let access_token = required(&tokens, "access_token")?;
    let claims = tokens["id_token"]
        .as_str()
        .and_then(jwt_claims)
        .into_iter()
        .chain(jwt_claims(&access_token))
        .collect::<Vec<_>>();
    let account_id = claims
        .iter()
        .find_map(|c| c[CODEX_AUTH_CLAIM]["chatgpt_account_id"].as_str())
        .map(str::to_string)
        .context("token response has no ChatGPT account id")?;
    let label = codex_label(&claims);
    let credential = Credential::OAuth {
        access_token,
        refresh_token: required(&tokens, "refresh_token")?,
        expires_at_ms: now_ms() + tokens["expires_in"].as_i64().unwrap_or(3600) * 1000,
        account_id: Some(account_id),
    };
    Ok((credential, label))
}

fn codex_label(claims: &[Value]) -> String {
    let email = claims
        .iter()
        .find_map(|c| c[CODEX_PROFILE_CLAIM]["email"].as_str());
    let plan = claims
        .iter()
        .find_map(|c| c[CODEX_AUTH_CLAIM]["chatgpt_plan_type"].as_str());
    match (email, plan) {
        (Some(email), Some(plan)) => format!("{email} ({plan})"),
        (Some(email), None) => email.to_string(),
        (None, _) => "ChatGPT account".to_string(),
    }
}

/// Refreshes a Codex credential. Keeps the previous account id when the new token has no claims.
async fn codex_refresh(
    http: &reqwest::Client,
    refresh_token: &str,
    previous_account_id: Option<&str>,
) -> Result<Credential> {
    let reply = http
        .post(CODEX_TOKEN_URL)
        .timeout(TOKEN_TIMEOUT)
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", CODEX_CLIENT_ID),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await
        .context("refreshing the ChatGPT token")?;
    let status = reply.status();
    let text = reply.text().await.context("reading the refresh response")?;
    if !status.is_success() {
        bail!("token refresh failed: {status} {text}");
    }
    let tokens: Value = serde_json::from_str(&text).context("parsing the refresh response")?;
    let access_token = required(&tokens, "access_token")?;
    let account_id = jwt_claims(&access_token)
        .and_then(|c| {
            c[CODEX_AUTH_CLAIM]["chatgpt_account_id"]
                .as_str()
                .map(str::to_string)
        })
        .or_else(|| previous_account_id.map(str::to_string));
    Ok(Credential::OAuth {
        access_token,
        refresh_token: tokens["refresh_token"]
            .as_str()
            .unwrap_or(refresh_token)
            .to_string(),
        expires_at_ms: now_ms() + tokens["expires_in"].as_i64().unwrap_or(3600) * 1000,
        account_id,
    })
}

/// Payload of a JWT, without verifying the signature. Used only for the account claims the
/// token endpoint returned over TLS.
fn jwt_claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn required(body: &Value, field: &str) -> Result<String> {
    body[field]
        .as_str()
        .map(str::to_string)
        .with_context(|| format!("token response is missing {field}"))
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Returns a usable credential for an enabled provider, refreshing an OAuth token first when it
/// is close to expiry. The refreshed credential is saved, so the next run reuses it.
pub async fn ensure_fresh(
    pool: &PgPool,
    http: &reqwest::Client,
    sealer: &Sealer,
    provider: &EnabledProvider,
) -> Result<Credential> {
    let credential = open_credential(sealer, provider)?;
    let Credential::OAuth {
        refresh_token,
        expires_at_ms,
        account_id,
        ..
    } = &credential
    else {
        return Ok(credential);
    };
    if *expires_at_ms - now_ms() > REFRESH_MARGIN_MS {
        return Ok(credential);
    }
    let refreshed = match provider.kind.as_str() {
        KIND_ANTHROPIC_OAUTH => anthropic_refresh(http, refresh_token).await?,
        KIND_OPENAI_CODEX_OAUTH => {
            codex_refresh(http, refresh_token, account_id.as_deref()).await?
        }
        other => bail!("provider kind {other} has no OAuth refresh"),
    };
    let sealed = seal_credential(sealer, &refreshed)?;
    recomphub_db::ai_providers::update_credential(pool, provider.id, &sealed)
        .await
        .context("saving the refreshed provider credential")?;
    Ok(refreshed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pasted_code_accepts_url_and_hash_forms() {
        let (code, state) =
            parse_pasted_code("http://localhost:54545/callback?code=abc&state=xyz").unwrap();
        assert_eq!((code.as_str(), state.as_deref()), ("abc", Some("xyz")));
        let (code, state) = parse_pasted_code("  abc#xyz \n").unwrap();
        assert_eq!((code.as_str(), state.as_deref()), ("abc", Some("xyz")));
        let (code, state) = parse_pasted_code("plaincode").unwrap();
        assert_eq!((code.as_str(), state), ("plaincode", None));
        assert!(parse_pasted_code("   ").is_err());
    }

    #[test]
    fn authorize_url_carries_pkce_and_fixed_params() {
        let url = anthropic_authorize_url("verifier", "state-1").unwrap();
        let parsed = reqwest::Url::parse(&url).unwrap();
        let params: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(params["code"], "true");
        assert_eq!(params["client_id"], ANTHROPIC_CLIENT_ID);
        assert_eq!(params["code_challenge_method"], "S256");
        assert_eq!(params["code_challenge"], pkce_challenge("verifier"));
        assert_eq!(params["state"], "state-1");
        assert_eq!(params["redirect_uri"], ANTHROPIC_REDIRECT_URI);
    }

    #[test]
    fn pkce_challenge_is_unpadded_base64url_sha256() {
        // RFC 7636 appendix B.
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn jwt_claims_reads_the_payload_segment() {
        let payload =
            URL_SAFE_NO_PAD.encode(br#"{"https://api.openai.com/profile":{"email":"a@b.c"}}"#);
        let token = format!("h.{payload}.s");
        let claims = jwt_claims(&token).unwrap();
        assert_eq!(claims[CODEX_PROFILE_CLAIM]["email"], "a@b.c");
        assert!(jwt_claims("not-a-jwt").is_none());
    }

    #[test]
    fn codex_label_joins_email_and_plan() {
        let claims = vec![
            json!({CODEX_PROFILE_CLAIM: {"email": "a@b.c"}}),
            json!({CODEX_AUTH_CLAIM: {"chatgpt_plan_type": "plus"}}),
        ];
        assert_eq!(codex_label(&claims), "a@b.c (plus)");
        assert_eq!(codex_label(&[]), "ChatGPT account");
    }

    #[test]
    fn anthropic_expiry_applies_the_skew() {
        let before = now_ms();
        let expiry = anthropic_expiry(&json!({"expires_in": 3600}));
        let expected = before + 3600 * 1000 - ANTHROPIC_SKEW_MS;
        assert!((expiry - expected).abs() < 5_000);
    }
}
