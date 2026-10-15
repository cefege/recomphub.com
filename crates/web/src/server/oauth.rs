//! GitHub OAuth sign-in and sign-out. These are plain axum handlers, mounted
//! outside Dioxus server functions because they set cookies and redirect.

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use dioxus::server::axum::extract::Query;
use dioxus::server::axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use dioxus::server::axum::response::{IntoResponse, Response};
use recomphub_db::media;
use recomphub_db::users_sessions::{self, GithubProfile};
use serde::Deserialize;

use super::{
    config::{self, Config},
    session, state,
};

/// Holds `state|next` while the user is away on GitHub.
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
struct TokenReply {
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

/// `GET /auth/github/login?next=`: starts the OAuth flow.
pub async fn login(Query(query): Query<LoginQuery>) -> Response {
    let cfg = config::get();
    if let Err(message) = oauth_credentials(cfg) {
        return plain(StatusCode::SERVICE_UNAVAILABLE, message);
    }
    let client_id = cfg.github_client_id.as_deref().unwrap_or_default();
    let next = safe_next(query.next.as_deref());
    let state = session::new_token();
    let redirect_uri = callback_url(cfg);
    let url = match reqwest::Url::parse_with_params(
        "https://github.com/login/oauth/authorize",
        &[
            ("client_id", client_id),
            ("redirect_uri", redirect_uri.as_str()),
            ("state", state.as_str()),
        ],
    ) {
        Ok(url) => url,
        Err(err) => {
            tracing::error!(error = %err, "building the GitHub authorize URL failed");
            return plain(StatusCode::INTERNAL_SERVER_ERROR, "sign-in failed");
        }
    };
    let cookie = format!(
        "{OAUTH_COOKIE}={state}|{next}; Path=/; HttpOnly; SameSite=Lax; Max-Age={OAUTH_TTL_SECS}{}",
        secure_attr(cfg)
    );
    redirect(StatusCode::FOUND, url.as_str(), &[cookie])
}

/// `GET /auth/github/callback`: verifies state, exchanges the code, and signs the user in.
pub async fn callback(headers: HeaderMap, Query(query): Query<CallbackQuery>) -> Response {
    let cfg = config::get();
    let Some(cookie) = cookie_value(&headers, OAUTH_COOKIE) else {
        return plain(StatusCode::BAD_REQUEST, "sign-in expired; start again");
    };
    let Some((expected_state, next)) = cookie.split_once('|') else {
        return plain(StatusCode::BAD_REQUEST, "sign-in expired; start again");
    };
    let state_matches =
        !expected_state.is_empty() && query.state.as_deref() == Some(expected_state);
    if !state_matches {
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
    if let Err(message) = oauth_credentials(cfg) {
        return plain(StatusCode::SERVICE_UNAVAILABLE, message);
    }
    let next = safe_next(Some(next));

    let github = match fetch_github_user(cfg, code).await {
        Ok(user) => user,
        Err(err) => {
            tracing::error!(error = %err, "GitHub OAuth exchange failed");
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

    let avatar_media_id = match github.avatar_url.as_deref() {
        Some(url) => match media::enqueue(pool, "avatar", &sized_avatar_url(url)).await {
            Ok(id) => Some(id),
            Err(err) => {
                tracing::warn!(error = %err, "could not queue the avatar; signing in without it");
                None
            }
        },
        None => None,
    };
    let profile = GithubProfile {
        github_id: github.id,
        login: github.login.clone(),
        name: github.name.clone(),
        avatar_media_id,
    };
    let is_admin = cfg
        .admin_logins
        .contains(&github.login.to_ascii_lowercase());
    let user = match users_sessions::upsert_github_user(pool, &profile, is_admin).await {
        Ok(user) => user,
        Err(err) => {
            tracing::error!(error = ?err, "saving the GitHub user failed");
            return plain(StatusCode::INTERNAL_SERVER_ERROR, "sign-in failed");
        }
    };
    if user.banned {
        return plain(StatusCode::FORBIDDEN, "this account is banned");
    }

    let token = session::new_token();
    let expires_at = Utc::now() + chrono::Duration::days(session::SESSION_DAYS);
    if let Err(err) = users_sessions::create_session(pool, &token, user.id, expires_at).await {
        tracing::error!(error = ?err, "creating the session failed");
        return plain(StatusCode::INTERNAL_SERVER_ERROR, "sign-in failed");
    }

    let max_age = session::SESSION_DAYS * 24 * 60 * 60;
    let secure = secure_attr(cfg);
    let session_cookie = format!(
        "{}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}",
        session::SESSION_COOKIE
    );
    let clear_oauth = format!("{OAUTH_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{secure}");
    redirect(StatusCode::FOUND, &next, &[session_cookie, clear_oauth])
}

/// `POST /auth/logout`: deletes the session and expires its cookie.
pub async fn logout(headers: HeaderMap) -> Response {
    if let Some(token) = session::token_from_headers(&headers) {
        let pool = match state::pool() {
            Ok(pool) => pool,
            Err(err) => {
                tracing::error!(error = %err, "database pool unavailable during logout");
                return plain(StatusCode::INTERNAL_SERVER_ERROR, "logout failed");
            }
        };
        if let Err(err) = users_sessions::delete_session(pool, &token).await {
            tracing::error!(error = ?err, "deleting the session failed");
            return plain(StatusCode::INTERNAL_SERVER_ERROR, "logout failed");
        }
    }
    let secure = secure_attr(config::get());
    let clear = format!(
        "{}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{secure}",
        session::SESSION_COOKIE
    );
    redirect(StatusCode::SEE_OTHER, "/", &[clear])
}

/// Exchanges the authorization code for a token and reads the GitHub profile.
/// The access token is used once and discarded.
async fn fetch_github_user(cfg: &Config, code: &str) -> Result<GithubUser> {
    let client_id = cfg
        .github_client_id
        .as_deref()
        .context("GITHUB_OAUTH_CLIENT_ID is not set")?;
    let client_secret = cfg
        .github_client_secret
        .as_deref()
        .context("GITHUB_OAUTH_CLIENT_SECRET is not set")?;
    let http = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(20))
        .build()
        .context("building the HTTP client")?;

    let reply: TokenReply = http
        .post("https://github.com/login/oauth/access_token")
        .header(header::ACCEPT, "application/json")
        .json(&serde_json::json!({
            "client_id": client_id,
            "client_secret": client_secret,
            "code": code,
            "redirect_uri": callback_url(cfg),
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

    http.get("https://api.github.com/user")
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

/// Returns a 503 message naming the first missing OAuth credential.
fn oauth_credentials(cfg: &Config) -> std::result::Result<(), &'static str> {
    if cfg.github_client_id.is_none() {
        return Err("GitHub sign-in is not configured: GITHUB_OAUTH_CLIENT_ID is not set");
    }
    if cfg.github_client_secret.is_none() {
        return Err("GitHub sign-in is not configured: GITHUB_OAUTH_CLIENT_SECRET is not set");
    }
    Ok(())
}

fn callback_url(cfg: &Config) -> String {
    format!("{}/auth/github/callback", cfg.site_url)
}

fn secure_attr(cfg: &Config) -> &'static str {
    if cfg.site_url.starts_with("https://") {
        "; Secure"
    } else {
        ""
    }
}

/// GitHub avatar URLs usually carry a query string already, such as `?v=4`.
fn sized_avatar_url(url: &str) -> String {
    if url.contains('?') {
        format!("{url}&s=80")
    } else {
        format!("{url}?s=80")
    }
}

/// Accepts only same-origin paths that are safe inside a `Location` header and a cookie.
pub fn safe_next(raw: Option<&str>) -> String {
    match raw {
        Some(next)
            if next.starts_with('/')
                && !next.starts_with("//")
                && next.bytes().all(|b| {
                    b.is_ascii_graphic() && !matches!(b, b';' | b',' | b'"' | b'\\' | b'|')
                }) =>
        {
            next.to_string()
        }
        _ => "/".to_string(),
    }
}

/// The value of a named cookie from every `Cookie` header.
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

fn plain(status: StatusCode, message: impl Into<String>) -> Response {
    (status, message.into()).into_response()
}

/// A redirect carrying the given `Set-Cookie` values.
fn redirect(status: StatusCode, location: &str, cookies: &[String]) -> Response {
    let mut response = (status, [(header::LOCATION, location.to_string())]).into_response();
    for cookie in cookies {
        match HeaderValue::from_str(cookie) {
            Ok(value) => {
                response.headers_mut().append(header::SET_COOKIE, value);
            }
            Err(err) => {
                tracing::error!(error = %err, "refusing to send an invalid cookie");
                return plain(StatusCode::INTERNAL_SERVER_ERROR, "sign-in failed");
            }
        }
    }
    response
}
