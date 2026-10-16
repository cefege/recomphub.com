//! Ordered AI provider list used by server jobs, plus in-flight OAuth logins.
//! Credentials are stored sealed (see `secrets`); this module only handles bytes.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgExecutor, PgPool};

use crate::{DbError, DbResult};

pub const KIND_ANTHROPIC_API: &str = "anthropic_api";
pub const KIND_OPENAI_API: &str = "openai_api";
pub const KIND_ANTHROPIC_OAUTH: &str = "anthropic_oauth";
pub const KIND_OPENAI_CODEX_OAUTH: &str = "openai_codex_oauth";

pub const KINDS: [&str; 4] = [
    KIND_ANTHROPIC_API,
    KIND_OPENAI_API,
    KIND_ANTHROPIC_OAUTH,
    KIND_OPENAI_CODEX_OAUTH,
];

/// Secret material stored (sealed) for one provider. Never sent to a browser.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Credential {
    ApiKey {
        api_key: String,
    },
    OAuth {
        access_token: String,
        refresh_token: String,
        /// Unix epoch milliseconds after which the access token must be refreshed.
        expires_at_ms: i64,
        /// Anthropic organization or ChatGPT account id, when the token response names one.
        account_id: Option<String>,
    },
}

/// Pending login state for one in-flight OAuth flow. Stored sealed.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "flow", rename_all = "snake_case")]
pub enum PendingFlow {
    /// Anthropic paste-back: PKCE verifier and the state that must come back with the code.
    Paste { verifier: String, state: String },
    /// ChatGPT device code: ids needed to poll for the authorization code.
    Device {
        device_auth_id: String,
        user_code: String,
        interval_secs: u64,
    },
}

/// Provider row without credential bytes, for admin listings.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct ProviderSummary {
    pub id: i64,
    pub position: i32,
    pub kind: String,
    pub model: String,
    pub enabled: bool,
    pub account_label: String,
    pub last_error: Option<String>,
    pub last_error_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
}

/// Enabled provider with its sealed credential, for the describe job.
#[derive(Clone, sqlx::FromRow)]
pub struct EnabledProvider {
    pub id: i64,
    pub kind: String,
    pub model: String,
    pub sealed: Vec<u8>,
}

#[derive(Clone, sqlx::FromRow)]
pub struct PendingRow {
    pub id: String,
    pub kind: String,
    pub flow: String,
    pub model: String,
    pub sealed: Vec<u8>,
}

pub async fn list(ex: impl PgExecutor<'_>) -> DbResult<Vec<ProviderSummary>> {
    Ok(sqlx::query_as::<_, ProviderSummary>(
        "SELECT id, position, kind, model, enabled, account_label, last_error, last_error_at, last_used_at
         FROM ai_providers ORDER BY position",
    )
    .fetch_all(ex)
    .await?)
}

pub async fn list_enabled(ex: impl PgExecutor<'_>) -> DbResult<Vec<EnabledProvider>> {
    Ok(sqlx::query_as::<_, EnabledProvider>(
        "SELECT id, kind, model, sealed FROM ai_providers WHERE enabled ORDER BY position",
    )
    .fetch_all(ex)
    .await?)
}

/// Appends a provider at the end of the order. Returns its id.
pub async fn insert(
    ex: impl PgExecutor<'_>,
    kind: &str,
    model: &str,
    account_label: &str,
    sealed: &[u8],
) -> DbResult<i64> {
    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO ai_providers (position, kind, model, account_label, sealed)
         VALUES ((SELECT COALESCE(MAX(position), 0) + 1 FROM ai_providers), $1, $2, $3, $4)
         RETURNING id",
    )
    .bind(kind)
    .bind(model)
    .bind(account_label)
    .bind(sealed)
    .fetch_one(ex)
    .await?;
    Ok(id)
}

pub async fn update(ex: impl PgExecutor<'_>, id: i64, model: &str, enabled: bool) -> DbResult<()> {
    let result = sqlx::query(
        "UPDATE ai_providers SET model = $2, enabled = $3, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(model)
    .bind(enabled)
    .execute(ex)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound(format!("ai provider {id}")));
    }
    Ok(())
}

/// Replaces the sealed credential after an OAuth refresh.
pub async fn update_credential(ex: impl PgExecutor<'_>, id: i64, sealed: &[u8]) -> DbResult<()> {
    sqlx::query("UPDATE ai_providers SET sealed = $2, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(sealed)
        .execute(ex)
        .await?;
    Ok(())
}

/// Swaps this provider with its neighbour above (`up`) or below. No-op at the ends.
pub async fn move_by(pool: &PgPool, id: i64, up: bool) -> DbResult<()> {
    let mut tx = pool.begin().await?;
    let current: Option<(i32,)> =
        sqlx::query_as("SELECT position FROM ai_providers WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((position,)) = current else {
        return Err(DbError::NotFound(format!("ai provider {id}")));
    };
    let neighbour_sql = if up {
        "SELECT id, position FROM ai_providers WHERE position < $1 ORDER BY position DESC LIMIT 1"
    } else {
        "SELECT id, position FROM ai_providers WHERE position > $1 ORDER BY position ASC LIMIT 1"
    };
    let neighbour: Option<(i64, i32)> = sqlx::query_as(neighbour_sql)
        .bind(position)
        .fetch_optional(&mut *tx)
        .await?;
    if let Some((other_id, other_position)) = neighbour {
        // The unique position constraint is deferred, so the swap can happen in one statement.
        sqlx::query(
            "UPDATE ai_providers SET position = CASE WHEN id = $1 THEN $3 ELSE $4 END WHERE id IN ($1, $2)",
        )
        .bind(id)
        .bind(other_id)
        .bind(other_position)
        .bind(position)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn delete(ex: impl PgExecutor<'_>, id: i64) -> DbResult<()> {
    let result = sqlx::query("DELETE FROM ai_providers WHERE id = $1")
        .bind(id)
        .execute(ex)
        .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound(format!("ai provider {id}")));
    }
    Ok(())
}

pub async fn record_success(ex: impl PgExecutor<'_>, id: i64) -> DbResult<()> {
    sqlx::query(
        "UPDATE ai_providers SET last_used_at = now(), last_error = NULL, last_error_at = NULL WHERE id = $1",
    )
    .bind(id)
    .execute(ex)
    .await?;
    Ok(())
}

pub async fn record_failure(ex: impl PgExecutor<'_>, id: i64, message: &str) -> DbResult<()> {
    sqlx::query("UPDATE ai_providers SET last_error = $2, last_error_at = now() WHERE id = $1")
        .bind(id)
        .bind(message)
        .execute(ex)
        .await?;
    Ok(())
}

pub async fn insert_pending(
    ex: impl PgExecutor<'_>,
    id: &str,
    kind: &str,
    flow: &str,
    model: &str,
    sealed: &[u8],
) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO ai_oauth_pending (id, kind, flow, model, sealed, expires_at)
         VALUES ($1, $2, $3, $4, $5, now() + interval '15 minutes')",
    )
    .bind(id)
    .bind(kind)
    .bind(flow)
    .bind(model)
    .bind(sealed)
    .execute(ex)
    .await?;
    Ok(())
}

/// Returns an unexpired pending login.
pub async fn get_pending(ex: impl PgExecutor<'_>, id: &str) -> DbResult<Option<PendingRow>> {
    Ok(sqlx::query_as::<_, PendingRow>(
        "SELECT id, kind, flow, model, sealed FROM ai_oauth_pending WHERE id = $1 AND expires_at > now()",
    )
    .bind(id)
    .fetch_optional(ex)
    .await?)
}

pub async fn delete_pending(ex: impl PgExecutor<'_>, id: &str) -> DbResult<()> {
    sqlx::query("DELETE FROM ai_oauth_pending WHERE id = $1")
        .bind(id)
        .execute(ex)
        .await?;
    Ok(())
}

pub async fn purge_expired_pending(ex: impl PgExecutor<'_>) -> DbResult<()> {
    sqlx::query("DELETE FROM ai_oauth_pending WHERE expires_at <= now()")
        .execute(ex)
        .await?;
    Ok(())
}
