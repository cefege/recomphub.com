//! Member sign-in with Google or GitHub (OAuth 2.0 authorization code flow).
//! `/auth/{google,github}` starts the flow and `/auth/{google,github}/callback` verifies
//! `state`, exchanges the code server-side, and signs the account in. Identities are keyed on
//! the provider's stable id (Google `sub`, GitHub user id), never on email.
//!
//! Admin: a Google account whose email Google reports as verified and that is listed in
//! `ADMIN_EMAILS` is promoted, the same rule as the Cloudflare Access path in `cloudflare.rs`.
//! GitHub never grants admin.

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
    cloudflare::{is_admin_email, plain, redirect, safe_next, secure_attr},
    config::{self, Config},
    session, state,
};

const GITHUB_AUTHORIZE_URL: &str = "https://github.com/login/oauth/authorize";
const GITHUB_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const GITHUB_USER_URL: &str = "https://api.github.com/user";
const GOOGLE_AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_USERINFO_URL: &str = "https://openidconnect.googleapis.com/v1/userinfo";
/// Holds `state|next` between the start and the callback.
const OAUTH_COOKIE: &str = "rh_oauth";
const OAUTH_TTL_SECS: i64 = 600;
const USER_AGENT: &str = "recomphub/1.0 (+https://recomphub.com)";

/// A member sign-in provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuth {
    Google,
    GitHub,
}

impl OAuth {
    fn slug(self) -> &'static str {
        match self {
            OAuth::Google => "google",
            OAuth::GitHub => "github",
        }
    }

    fn label(self) -> &'static str {
        match self {
            OAuth::Google => "Google",
            OAuth::GitHub => "GitHub",
        }
    }

    /// The client id and secret, or a message naming the first missing variable.
    fn credentials(self, cfg: &Config) -> std::result::Result<(&str, &str), String> {
        let (id, secret, id_var, secret_var) = match self {
            OAuth::Google => (
                cfg.google_client_id.as_deref(),
                cfg.google_client_secret.as_deref(),
                "GOOGLE_OAUTH_CLIENT_ID",
                "GOOGLE_OAUTH_CLIENT_SECRET",
            ),
            OAuth::GitHub => (
                cfg.github_client_id.as_deref(),
                cfg.github_client_secret.as_deref(),
                "GITHUB_OAUTH_CLIENT_ID",
                "GITHUB_OAUTH_CLIENT_SECRET",
            ),
        };
        match (id, secret) {
            (Some(id), Some(secret)) => Ok((id, secret)),
            (None, _) => Err(format!("Sign-in is not set up yet: {id_var} is not set")),
            (Some(_), None) => Err(format!(
                "Sign-in is not set up yet: {secret_var} is not set"
            )),
        }
    }

    /// Whether the provider has both credentials, so the sign-in page can offer it.
    pub fn configured(self, cfg: &Config) -> bool {
        self.credentials(cfg).is_ok()
    }

    fn callback_url(self, cfg: &Config) -> String {
        format!("{}/auth/{}/callback", cfg.site_url, self.slug())
    }

    fn authorize_url(self, client_id: &str, redirect_uri: &str, state: &str) -> Result<String> {
        let url = match self {
            OAuth::GitHub => reqwest::Url::parse_with_params(
                GITHUB_AUTHORIZE_URL,
                &[
                    ("client_id", client_id),
                    ("redirect_uri", redirect_uri),
                    ("state", state),
                ],
            ),
            OAuth::Google => reqwest::Url::parse_with_params(
                GOOGLE_AUTHORIZE_URL,
                &[
                    ("client_id", client_id),
                    ("redirect_uri", redirect_uri),
                    ("state", state),
                    ("response_type", "code"),
                    ("scope", "openid email profile"),
                    ("prompt", "select_account"),
                ],
            ),
        }?;
        Ok(url.to_string())
    }
}

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

/// What a provider told us about the account, before the avatar is queued.
struct Account {
    provider: Provider,
    subject: String,
    login: Option<String>,
    email: Option<String>,
    email_verified: bool,
    name: Option<String>,
    avatar_url: Option<String>,
}

/// `GET /auth/google?next=`: starts Google sign-in.
pub async fn google_login(query: Query<LoginQuery>) -> Response {
    login(OAuth::Google, query)
}

/// `GET /auth/google/callback`: completes Google sign-in.
pub async fn google_callback(headers: HeaderMap, query: Query<CallbackQuery>) -> Response {
    callback(OAuth::Google, headers, query).await
}

/// `GET /auth/github?next=`: starts GitHub sign-in.
pub async fn github_login(query: Query<LoginQuery>) -> Response {
    login(OAuth::GitHub, query)
}

/// `GET /auth/github/callback`: completes GitHub sign-in.
pub async fn github_callback(headers: HeaderMap, query: Query<CallbackQuery>) -> Response {
    callback(OAuth::GitHub, headers, query).await
}

fn login(provider: OAuth, Query(query): Query<LoginQuery>) -> Response {
    let cfg = config::get();
    let (client_id, _) = match provider.credentials(cfg) {
        Ok(pair) => pair,
        Err(message) => return plain(StatusCode::SERVICE_UNAVAILABLE, message),
    };
    let next = safe_next(query.next.as_deref());
    let state = session::new_token();
    let url = match provider.authorize_url(client_id, &provider.callback_url(cfg), &state) {
        Ok(url) => url,
        Err(err) => {
            tracing::error!(error = %err, provider = provider.slug(), "building the authorize URL failed");
            return plain(StatusCode::INTERNAL_SERVER_ERROR, "sign-in failed");
        }
    };
    let cookie = format!(
        "{OAUTH_COOKIE}={state}|{next}; Path=/; HttpOnly; SameSite=Lax; Max-Age={OAUTH_TTL_SECS}{}",
        secure_attr(cfg)
    );
    redirect(&url, &[cookie])
}

async fn callback(
    provider: OAuth,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let cfg = config::get();
    let label = provider.label();
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
        return plain(
            StatusCode::BAD_REQUEST,
            format!("{label} sign-in was not completed"),
        );
    }
    let Some(code) = query.code.as_deref() else {
        return plain(
            StatusCode::BAD_REQUEST,
            format!("{label} sign-in returned no code"),
        );
    };
    let (client_id, client_secret) = match provider.credentials(cfg) {
        Ok(pair) => pair,
        Err(message) => return plain(StatusCode::SERVICE_UNAVAILABLE, message),
    };

    let redirect_uri = provider.callback_url(cfg);
    let fetched = match provider {
        OAuth::GitHub => fetch_github(client_id, client_secret, &redirect_uri, code).await,
        OAuth::Google => fetch_google(client_id, client_secret, &redirect_uri, code).await,
    };
    let account = match fetched {
        Ok(account) => account,
        Err(err) => {
            tracing::error!(error = %err, provider = provider.slug(), "sign-in exchange failed");
            return plain(
                StatusCode::BAD_GATEWAY,
                format!("could not complete sign-in with {label}; try again"),
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
    let is_admin = account.provider == Provider::Google
        && account.email_verified
        && account
            .email
            .as_deref()
            .is_some_and(|email| is_admin_email(email, &cfg.admin_emails));
    let profile = IdentityProfile {
        provider: account.provider,
        subject: account.subject,
        login: account.login,
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

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(20))
        .build()
        .context("building the HTTP client")
}

/// Exchanges the code for a token and reads the GitHub user it belongs to.
async fn fetch_github(
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
    struct User {
        id: i64,
        login: String,
        name: Option<String>,
        avatar_url: Option<String>,
    }
    let http = http_client()?;
    let reply: Token = http
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
    let user: User = http
        .get(GITHUB_USER_URL)
        .bearer_auth(&access_token)
        .header(header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .context("requesting the GitHub user")?
        .error_for_status()
        .context("GitHub rejected the user request")?
        .json()
        .await
        .context("reading the GitHub user")?;
    Ok(Account {
        provider: Provider::GitHub,
        subject: user.id.to_string(),
        login: Some(user.login),
        email: None,
        email_verified: false,
        name: user.name,
        avatar_url: user.avatar_url.as_deref().map(sized_github_avatar),
    })
}

/// Exchanges the code for a token and reads the Google account from the OpenID userinfo
/// endpoint. The reply comes straight from Google over TLS, so the ID token is not needed.
async fn fetch_google(
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
    let http = http_client()?;
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
        provider: Provider::Google,
        subject: info.sub,
        login: None,
        email: info.email.map(|email| email.trim().to_string()),
        email_verified: info.email_verified,
        name: info.name,
        avatar_url: info.picture,
    })
}

/// GitHub avatar URLs usually carry a query string already, such as `?v=4`.
fn sized_github_avatar(url: &str) -> String {
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
