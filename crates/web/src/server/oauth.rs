//! Sign-in and sign-out for GitHub and Google, plus linking extra providers to a
//! signed-in account. These are plain axum handlers, mounted outside Dioxus server
//! functions because they set cookies and redirect.
//!
//! Identities are keyed on each provider's stable subject, never on email. See
//! `identities` in the db crate for the link rules.

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use dioxus::server::axum::extract::Query;
use dioxus::server::axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use dioxus::server::axum::response::{IntoResponse, Response};
use recomphub_db::identities::{self, IdentityProfile, Provider};
use recomphub_db::users_sessions::{self, UserRow};
use recomphub_db::{DbError, media};
use serde::Deserialize;

use super::{
    config::{self, Config},
    google, session, state,
};

const GITHUB_AUTHORIZE_URL: &str = "https://github.com/login/oauth/authorize";
const GITHUB_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const GITHUB_USER_URL: &str = "https://api.github.com/user";

/// Holds the provider, state, nonce, PKCE verifier, link flag and `next` while the
/// user is away at the provider. See [`Pending`].
const OAUTH_COOKIE: &str = "rh_oauth";
const OAUTH_TTL_SECS: i64 = 600;
const USER_AGENT: &str = "recomphub/1.0 (+https://recomphub.com)";

#[derive(Deserialize)]
pub struct LoginQuery {
    next: Option<String>,
    /// `1` connects this provider to the signed-in account instead of signing in.
    link: Option<String>,
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

/// What the callback needs from the login step. It travels in the `rh_oauth` cookie as
/// `provider|state|nonce|verifier|link|next`. Nonce and verifier are empty for GitHub.
struct Pending {
    provider: Provider,
    state: String,
    nonce: String,
    verifier: String,
    link: bool,
    next: String,
}

impl Pending {
    fn encode(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}",
            self.provider.as_str(),
            self.state,
            self.nonce,
            self.verifier,
            if self.link { "1" } else { "0" },
            self.next
        )
    }

    fn decode(raw: &str) -> Option<Self> {
        let parts: Vec<&str> = raw.splitn(6, '|').collect();
        let [provider, state, nonce, verifier, link, next] = parts.as_slice() else {
            return None;
        };
        Some(Self {
            provider: Provider::parse(provider)?,
            state: state.to_string(),
            nonce: nonce.to_string(),
            verifier: verifier.to_string(),
            link: *link == "1",
            next: next.to_string(),
        })
    }
}

/// `GET /auth/github/login?next=&link=`: starts GitHub sign-in.
pub async fn github_login(headers: HeaderMap, Query(query): Query<LoginQuery>) -> Response {
    start(Provider::GitHub, &headers, query).await
}

/// `GET /auth/google/login?next=&link=`: starts Google sign-in.
pub async fn google_login(headers: HeaderMap, Query(query): Query<LoginQuery>) -> Response {
    start(Provider::Google, &headers, query).await
}

/// `GET /auth/github/callback`: verifies state, exchanges the code, and signs in or links.
pub async fn github_callback(headers: HeaderMap, Query(query): Query<CallbackQuery>) -> Response {
    callback(Provider::GitHub, &headers, query).await
}

/// `GET /auth/google/callback`: verifies state and nonce, exchanges the code, and signs in or links.
pub async fn google_callback(headers: HeaderMap, Query(query): Query<CallbackQuery>) -> Response {
    callback(Provider::Google, &headers, query).await
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

async fn start(provider: Provider, headers: &HeaderMap, query: LoginQuery) -> Response {
    let cfg = config::get();
    let (client_id, _) = match credentials(cfg, provider) {
        Ok(pair) => pair,
        Err(message) => return plain(StatusCode::SERVICE_UNAVAILABLE, message),
    };
    let link = query.link.as_deref() == Some("1");
    if link && let Err(response) = signed_in_user(headers).await {
        return response;
    }
    let next = safe_next(query.next.as_deref().or(link.then_some("/account")));

    let state = session::new_token();
    let nonce = session::new_token();
    let verifier = session::new_token();
    let redirect_uri = callback_url(cfg, provider);
    let authorize = match provider {
        Provider::GitHub => github_authorize_url(client_id, &redirect_uri, &state),
        Provider::Google => google::authorize_url(
            client_id,
            &redirect_uri,
            &state,
            &nonce,
            &google::code_challenge(&verifier),
            link,
        ),
    };
    let url = match authorize {
        Ok(url) => url,
        Err(err) => {
            tracing::error!(error = %err, provider = provider.as_str(), "building the authorize URL failed");
            return plain(StatusCode::INTERNAL_SERVER_ERROR, "sign-in failed");
        }
    };

    let pending = Pending {
        provider,
        state,
        nonce,
        verifier,
        link,
        next,
    };
    let cookie = format!(
        "{OAUTH_COOKIE}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age={OAUTH_TTL_SECS}{}",
        pending.encode(),
        secure_attr(cfg)
    );
    redirect(StatusCode::FOUND, &url, &[cookie])
}

async fn callback(provider: Provider, headers: &HeaderMap, query: CallbackQuery) -> Response {
    let cfg = config::get();
    let label = provider.label();
    let Some(pending) = cookie_value(headers, OAUTH_COOKIE)
        .and_then(Pending::decode)
        .filter(|pending| pending.provider == provider)
    else {
        return plain(StatusCode::BAD_REQUEST, "sign-in expired; start again");
    };
    let state_matches =
        !pending.state.is_empty() && query.state.as_deref() == Some(pending.state.as_str());
    if !state_matches {
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
    let (client_id, client_secret) = match credentials(cfg, provider) {
        Ok(pair) => pair,
        Err(message) => return plain(StatusCode::SERVICE_UNAVAILABLE, message),
    };
    // Linking needs a live session. Check it before spending the code. `identities::link`
    // re-checks the member under a row lock and refuses banned accounts.
    let member = if pending.link {
        match signed_in_user(headers).await {
            Ok(user) => Some(user),
            Err(response) => return response,
        }
    } else {
        None
    };
    let next = safe_next(Some(pending.next.as_str()));

    let redirect_uri = callback_url(cfg, provider);
    let (mut profile, avatar_url) = match fetch_account(
        provider,
        client_id,
        client_secret,
        &redirect_uri,
        code,
        &pending,
    )
    .await
    {
        Ok(account) => account,
        Err(err) => {
            tracing::error!(error = %err, provider = provider.as_str(), "OAuth exchange failed");
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
    profile.avatar_media_id = match avatar_url.as_deref() {
        Some(url) => match media::enqueue(pool, "avatar", url).await {
            Ok(id) => Some(id),
            Err(err) => {
                tracing::warn!(error = %err, "could not queue the avatar; continuing without it");
                None
            }
        },
        None => None,
    };

    let secure = secure_attr(cfg);
    let clear_oauth = format!("{OAUTH_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{secure}");

    if let Some(member) = member {
        return match identities::link(pool, member.id, &profile).await {
            Ok(()) => redirect(StatusCode::FOUND, &next, &[clear_oauth]),
            Err(err) => identity_error(err),
        };
    }

    let is_admin = is_admin(&profile, &cfg.admin_logins, &cfg.admin_google_emails);
    let user = match identities::sign_in(pool, &profile, is_admin).await {
        Ok(user) if user.banned => return plain(StatusCode::FORBIDDEN, "this account is banned"),
        Ok(user) => user,
        Err(err) => return identity_error(err),
    };

    let token = session::new_token();
    let expires_at = Utc::now() + chrono::Duration::days(session::SESSION_DAYS);
    if let Err(err) = users_sessions::create_session(pool, &token, user.id, expires_at).await {
        tracing::error!(error = ?err, "creating the session failed");
        return plain(StatusCode::INTERNAL_SERVER_ERROR, "sign-in failed");
    }

    let max_age = session::SESSION_DAYS * 24 * 60 * 60;
    let session_cookie = format!(
        "{}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}",
        session::SESSION_COOKIE
    );
    redirect(StatusCode::FOUND, &next, &[session_cookie, clear_oauth])
}

/// Runs the provider's code exchange and returns the account to store, plus the avatar
/// URL to queue. GitHub avatars get a size hint, which Google's `picture` URLs must not.
async fn fetch_account(
    provider: Provider,
    client_id: &str,
    client_secret: &str,
    redirect_uri: &str,
    code: &str,
    pending: &Pending,
) -> Result<(IdentityProfile, Option<String>)> {
    match provider {
        Provider::GitHub => {
            let user = fetch_github_user(client_id, client_secret, redirect_uri, code).await?;
            let avatar = user.avatar_url.as_deref().map(sized_avatar_url);
            let profile = IdentityProfile {
                provider,
                subject: user.id.to_string(),
                login: Some(user.login),
                email: None,
                name: user.name,
                avatar_media_id: None,
            };
            Ok((profile, avatar))
        }
        Provider::Google => {
            let account = google::fetch_account(
                client_id,
                client_secret,
                redirect_uri,
                code,
                &pending.verifier,
                &pending.nonce,
            )
            .await?;
            let profile = IdentityProfile {
                provider,
                subject: account.subject,
                login: None,
                email: account.email,
                name: account.name,
                avatar_media_id: None,
            };
            Ok((profile, account.picture))
        }
    }
}

/// Whether this account gets the admin role on sign-in. GitHub matches the login;
/// Google matches the email, and `profile.email` holds only verified emails.
fn is_admin(profile: &IdentityProfile, admin_logins: &[String], admin_emails: &[String]) -> bool {
    match profile.provider {
        Provider::GitHub => profile
            .login
            .as_deref()
            .is_some_and(|login| admin_logins.contains(&login.to_ascii_lowercase())),
        Provider::Google => profile
            .email
            .as_deref()
            .is_some_and(|email| admin_emails.contains(&email.to_ascii_lowercase())),
    }
}

/// The signed-in, unbanned member behind these headers, or the response to send instead.
async fn signed_in_user(headers: &HeaderMap) -> std::result::Result<UserRow, Response> {
    let sign_in_first = || {
        plain(
            StatusCode::UNAUTHORIZED,
            "sign in before connecting another account",
        )
    };
    let Some(token) = session::token_from_headers(headers) else {
        return Err(sign_in_first());
    };
    let pool = state::pool().map_err(|err| {
        tracing::error!(error = %err, "database pool unavailable during sign-in");
        plain(StatusCode::INTERNAL_SERVER_ERROR, "sign-in failed")
    })?;
    let user = users_sessions::find_session_user(pool, &token)
        .await
        .map_err(|err| {
            tracing::error!(error = ?err, "loading the session failed");
            plain(StatusCode::INTERNAL_SERVER_ERROR, "sign-in failed")
        })?
        .ok_or_else(sign_in_first)?;
    if user.banned {
        return Err(plain(StatusCode::FORBIDDEN, "this account is banned"));
    }
    Ok(user)
}

/// Maps an identity failure to a response. Conflict messages are written for members.
fn identity_error(err: DbError) -> Response {
    match err {
        DbError::Conflict(message) => plain(StatusCode::CONFLICT, message),
        DbError::Banned => plain(StatusCode::FORBIDDEN, "this account is banned"),
        other => {
            tracing::error!(error = ?other, "saving the sign-in failed");
            plain(StatusCode::INTERNAL_SERVER_ERROR, "sign-in failed")
        }
    }
}

/// Exchanges the GitHub authorization code for a token and reads the GitHub profile.
/// The access token is used once and discarded.
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

fn github_authorize_url(client_id: &str, redirect_uri: &str, state: &str) -> Result<String> {
    Ok(reqwest::Url::parse_with_params(
        GITHUB_AUTHORIZE_URL,
        &[
            ("client_id", client_id),
            ("redirect_uri", redirect_uri),
            ("state", state),
        ],
    )?
    .to_string())
}

/// The client id and secret for a provider, or a 503 message naming the first missing variable.
fn credentials(cfg: &Config, provider: Provider) -> std::result::Result<(&str, &str), String> {
    let (id, secret, id_var, secret_var) = match provider {
        Provider::GitHub => (
            cfg.github_client_id.as_deref(),
            cfg.github_client_secret.as_deref(),
            "GITHUB_OAUTH_CLIENT_ID",
            "GITHUB_OAUTH_CLIENT_SECRET",
        ),
        Provider::Google => (
            cfg.google_client_id.as_deref(),
            cfg.google_client_secret.as_deref(),
            "GOOGLE_OAUTH_CLIENT_ID",
            "GOOGLE_OAUTH_CLIENT_SECRET",
        ),
    };
    match (id, secret) {
        (Some(id), Some(secret)) => Ok((id, secret)),
        (None, _) => Err(format!(
            "{} sign-in is not configured: {id_var} is not set",
            provider.label()
        )),
        (Some(_), None) => Err(format!(
            "{} sign-in is not configured: {secret_var} is not set",
            provider.label()
        )),
    }
}

fn callback_url(cfg: &Config, provider: Provider) -> String {
    format!("{}/auth/{}/callback", cfg.site_url, provider.as_str())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(provider: Provider, login: Option<&str>, email: Option<&str>) -> IdentityProfile {
        IdentityProfile {
            provider,
            subject: "subject".to_string(),
            login: login.map(str::to_string),
            email: email.map(str::to_string),
            name: None,
            avatar_media_id: None,
        }
    }

    #[test]
    fn safe_next_keeps_same_origin_paths() {
        assert_eq!(safe_next(Some("/")), "/");
        assert_eq!(safe_next(Some("/account?x=1")), "/account?x=1");
    }

    #[test]
    fn safe_next_rejects_other_origins_and_unsafe_bytes() {
        for raw in ["//evil", "/\\evil", "https://x", "/a b", "", "/a;b", "/a|b"] {
            assert_eq!(safe_next(Some(raw)), "/", "{raw:?} should fall back to /");
        }
        assert_eq!(safe_next(None), "/");
    }

    #[test]
    fn pending_round_trips_through_the_cookie() {
        let pending = Pending {
            provider: Provider::Google,
            state: "s".to_string(),
            nonce: "n".to_string(),
            verifier: "v".to_string(),
            link: true,
            next: "/account".to_string(),
        };
        let decoded = Pending::decode(&pending.encode()).expect("decodes");
        assert_eq!(decoded.provider, Provider::Google);
        assert_eq!(decoded.state, "s");
        assert_eq!(decoded.nonce, "n");
        assert_eq!(decoded.verifier, "v");
        assert!(decoded.link);
        assert_eq!(decoded.next, "/account");
    }

    #[test]
    fn pending_rejects_a_short_or_unknown_cookie() {
        assert!(Pending::decode("github|s|n").is_none());
        assert!(Pending::decode("twitter|s|n|v|0|/").is_none());
    }

    #[test]
    fn github_admin_matches_the_login_case_insensitively() {
        let admins = vec!["mike".to_string()];
        let p = profile(Provider::GitHub, Some("Mike"), None);
        assert!(is_admin(&p, &admins, &[]));
        let other = profile(Provider::GitHub, Some("someone"), None);
        assert!(!is_admin(&other, &admins, &[]));
    }

    #[test]
    fn google_admin_needs_a_verified_email_on_the_list() {
        let emails = vec!["mike@example.com".to_string()];
        let verified = profile(Provider::Google, None, Some("Mike@Example.com"));
        assert!(is_admin(&verified, &[], &emails));
        // Unverified emails never reach `profile.email`, so the match cannot happen.
        let unverified = profile(Provider::Google, None, None);
        assert!(!is_admin(&unverified, &[], &emails));
    }
}
