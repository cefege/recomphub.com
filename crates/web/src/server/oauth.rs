//! GitHub sign-in for members. `/auth/github` starts the OAuth flow and
//! `/auth/github/callback` verifies `state`, exchanges the code, and signs the GitHub account
//! in. Identities are keyed on the GitHub user id. GitHub never grants admin; that comes only
//! from the Cloudflare Access path in `cloudflare.rs`.

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use dioxus::server::axum::extract::Query;
use dioxus::server::axum::http::{HeaderMap, StatusCode, header};
use dioxus::server::axum::response::Response;
use recomphub_db::identities::{self, IdentityProfile, Provider};
use recomphub_db::{media, users_sessions};
use serde::Deserialize;

use super::{
    cloudflare::{plain, redirect, safe_next, secure_attr},
    config::{self, Config},
    session, state,
};

const GITHUB_AUTHORIZE_URL: &str = "https://github.com/login/oauth/authorize";
const GITHUB_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const GITHUB_USER_URL: &str = "https://api.github.com/user";
/// Holds `state|next` between the start and the callback.
const OAUTH_COOKIE: &str = "rh_oauth";
const OAUTH_TTL_SECS: i64 = 600;
const USER_AGENT: &str = "recomphub/1.0 (+https://recomphub.com)";

#[derive(Deserialize)]
pub struct LoginQuery {
    next: Option<String>,
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct GithubToken {
    access_token: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct GithubUser {
    id: i64,
    login: String,
    name: Option<String>,
    avatar_url: Option<String>,
}

/// `GET /auth/github?next=`: starts GitHub sign-in.
pub async fn login(Query(query): Query<LoginQuery>) -> Response {
    let cfg = config::get();
    let (client_id, _) = match credentials(cfg) {
        Ok(pair) => pair,
        Err(message) => return plain(StatusCode::SERVICE_UNAVAILABLE, message),
    };
    let next = safe_next(query.next.as_deref());
    let state = session::new_token();
    let redirect_uri = callback_url(cfg);
    let url = match reqwest::Url::parse_with_params(
        GITHUB_AUTHORIZE_URL,
        &[
            ("client_id", client_id),
            ("redirect_uri", redirect_uri.as_str()),
            ("state", state.as_str()),
        ],
    ) {
        Ok(url) => url.to_string(),
        Err(err) => {
            tracing::error!(error = %err, "building the GitHub authorize URL failed");
            return plain(StatusCode::INTERNAL_SERVER_ERROR, "sign-in failed");
        }
    };
    let cookie = format!(
        "{OAUTH_COOKIE}={state}|{next}; Path=/; HttpOnly; SameSite=Lax; Max-Age={OAUTH_TTL_SECS}{}",
        secure_attr(cfg)
    );
    redirect(&url, &[cookie])
}

/// `GET /auth/github/callback`: completes GitHub sign-in and starts the local session.
pub async fn callback(headers: HeaderMap, Query(query): Query<CallbackQuery>) -> Response {
    let cfg = config::get();
    let Some((expected, next)) = cookie_value(&headers, OAUTH_COOKIE)
        .and_then(|raw| raw.split_once('|'))
        .map(|(state, next)| (state.to_string(), next.to_string()))
    else {
        return plain(StatusCode::BAD_REQUEST, "sign-in expired; start again");
    };
    if expected.is_empty() || query.state.as_deref() != Some(expected.as_str()) {
        return plain(
            StatusCode::BAD_REQUEST,
            "OAuth state mismatch; start sign-in again",
        );
    }
    if query.error.is_some() {
        return plain(StatusCode::BAD_REQUEST, "GitHub sign-in was not completed");
    }
    let Some(code) = query.code.as_deref() else {
        return plain(StatusCode::BAD_REQUEST, "GitHub sign-in returned no code");
    };
    let (client_id, client_secret) = match credentials(cfg) {
        Ok(pair) => pair,
        Err(message) => return plain(StatusCode::SERVICE_UNAVAILABLE, message),
    };

    let user = match fetch_github_user(client_id, client_secret, &callback_url(cfg), code).await {
        Ok(user) => user,
        Err(err) => {
            tracing::error!(error = %err, "GitHub sign-in exchange failed");
            return plain(
                StatusCode::BAD_GATEWAY,
                "could not complete sign-in with GitHub; try again",
            );
        }
    };

    let pool = match state::pool() {
        Ok(pool) => pool,
        Err(err) => {
            tracing::error!(error = %err, "database pool unavailable during sign-in");
            return plain(StatusCode::INTERNAL_SERVER_ERROR, "sign-in failed");
        }
    };
    let avatar_media_id = match user.avatar_url.as_deref().map(sized_avatar_url) {
        Some(url) => match media::enqueue(pool, "avatar", &url).await {
            Ok(id) => Some(id),
            Err(err) => {
                tracing::warn!(error = %err, "could not queue the avatar; continuing without it");
                None
            }
        },
        None => None,
    };
    let profile = IdentityProfile {
        provider: Provider::GitHub,
        subject: user.id.to_string(),
        login: Some(user.login),
        email: None,
        name: user.name,
        avatar_media_id,
    };
    let user = match identities::sign_in(pool, &profile, false).await {
        Ok(user) if user.banned => return plain(StatusCode::FORBIDDEN, "this account is banned"),
        Ok(user) => user,
        Err(err) => {
            tracing::error!(error = ?err, "saving the sign-in failed");
            return plain(StatusCode::INTERNAL_SERVER_ERROR, "sign-in failed");
        }
    };

    let token = session::new_token();
    let expires_at = Utc::now() + chrono::Duration::days(session::SESSION_DAYS);
    if let Err(err) = users_sessions::create_session(pool, &token, user.id, expires_at).await {
        tracing::error!(error = ?err, "creating the session failed");
        return plain(StatusCode::INTERNAL_SERVER_ERROR, "sign-in failed");
    }

    let secure = secure_attr(cfg);
    let max_age = session::SESSION_DAYS * 24 * 60 * 60;
    let session_cookie = format!(
        "{}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}",
        session::SESSION_COOKIE
    );
    let clear_oauth = format!("{OAUTH_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{secure}");
    redirect(&safe_next(Some(&next)), &[session_cookie, clear_oauth])
}

/// The client id and secret, or a message naming the first missing variable.
fn credentials(cfg: &Config) -> std::result::Result<(&str, &str), String> {
    match (
        cfg.github_client_id.as_deref(),
        cfg.github_client_secret.as_deref(),
    ) {
        (Some(id), Some(secret)) => Ok((id, secret)),
        (None, _) => {
            Err("Sign-in is not set up yet: GITHUB_OAUTH_CLIENT_ID is not set".to_string())
        }
        (Some(_), None) => {
            Err("Sign-in is not set up yet: GITHUB_OAUTH_CLIENT_SECRET is not set".to_string())
        }
    }
}

fn callback_url(cfg: &Config) -> String {
    format!("{}/auth/github/callback", cfg.site_url)
}

/// Exchanges the code for a token and reads the GitHub user it belongs to.
async fn fetch_github_user(
    client_id: &str,
    client_secret: &str,
    redirect_uri: &str,
    code: &str,
) -> Result<GithubUser> {
    let http = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(20))
        .build()
        .context("building the HTTP client")?;

    let reply: GithubToken = http
        .post(GITHUB_TOKEN_URL)
        .header(header::ACCEPT, "application/json")
        .json(&serde_json::json!({
            "client_id": client_id,
            "client_secret": client_secret,
            "code": code,
            "redirect_uri": redirect_uri,
        }))
        .send()
        .await
        .context("requesting the GitHub access token")?
        .error_for_status()
        .context("GitHub rejected the token request")?
        .json()
        .await
        .context("reading the GitHub token reply")?;
    let access_token = reply.access_token.ok_or_else(|| {
        anyhow!(
            "GitHub returned no access token: {}",
            reply.error.unwrap_or_default()
        )
    })?;

    http.get(GITHUB_USER_URL)
        .bearer_auth(&access_token)
        .header(header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .context("requesting the GitHub user")?
        .error_for_status()
        .context("GitHub rejected the user request")?
        .json()
        .await
        .context("reading the GitHub user")
}

/// GitHub avatar URLs usually carry a query string already, such as `?v=4`.
fn sized_avatar_url(url: &str) -> String {
    if url.contains('?') {
        format!("{url}&s=80")
    } else {
        format!("{url}?s=80")
    }
}

/// The value of one cookie from the request headers.
fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| {
            let (key, value) = pair.trim().split_once('=')?;
            (key == name).then_some(value)
        })
}
