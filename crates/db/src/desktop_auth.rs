//! One-time, browser-approved desktop login grants.

use chrono::{DateTime, Duration, Utc};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use subtle::ConstantTimeEq;

use crate::{DbError, users_sessions};

const REQUEST_TTL: Duration = Duration::minutes(10);
const REQUEST_CAPACITY: i64 = 1_000;
const ADMISSION_LOCK: i64 = 0x0052_4844_4155_5448;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExchangeResult {
    Pending,
    Complete { expires_at: DateTime<Utc> },
}

#[derive(Debug, thiserror::Error)]
pub enum DesktopAuthError {
    #[error("desktop authorization request is invalid")]
    Invalid,
    #[error("desktop authorization request is gone")]
    Gone,
    #[error("desktop authorization request is forbidden")]
    Forbidden,
    #[error("desktop authorization request conflicts with its current state")]
    Conflict,
    #[error("desktop authorization capacity is exhausted")]
    Capacity,
    #[error(transparent)]
    Database(#[from] DbError),
}

impl From<sqlx::Error> for DesktopAuthError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(DbError::Sql(error))
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct LoginRequest {
    challenge_hash: Vec<u8>,
    user_id: Option<i64>,
    csrf_hash: Option<Vec<u8>>,
    browser_session_hash: Option<Vec<u8>>,
    expires_at: DateTime<Utc>,
    consumed_at: Option<DateTime<Utc>>,
}

fn hash(value: &str) -> Vec<u8> {
    Sha256::digest(value.as_bytes()).to_vec()
}

fn matches(left: &[u8], right: &[u8]) -> bool {
    left.ct_eq(right).into()
}

fn request_is_gone(request: &LoginRequest) -> bool {
    request.expires_at <= Utc::now() || request.consumed_at.is_some()
}

async fn lock_request(
    transaction: &mut Transaction<'_, Postgres>,
    request_id: &str,
) -> Result<LoginRequest, DesktopAuthError> {
    let request = sqlx::query_as::<_, LoginRequest>(
        "SELECT challenge_hash, user_id, csrf_hash, browser_session_hash, expires_at, consumed_at
         FROM desktop_login_requests WHERE request_hash = $1 FOR UPDATE",
    )
    .bind(hash(request_id))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(DesktopAuthError::Invalid)?;
    if request_is_gone(&request) {
        return Err(DesktopAuthError::Gone);
    }
    Ok(request)
}

async fn lock_browser_user(
    transaction: &mut Transaction<'_, Postgres>,
    browser_token: &str,
) -> Result<users_sessions::UserRow, DesktopAuthError> {
    let user = users_sessions::find_session_user(&mut **transaction, browser_token)
        .await?
        .ok_or(DesktopAuthError::Invalid)?;
    users_sessions::lock_active(&mut **transaction, user.id)
        .await
        .map_err(|error| match error {
            DbError::Banned => DesktopAuthError::Forbidden,
            other => DesktopAuthError::Database(other),
        })
}

/// Confirms that a browser-facing authorization URL still names a live grant.
/// This check deliberately runs before deciding whether to send a visitor to Google.
pub async fn validate_request(pool: &PgPool, request_id: &str) -> Result<(), DesktopAuthError> {
    let mut transaction = pool.begin().await?;
    lock_request(&mut transaction, request_id).await?;
    transaction.commit().await?;
    Ok(())
}

/// Creates a short-lived request after serialised expiry cleanup and capacity admission.
pub async fn start_request(
    pool: &PgPool,
    request_id: &str,
    challenge: &[u8; 32],
) -> Result<DateTime<Utc>, DesktopAuthError> {
    let mut transaction = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(ADMISSION_LOCK)
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        "DELETE FROM desktop_login_requests WHERE created_at <= now() - interval '10 minutes'",
    )
    .execute(&mut *transaction)
    .await?;
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM desktop_login_requests WHERE created_at > now() - interval '10 minutes'",
    )
    .fetch_one(&mut *transaction)
    .await?;
    if count >= REQUEST_CAPACITY {
        return Err(DesktopAuthError::Capacity);
    }
    let expires_at = Utc::now() + REQUEST_TTL;
    sqlx::query(
        "INSERT INTO desktop_login_requests(request_hash, challenge_hash, expires_at)
         VALUES ($1, $2, $3)",
    )
    .bind(hash(request_id))
    .bind(challenge.as_slice())
    .bind(expires_at)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(expires_at)
}

/// Binds one active browser session and fresh CSRF token to the request before rendering approval.
pub async fn bind_confirmation(
    pool: &PgPool,
    request_id: &str,
    browser_token: &str,
    csrf_hash: &[u8; 32],
) -> Result<users_sessions::UserRow, DesktopAuthError> {
    let mut transaction = pool.begin().await?;
    let request = lock_request(&mut transaction, request_id).await?;
    if request.user_id.is_some() {
        return Err(DesktopAuthError::Conflict);
    }
    let user = lock_browser_user(&mut transaction, browser_token).await?;
    let session_hash = users_sessions::session_hash(browser_token);
    if request
        .browser_session_hash
        .as_deref()
        .is_some_and(|stored| !matches(stored, &session_hash))
    {
        return Err(DesktopAuthError::Forbidden);
    }
    sqlx::query(
        "UPDATE desktop_login_requests SET browser_session_hash = $2, csrf_hash = $3
         WHERE request_hash = $1",
    )
    .bind(hash(request_id))
    .bind(session_hash)
    .bind(csrf_hash.as_slice())
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(user)
}

/// Explicitly approves a request bound to this exact active browser session and CSRF token.
pub async fn approve_request(
    pool: &PgPool,
    request_id: &str,
    browser_token: &str,
    csrf_hash: &[u8; 32],
) -> Result<(), DesktopAuthError> {
    let mut transaction = pool.begin().await?;
    let request = lock_request(&mut transaction, request_id).await?;
    if request.user_id.is_some() {
        return Err(DesktopAuthError::Conflict);
    }
    let session = request
        .browser_session_hash
        .as_deref()
        .ok_or(DesktopAuthError::Forbidden)?;
    let csrf = request
        .csrf_hash
        .as_deref()
        .ok_or(DesktopAuthError::Forbidden)?;
    let session_hash = users_sessions::session_hash(browser_token);
    if !matches(session, &session_hash) || !matches(csrf, csrf_hash) {
        return Err(DesktopAuthError::Forbidden);
    }
    let user = lock_browser_user(&mut transaction, browser_token).await?;
    let result = sqlx::query(
        "UPDATE desktop_login_requests SET user_id = $2 WHERE request_hash = $1 AND user_id IS NULL",
    )
    .bind(hash(request_id))
    .bind(user.id)
    .execute(&mut *transaction)
    .await?;
    if result.rows_affected() != 1 {
        return Err(DesktopAuthError::Conflict);
    }
    transaction.commit().await?;
    Ok(())
}

/// Exchanges an approved verifier exactly once for an independent native session.
pub async fn exchange_request(
    pool: &PgPool,
    request_id: &str,
    verifier_hash: &[u8; 32],
    new_session_token: &str,
) -> Result<ExchangeResult, DesktopAuthError> {
    let mut transaction = pool.begin().await?;
    let request = lock_request(&mut transaction, request_id).await?;
    if !matches(&request.challenge_hash, verifier_hash) {
        return Err(DesktopAuthError::Invalid);
    }
    let Some(user_id) = request.user_id else {
        return Ok(ExchangeResult::Pending);
    };
    users_sessions::lock_active(&mut *transaction, user_id)
        .await
        .map_err(|error| match error {
            DbError::Banned => DesktopAuthError::Forbidden,
            DbError::NotFound(_) => DesktopAuthError::Gone,
            other => DesktopAuthError::Database(other),
        })?;
    let expires_at = Utc::now() + Duration::days(30);
    users_sessions::create_session(&mut *transaction, new_session_token, user_id, expires_at)
        .await?;
    let result = sqlx::query(
        "UPDATE desktop_login_requests SET consumed_at = now()
         WHERE request_hash = $1 AND consumed_at IS NULL",
    )
    .bind(hash(request_id))
    .execute(&mut *transaction)
    .await?;
    if result.rows_affected() != 1 {
        return Err(DesktopAuthError::Gone);
    }
    transaction.commit().await?;
    Ok(ExchangeResult::Complete { expires_at })
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};

    use super::*;

    async fn browser_session(pool: &PgPool) -> (i64, String) {
        let user_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO users (display_name, role) VALUES ('Desktop auth test', 'user') RETURNING id",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        let token = "browser-session".to_string();
        users_sessions::create_session(pool, &token, user_id, Utc::now() + Duration::days(1))
            .await
            .unwrap();
        (user_id, token)
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn approval_exchanges_once_and_binds_browser_session(pool: PgPool) {
        let (user_id, browser_token) = browser_session(&pool).await;
        let request_id = "request-id";
        let verifier = [7; 32];
        start_request(&pool, request_id, &verifier).await.unwrap();
        let csrf = [8; 32];
        assert_eq!(
            bind_confirmation(&pool, request_id, &browser_token, &csrf)
                .await
                .unwrap()
                .id,
            user_id
        );
        assert!(matches!(
            approve_request(&pool, request_id, "other-browser-session", &csrf).await,
            Err(DesktopAuthError::Forbidden)
        ));
        approve_request(&pool, request_id, &browser_token, &csrf)
            .await
            .unwrap();
        assert!(matches!(
            exchange_request(&pool, request_id, &[9; 32], "native-session").await,
            Err(DesktopAuthError::Invalid)
        ));
        assert!(matches!(
            exchange_request(&pool, request_id, &verifier, "native-session").await,
            Ok(ExchangeResult::Complete { .. })
        ));
        assert!(matches!(
            exchange_request(&pool, request_id, &verifier, "replay").await,
            Err(DesktopAuthError::Gone)
        ));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn concurrent_admission_never_exceeds_one_thousand_grants(pool: PgPool) {
        let mut requests = Vec::with_capacity(1_001);
        for index in 0..1_001 {
            let pool = pool.clone();
            requests.push(tokio::spawn(async move {
                start_request(&pool, &format!("capacity-fixture-{index}"), &[1; 32]).await
            }));
        }
        let mut admitted = 0;
        let mut refused = 0;
        for request in requests {
            match request.await.unwrap() {
                Ok(_) => admitted += 1,
                Err(DesktopAuthError::Capacity) => refused += 1,
                Err(error) => panic!("unexpected admission failure: {error}"),
            }
        }
        assert_eq!(admitted, 1_000);
        assert_eq!(refused, 1);
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM desktop_login_requests")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1_000);
    }
}
