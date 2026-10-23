//! Browser-approved desktop authorization endpoints.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use dioxus::server::axum::extract::{Form, Json, Query};
use dioxus::server::axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use dioxus::server::axum::response::{IntoResponse, Response};
use dioxus::server::axum::routing::{get, post};
use dioxus::server::axum::{Router, extract::DefaultBodyLimit};
use recomphub_db::desktop_auth::{self, DesktopAuthError, ExchangeResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{config, session, state};

const AUTH_PATH: &str = "/auth/desktop";
const AUTH_INTERVAL_SECONDS: u64 = 3;
const AUTH_EXPIRES_SECONDS: u64 = 600;
const AUTH_CSP: &str =
    "default-src 'none'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'";

#[derive(Deserialize)]
struct StartBody {
    challenge: String,
}

#[derive(Serialize)]
struct StartReply {
    request_id: String,
    authorization_url: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Deserialize)]
struct DesktopQuery {
    request_id: String,
}

#[derive(Deserialize)]
struct ApprovalForm {
    request_id: String,
    csrf_token: String,
}

#[derive(Deserialize)]
struct ExchangeBody {
    request_id: String,
    verifier: String,
}

#[derive(Serialize)]
struct PendingReply {
    status: &'static str,
}

#[derive(Serialize)]
struct CompleteReply {
    session_token: String,
    expires_at: String,
}

/// The browser flow is deliberately kept separate from server functions so request bodies are
/// bounded and token responses are never serialised into the Dioxus server-function channel.
pub fn routes() -> Router {
    Router::new()
        .route("/api/v1/desktop/auth/start", post(start))
        .route(AUTH_PATH, get(authorize))
        .route("/auth/desktop/approve", post(approve))
        .route("/api/v1/desktop/auth/exchange", post(exchange))
        .route("/api/v1/desktop/auth/logout", post(logout))
        .layer(DefaultBodyLimit::max(4 * 1024))
}

async fn start(Json(body): Json<StartBody>) -> Response {
    let challenge = match decode_32(&body.challenge) {
        Some(value) => value,
        None => {
            return plain(
                StatusCode::BAD_REQUEST,
                "invalid desktop authorization request",
            );
        }
    };
    let pool = match state::pool() {
        Ok(pool) => pool,
        Err(_) => return database_unavailable(),
    };
    let request_id = session::new_token();
    if let Err(error) = desktop_auth::start_request(pool, &request_id, &challenge).await {
        return auth_error(error);
    }
    Json(StartReply {
        authorization_url: format!(
            "{}{}?request_id={}",
            config::get().site_url,
            AUTH_PATH,
            request_id
        ),
        request_id,
        expires_in: AUTH_EXPIRES_SECONDS,
        interval: AUTH_INTERVAL_SECONDS,
    })
    .into_response()
}

async fn authorize(headers: HeaderMap, Query(query): Query<DesktopQuery>) -> Response {
    if decode_32(&query.request_id).is_none() {
        return plain(
            StatusCode::BAD_REQUEST,
            "invalid desktop authorization request",
        );
    }
    let pool = match state::pool() {
        Ok(pool) => pool,
        Err(_) => return database_unavailable(),
    };
    if let Err(error) = desktop_auth::validate_request(pool, &query.request_id).await {
        return auth_error(error);
    }
    let Some(browser_token) = session::token_from_headers(&headers) else {
        return redirect_to_google(&query.request_id);
    };
    let csrf_token = session::new_token();
    let csrf_hash = hash32(&csrf_token);
    let user =
        match desktop_auth::bind_confirmation(pool, &query.request_id, &browser_token, &csrf_hash)
            .await
        {
            Ok(user) => user,
            Err(DesktopAuthError::Invalid) => return redirect_to_google(&query.request_id),
            Err(error) => return auth_error(error),
        };
    let body = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>Connect RecompHub desktop</title><h1>Connect RecompHub desktop</h1><p>Signed in as {}</p><form method=\"post\" action=\"/auth/desktop/approve\"><input type=\"hidden\" name=\"request_id\" value=\"{}\"><input type=\"hidden\" name=\"csrf_token\" value=\"{}\"><button type=\"submit\">Connect RecompHub desktop</button></form>",
        html_escape(&user.display_name),
        query.request_id,
        csrf_token,
    );
    // A no-referrer document gives a non-CORS form POST an opaque Origin in browsers.
    // Preserve exact Origin validation without scripts or cross-origin referrer disclosure.
    let mut response = html(body);
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    response
}

async fn approve(headers: HeaderMap, Form(form): Form<ApprovalForm>) -> Response {
    if headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        != Some(config::get().site_url.as_str())
    {
        return plain(
            StatusCode::FORBIDDEN,
            "desktop authorization origin mismatch",
        );
    }
    let Some(browser_token) = session::token_from_headers(&headers) else {
        return plain(StatusCode::UNAUTHORIZED, "sign in required");
    };
    let Some(csrf_hash) = decode_32(&form.csrf_token).map(|_| hash32(&form.csrf_token)) else {
        return plain(
            StatusCode::BAD_REQUEST,
            "invalid desktop authorization request",
        );
    };
    let pool = match state::pool() {
        Ok(pool) => pool,
        Err(_) => return database_unavailable(),
    };
    match desktop_auth::approve_request(pool, &form.request_id, &browser_token, &csrf_hash).await {
        Ok(()) => html("<!doctype html><meta charset=\"utf-8\"><title>Desktop connected</title><h1>RecompHub desktop connected</h1><p>Return to the desktop app.</p>".to_string()),
        Err(error) => auth_error(error),
    }
}

async fn exchange(Json(body): Json<ExchangeBody>) -> Response {
    let Some(verifier) = decode_32(&body.verifier) else {
        return plain(
            StatusCode::BAD_REQUEST,
            "invalid desktop authorization request",
        );
    };
    let pool = match state::pool() {
        Ok(pool) => pool,
        Err(_) => return database_unavailable(),
    };
    let verifier_hash: [u8; 32] = Sha256::digest(verifier).into();
    let session_token = session::new_token();
    match desktop_auth::exchange_request(pool, &body.request_id, &verifier_hash, &session_token)
        .await
    {
        Ok(ExchangeResult::Pending) => (
            StatusCode::ACCEPTED,
            Json(PendingReply { status: "pending" }),
        )
            .into_response(),
        Ok(ExchangeResult::Complete { expires_at }) => Json(CompleteReply {
            session_token,
            expires_at: expires_at.to_rfc3339(),
        })
        .into_response(),
        Err(error) => auth_error(error),
    }
}

async fn logout(headers: HeaderMap) -> Response {
    let Some(token) = session::token_from_headers(&headers) else {
        return plain(StatusCode::UNAUTHORIZED, "sign in required");
    };
    let pool = match state::pool() {
        Ok(pool) => pool,
        Err(_) => return database_unavailable(),
    };
    let mut transaction = match pool.begin().await {
        Ok(transaction) => transaction,
        Err(_) => return plain(StatusCode::INTERNAL_SERVER_ERROR, "desktop logout failed"),
    };
    let Some(user) =
        (match recomphub_db::users_sessions::find_session_user(&mut *transaction, &token).await {
            Ok(user) => user,
            Err(_) => return plain(StatusCode::INTERNAL_SERVER_ERROR, "desktop logout failed"),
        })
    else {
        return plain(StatusCode::UNAUTHORIZED, "sign in required");
    };
    if let Err(error) = recomphub_db::users_sessions::lock_active(&mut *transaction, user.id).await
    {
        return match error {
            recomphub_db::DbError::Banned => plain(
                StatusCode::FORBIDDEN,
                "desktop authorization request forbidden",
            ),
            recomphub_db::DbError::NotFound(_) => {
                plain(StatusCode::UNAUTHORIZED, "sign in required")
            }
            _ => plain(StatusCode::INTERNAL_SERVER_ERROR, "desktop logout failed"),
        };
    }
    if recomphub_db::users_sessions::delete_session(&mut *transaction, &token)
        .await
        .is_err()
        || transaction.commit().await.is_err()
    {
        return plain(StatusCode::INTERNAL_SERVER_ERROR, "desktop logout failed");
    }
    StatusCode::NO_CONTENT.into_response()
}

fn redirect_to_google(request_id: &str) -> Response {
    let next = format!("{AUTH_PATH}?request_id={request_id}");
    let location = format!("/auth/google?next={}", url_encode(&next));
    (StatusCode::FOUND, [(header::LOCATION, location)]).into_response()
}

fn database_unavailable() -> Response {
    plain(
        StatusCode::INTERNAL_SERVER_ERROR,
        "desktop authorization unavailable",
    )
}

fn auth_error(error: DesktopAuthError) -> Response {
    match error {
        DesktopAuthError::Invalid => plain(
            StatusCode::BAD_REQUEST,
            "invalid desktop authorization request",
        ),
        DesktopAuthError::Gone => plain(StatusCode::GONE, "desktop authorization request expired"),
        DesktopAuthError::Forbidden => plain(
            StatusCode::FORBIDDEN,
            "desktop authorization request forbidden",
        ),
        DesktopAuthError::Conflict => plain(
            StatusCode::CONFLICT,
            "desktop authorization request already approved",
        ),
        DesktopAuthError::Capacity => {
            let mut response = plain(
                StatusCode::SERVICE_UNAVAILABLE,
                "desktop authorization is busy",
            );
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("60"));
            response
        }
        DesktopAuthError::Database(error) => {
            tracing::error!(error = %error, "desktop authorization database failure");
            plain(
                StatusCode::INTERNAL_SERVER_ERROR,
                "desktop authorization unavailable",
            )
        }
    }
}

fn plain(status: StatusCode, message: &'static str) -> Response {
    (status, message).into_response()
}

fn html(body: String) -> Response {
    let mut response = (StatusCode::OK, body).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(AUTH_CSP),
    );
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

fn decode_32(value: &str) -> Option<[u8; 32]> {
    let decoded = URL_SAFE_NO_PAD.decode(value).ok()?;
    decoded.try_into().ok()
}

fn hash32(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

fn url_encode(value: &str) -> String {
    reqwest::Url::parse("https://recomphub.invalid/")
        .ok()
        .map(|mut url| {
            url.query_pairs_mut().append_pair("next", value);
            url.query()
                .unwrap_or_default()
                .strip_prefix("next=")
                .unwrap_or_default()
                .to_string()
        })
        .unwrap_or_default()
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\"', "&quot;")
        .replace('\'', "&#x27;")
}
