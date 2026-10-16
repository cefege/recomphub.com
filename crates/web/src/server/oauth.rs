//! Sign-in with Google (OAuth 2.0 authorization code flow). `/auth/google` starts the flow and
//! `/auth/google/callback` verifies `state`, exchanges the code server-side, reads the account
//! from Google's OpenID userinfo endpoint and starts the local session. Identities are keyed on
//! Google's `sub`, never on email. A verified email listed in `ADMIN_EMAILS` gets the admin role.

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use dioxus::server::axum::extract::Query;
use dioxus::server::axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use dioxus::server::axum::response::{IntoResponse, Response};
use recomphub_db::identities::{self, IdentityProfile};
use recomphub_db::{media, users_sessions};
use serde::Deserialize;

use super::{
    config::{self, Config},
    session, state,
};

const GOOGLE_AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_USERINFO_URL: &str = "https://openidconnect.googleapis.com/v1/userinfo";
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

/// What Google told us about the account, before the avatar is queued.
struct Account {
    subject: String,
    email: Option<String>,
    email_verified: bool,
    name: Option<String>,
    avatar_url: Option<String>,
}

/// The client id and secret, or a message naming the first missing variable.
fn credentials(cfg: &Config) -> std::result::Result<(&str, &str), String> {
    match (
        cfg.google_client_id.as_deref(),
        cfg.google_client_secret.as_deref(),
    ) {
        (Some(id), Some(secret)) => Ok((id, secret)),
        (None, _) => {
            Err("Sign-in is not set up yet: GOOGLE_OAUTH_CLIENT_ID is not set".to_string())
        }
        (Some(_), None) => {
            Err("Sign-in is not set up yet: GOOGLE_OAUTH_CLIENT_SECRET is not set".to_string())
        }
    }
}

fn callback_url(cfg: &Config) -> String {
    format!("{}/auth/google/callback", cfg.site_url)
}

/// `GET /auth/google?next=`: starts Google sign-in.
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
        GOOGLE_AUTHORIZE_URL,
        &[
            ("client_id", client_id),
            ("redirect_uri", redirect_uri.as_str()),
            ("state", state.as_str()),
            ("response_type", "code"),
            ("scope", "openid email profile"),
            ("prompt", "select_account"),
        ],
    ) {
        Ok(url) => url.to_string(),
        Err(err) => {
            tracing::error!(error = %err, "building the Google authorize URL failed");
            return plain(StatusCode::INTERNAL_SERVER_ERROR, "sign-in failed");
        }
    };
    let cookie = format!(
        "{OAUTH_COOKIE}={state}|{next}; Path=/; HttpOnly; SameSite=Lax; Max-Age={OAUTH_TTL_SECS}{}",
        secure_attr(cfg)
    );
    redirect(&url, &[cookie])
}

/// `GET /auth/google/callback`: completes Google sign-in and starts the local session.
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
        return plain(StatusCode::BAD_REQUEST, "Google sign-in was not completed");
    }
    let Some(code) = query.code.as_deref() else {
        return plain(StatusCode::BAD_REQUEST, "Google sign-in returned no code");
    };
    let (client_id, client_secret) = match credentials(cfg) {
        Ok(pair) => pair,
        Err(message) => return plain(StatusCode::SERVICE_UNAVAILABLE, message),
    };

    let account = match fetch_account(client_id, client_secret, &callback_url(cfg), code).await {
        Ok(account) => account,
        Err(err) => {
            tracing::error!(error = %err, "Google sign-in exchange failed");
            return plain(
                StatusCode::BAD_GATEWAY,
                "could not complete sign-in with Google; try again",
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
    let avatar_media_id = match account.avatar_url.as_deref() {
        Some(url) => match media::enqueue(pool, "avatar", url).await {
            Ok(id) => Some(id),
            Err(err) => {
                tracing::warn!(error = %err, "could not queue the avatar; continuing without it");
                None
            }
        },
        None => None,
    };
    let is_admin = account.email_verified
        && account
            .email
            .as_deref()
            .is_some_and(|email| is_admin_email(email, &cfg.admin_emails));
    let profile = IdentityProfile {
        subject: account.subject,
        email: account.email,
        name: account.name,
        avatar_media_id,
    };
    let user = match identities::sign_in(pool, &profile, is_admin).await {
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

/// `POST /auth/logout`: deletes the local session and expires its cookie.
pub async fn logout(headers: HeaderMap) -> Response {
    let cfg = config::get();
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
    let clear = format!(
        "{}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{}",
        session::SESSION_COOKIE,
        secure_attr(cfg)
    );
    redirect("/", &[clear])
}

/// Exchanges the code for a token and reads the Google account from the OpenID userinfo
/// endpoint. The reply comes straight from Google over TLS, so the ID token is not needed.
async fn fetch_account(
    client_id: &str,
    client_secret: &str,
    redirect_uri: &str,
    code: &str,
) -> Result<Account> {
    #[derive(Deserialize)]
    struct Token {
        access_token: Option<String>,
        error: Option<String>,
    }
    #[derive(Deserialize)]
    struct UserInfo {
        sub: String,
        email: Option<String>,
        #[serde(default)]
        email_verified: bool,
        name: Option<String>,
        picture: Option<String>,
    }
    let http = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(20))
        .build()
        .context("building the HTTP client")?;
    let reply: Token = http
        .post(GOOGLE_TOKEN_URL)
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await
        .context("requesting the Google access token")?
        .error_for_status()
        .context("Google rejected the token request")?
        .json()
        .await
        .context("reading the Google token reply")?;
    let access_token = reply.access_token.ok_or_else(|| {
        anyhow!(
            "Google returned no access token: {}",
            reply.error.unwrap_or_default()
        )
    })?;
    let info: UserInfo = http
        .get(GOOGLE_USERINFO_URL)
        .bearer_auth(&access_token)
        .send()
        .await
        .context("requesting the Google account")?
        .error_for_status()
        .context("Google rejected the userinfo request")?
        .json()
        .await
        .context("reading the Google account")?;
    if info.sub.is_empty() {
        return Err(anyhow!("Google returned an account without an id"));
    }
    Ok(Account {
        subject: info.sub,
        email: info.email.map(|email| email.trim().to_string()),
        email_verified: info.email_verified,
        name: info.name,
        avatar_url: info.picture,
    })
}

/// Admin role for a verified email listed in `ADMIN_EMAILS`. Lists are lowercased at startup.
pub fn is_admin_email(email: &str, admins: &[String]) -> bool {
    admins.contains(&email.trim().to_ascii_lowercase())
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

/// Redirects with the given `Set-Cookie` values. Cookie text that cannot be sent is a 500.
fn redirect(location: &str, cookies: &[String]) -> Response {
    let mut response = (
        StatusCode::FOUND,
        [(header::LOCATION, location.to_string())],
    )
        .into_response();
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

fn plain(status: StatusCode, message: impl Into<String>) -> Response {
    (status, message.into()).into_response()
}

/// `Secure` on the cookies whenever the site is served over HTTPS.
fn secure_attr(cfg: &Config) -> &'static str {
    if cfg.site_url.starts_with("https://") {
        "; Secure"
    } else {
        ""
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_grant_matches_the_verified_email_case_insensitively() {
        let admins = vec!["mateiasmihaiandrei@gmail.com".to_string()];
        assert!(is_admin_email("MateiasMihaiAndrei@Gmail.com", &admins));
        assert!(!is_admin_email("someone@example.com", &admins));
    }

    #[test]
    fn safe_next_keeps_same_origin_paths_and_rejects_the_rest() {
        assert_eq!(safe_next(Some("/account?x=1")), "/account?x=1");
        for raw in ["//evil", "/\\evil", "https://x", "/a b", "", "/a;b", "/a|b"] {
            assert_eq!(safe_next(Some(raw)), "/", "{raw:?} should fall back to /");
        }
        assert_eq!(safe_next(None), "/");
    }
}
