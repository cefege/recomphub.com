//! Accounts, roles, bans and cookie-backed sessions. Sign-in identities live in `identities`.

use crate::{DbError, DbResult};
use chrono::{DateTime, Utc};
use recomphub_shared::Role;
use sha2::{Digest, Sha256};
use sqlx::PgExecutor;

/// A row of the `users` table.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UserRow {
    pub id: i64,
    /// The name shown for this member, whichever provider it came from.
    pub display_name: String,
    /// The GitHub login, while a GitHub identity is connected. Used for author links.
    pub github_login: Option<String>,
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

/// A user row plus the providers and sign-in emails connected to it, for the admin user list.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UserListRow {
    #[sqlx(flatten)]
    pub user: UserRow,
    pub providers: Vec<String>,
    pub emails: Vec<String>,
}

/// SHA-256 of a session token. Only this hash is stored.
pub fn session_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
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

/// Users whose display name, GitHub login or sign-in email contains `q` (case-insensitive), at
/// most `limit` rows.
pub async fn search(ex: impl PgExecutor<'_>, q: &str, limit: i64) -> DbResult<Vec<UserListRow>> {
    let pattern = format!("%{}%", q.replace('%', "\\%").replace('_', "\\_"));
    let rows = sqlx::query_as::<_, UserListRow>(
        "SELECT u.*,
            ARRAY(SELECT i.provider FROM user_identities i WHERE i.user_id = u.id ORDER BY i.provider) AS providers,
            ARRAY(SELECT DISTINCT i.email FROM user_identities i
                  WHERE i.user_id = u.id AND i.email IS NOT NULL ORDER BY i.email) AS emails
         FROM users u
         WHERE u.display_name ILIKE $1 OR u.github_login ILIKE $1
            OR EXISTS (SELECT 1 FROM user_identities i WHERE i.user_id = u.id AND i.email ILIKE $1)
         ORDER BY u.display_name, u.id
         LIMIT $2",
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
