//! Cookie sessions and the auth guards used by server functions.
//!
//! The browser holds an opaque random token in an HttpOnly cookie. The database
//! stores only its SHA-256 (see `recomphub_db::users_sessions`).

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use dioxus::fullstack::{FullstackContext, HeaderMap, StatusCode, http::header::COOKIE};
use dioxus::prelude::ServerFnError;
use rand::Rng;
use recomphub_db::DbError;
use recomphub_db::users_sessions::{self, UserRow};
use recomphub_shared::{MediaRef, SessionUser};

use super::state;

pub const SESSION_COOKIE: &str = "rh_session";
pub const SESSION_DAYS: i64 = 30;

/// A fresh 256-bit token, base64url without padding.
pub fn new_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes[..]);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The session token from a `Cookie` header, if present.
pub fn token_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == SESSION_COOKIE)
        .map(|(_, value)| value.to_string())
        .filter(|value| !value.is_empty())
}

/// The active user for the current request, or `None` when signed out.
pub async fn current_user_row() -> Result<Option<UserRow>, ServerFnError> {
    let headers: HeaderMap = FullstackContext::extract::<HeaderMap, _>().await?;
    let Some(token) = token_from_headers(&headers) else {
        return Ok(None);
    };
    users_sessions::find_by_session(state::pool()?, &token)
        .await
        .map_err(db_error)
}

/// The active user, 401 when signed out, or 403 when the account is banned.
pub async fn require_user() -> Result<UserRow, ServerFnError> {
    let headers: HeaderMap = FullstackContext::extract::<HeaderMap, _>().await?;
    let token = token_from_headers(&headers)
        .ok_or_else(|| http_error(StatusCode::UNAUTHORIZED, "sign in required"))?;
    let row = users_sessions::find_session_user(state::pool()?, &token)
        .await
        .map_err(db_error)?
        .ok_or_else(|| http_error(StatusCode::UNAUTHORIZED, "sign in required"))?;
    if row.banned {
        return Err(http_error(StatusCode::FORBIDDEN, "account is banned"));
    }
    Ok(row)
}

/// An active moderator or admin, or 401/403.
pub async fn require_moderator() -> Result<UserRow, ServerFnError> {
    let user = require_user().await?;
    if user.role().map_err(db_error)?.is_moderator() {
        Ok(user)
    } else {
        Err(http_error(StatusCode::FORBIDDEN, "moderator role required"))
    }
}

/// An active admin, or 401/403.
pub async fn require_admin() -> Result<UserRow, ServerFnError> {
    let user = require_user().await?;
    if user.role().map_err(db_error)?.is_admin() {
        Ok(user)
    } else {
        Err(http_error(StatusCode::FORBIDDEN, "admin role required"))
    }
}

/// The browser-facing shape of a user. `avatar` is resolved by the caller.
pub fn session_user(row: &UserRow, avatar: Option<MediaRef>) -> SessionUser {
    SessionUser {
        id: row.id,
        display_name: row.display_name.clone(),
        github_login: row.github_login.clone(),
        role: row.role.clone(),
        avatar,
    }
}

/// Maps a database error to a server-function error with an HTTP status.
pub fn db_error(err: DbError) -> ServerFnError {
    match err {
        DbError::NotFound(what) => http_error(StatusCode::NOT_FOUND, what),
        DbError::Banned => http_error(StatusCode::FORBIDDEN, "account is banned"),
        DbError::RateLimited(seconds) => http_error(
            StatusCode::TOO_MANY_REQUESTS,
            format!("daily submission limit reached; retry in {seconds} seconds"),
        ),
        DbError::InvalidPayload(msg) | DbError::InvalidState(msg) => {
            http_error(StatusCode::BAD_REQUEST, msg)
        }
        DbError::Conflict(msg) => http_error(StatusCode::CONFLICT, msg),
        other => {
            tracing::error!(error = %other, "database error");
            http_error(StatusCode::INTERNAL_SERVER_ERROR, "internal server error")
        }
    }
}

/// A server-function error carrying an HTTP status code.
pub fn http_error(status: StatusCode, message: impl Into<String>) -> ServerFnError {
    ServerFnError::ServerError {
        message: message.into(),
        code: status.as_u16(),
        details: None,
    }
}
