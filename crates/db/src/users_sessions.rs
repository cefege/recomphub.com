//! Accounts, roles, bans and cookie-backed sessions.

use crate::{DbError, DbResult};
use chrono::{DateTime, Utc};
use recomphub_shared::Role;
use sha2::{Digest, Sha256};
use sqlx::PgExecutor;

/// A row of the `users` table.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UserRow {
    pub id: i64,
    pub github_id: i64,
    pub login: String,
    pub name: Option<String>,
    pub avatar_media_id: Option<i64>,
    pub role: String,
    pub approved_count: i32,
    pub banned: bool,
    pub created_at: DateTime<Utc>,
}

impl UserRow {
    /// Parses the stored role.
    pub fn role(&self) -> DbResult<Role> {
        Ok(self.role.parse::<Role>()?)
    }
}

/// Profile fields taken from GitHub at login.
#[derive(Debug, Clone)]
pub struct GithubProfile {
    pub github_id: i64,
    pub login: String,
    pub name: Option<String>,
    pub avatar_media_id: Option<i64>,
}

/// SHA-256 of a session token. Only this hash is stored.
pub fn session_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// Creates or refreshes the user for a GitHub identity. `is_admin` promotes
/// the account to admin; otherwise the existing role is kept.
pub async fn upsert_github_user(
    ex: impl PgExecutor<'_>,
    profile: &GithubProfile,
    is_admin: bool,
) -> DbResult<UserRow> {
    let role = if is_admin { "admin" } else { "user" };
    let row = sqlx::query_as::<_, UserRow>(
        "INSERT INTO users (github_id, login, name, avatar_media_id, role)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (github_id) DO UPDATE SET
            login = EXCLUDED.login,
            name = EXCLUDED.name,
            avatar_media_id = EXCLUDED.avatar_media_id,
            role = CASE WHEN $6 THEN 'admin' ELSE users.role END
         RETURNING *",
    )
    .bind(profile.github_id)
    .bind(profile.login.clone())
    .bind(profile.name.clone())
    .bind(profile.avatar_media_id)
    .bind(role)
    .bind(is_admin)
    .fetch_one(ex)
    .await?;
    Ok(row)
}

/// Stores a session keyed by the hash of `token`.
pub async fn create_session(
    ex: impl PgExecutor<'_>,
    token: &str,
    user_id: i64,
    expires_at: DateTime<Utc>,
) -> DbResult<()> {
    sqlx::query("INSERT INTO sessions (id_hash, user_id, expires_at) VALUES ($1, $2, $3)")
        .bind(session_hash(token))
        .bind(user_id)
        .bind(expires_at)
        .execute(ex)
        .await?;
    Ok(())
}

/// The active (unexpired, unbanned) user behind a session token, if any.
pub async fn find_by_session(ex: impl PgExecutor<'_>, token: &str) -> DbResult<Option<UserRow>> {
    Ok(find_session_user(ex, token)
        .await?
        .filter(|user| !user.banned))
}

/// The user behind an unexpired session token, banned or not. Lets callers answer
/// a banned account with 403 instead of treating it as signed out.
pub async fn find_session_user(ex: impl PgExecutor<'_>, token: &str) -> DbResult<Option<UserRow>> {
    let row = sqlx::query_as::<_, UserRow>(
        "SELECT u.* FROM sessions s JOIN users u ON u.id = s.user_id
         WHERE s.id_hash = $1 AND s.expires_at > now()",
    )
    .bind(session_hash(token))
    .fetch_optional(ex)
    .await?;
    Ok(row)
}

/// Deletes the session for a token.
pub async fn delete_session(ex: impl PgExecutor<'_>, token: &str) -> DbResult<()> {
    sqlx::query("DELETE FROM sessions WHERE id_hash = $1")
        .bind(session_hash(token))
        .execute(ex)
        .await?;
    Ok(())
}

/// Removes expired sessions and returns how many were deleted.
pub async fn purge_expired_sessions(ex: impl PgExecutor<'_>) -> DbResult<u64> {
    let r = sqlx::query("DELETE FROM sessions WHERE expires_at <= now()")
        .execute(ex)
        .await?;
    Ok(r.rows_affected())
}

/// A user by id.
pub async fn get_by_id(ex: impl PgExecutor<'_>, id: i64) -> DbResult<UserRow> {
    sqlx::query_as::<_, UserRow>("SELECT * FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(ex)
        .await?
        .ok_or_else(|| DbError::NotFound(format!("user {id}")))
}

/// A user by GitHub login, compared case-insensitively.
pub async fn get_by_login(ex: impl PgExecutor<'_>, login: &str) -> DbResult<Option<UserRow>> {
    let row = sqlx::query_as::<_, UserRow>("SELECT * FROM users WHERE lower(login) = lower($1)")
        .bind(login)
        .fetch_optional(ex)
        .await?;
    Ok(row)
}

/// Users whose login contains `q` (case-insensitive), at most `limit` rows.
pub async fn search(ex: impl PgExecutor<'_>, q: &str, limit: i64) -> DbResult<Vec<UserRow>> {
    let pattern = format!("%{}%", q.replace('%', "\\%").replace('_', "\\_"));
    let rows = sqlx::query_as::<_, UserRow>(
        "SELECT * FROM users WHERE login ILIKE $1 ORDER BY login LIMIT $2",
    )
    .bind(pattern)
    .bind(limit)
    .fetch_all(ex)
    .await?;
    Ok(rows)
}

/// Sets a user's role.
pub async fn set_role(ex: impl PgExecutor<'_>, id: i64, role: Role) -> DbResult<()> {
    let r = sqlx::query("UPDATE users SET role = $2 WHERE id = $1")
        .bind(id)
        .bind(role.as_str())
        .execute(ex)
        .await?;
    if r.rows_affected() == 0 {
        return Err(DbError::NotFound(format!("user {id}")));
    }
    Ok(())
}

/// Bans or unbans a user. Banning does not delete sessions, but
/// `find_by_session` stops returning banned users immediately.
pub async fn set_banned(ex: impl PgExecutor<'_>, id: i64, banned: bool) -> DbResult<()> {
    let r = sqlx::query("UPDATE users SET banned = $2 WHERE id = $1")
        .bind(id)
        .bind(banned)
        .execute(ex)
        .await?;
    if r.rows_affected() == 0 {
        return Err(DbError::NotFound(format!("user {id}")));
    }
    Ok(())
}

async fn fetch_active(ex: impl PgExecutor<'_>, id: i64, lock: bool) -> DbResult<UserRow> {
    let sql = if lock {
        "SELECT * FROM users WHERE id = $1 FOR UPDATE"
    } else {
        "SELECT * FROM users WHERE id = $1"
    };
    let user = sqlx::query_as::<_, UserRow>(sql)
        .bind(id)
        .fetch_optional(ex)
        .await?
        .ok_or_else(|| DbError::NotFound(format!("user {id}")))?;
    if user.banned {
        return Err(DbError::Banned);
    }
    Ok(user)
}

/// Returns the user, or `Banned` if the account is banned.
pub async fn ensure_active(ex: impl PgExecutor<'_>, id: i64) -> DbResult<UserRow> {
    fetch_active(ex, id, false).await
}

/// Like `ensure_active`, but locks the user row until the transaction ends.
pub async fn lock_active(ex: impl PgExecutor<'_>, id: i64) -> DbResult<UserRow> {
    fetch_active(ex, id, true).await
}

/// Adds one approved submission to the user's trust count.
pub async fn increment_approved(ex: impl PgExecutor<'_>, id: i64) -> DbResult<()> {
    sqlx::query("UPDATE users SET approved_count = approved_count + 1 WHERE id = $1")
        .bind(id)
        .execute(ex)
        .await?;
    Ok(())
}
