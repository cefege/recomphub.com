//! Sign-in identities: which provider accounts belong to which RecompHub user.
//!
//! An identity is keyed by the provider's stable subject (GitHub's numeric user id,
//! Google's `sub`). Email is stored for display and admin matching only, never as a key.
//! Nothing here merges accounts by email. Two provider accounts share one user only
//! when a signed-in user links the second one from the account page.

use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor, PgPool};

use crate::users_sessions::{UserRow, lock_active};
use crate::{DbError, DbResult};

/// A sign-in provider. The stored text is the `provider` column value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    GitHub,
    Google,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::GitHub => "github",
            Provider::Google => "google",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Provider::GitHub => "GitHub",
            Provider::Google => "Google",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "github" => Some(Provider::GitHub),
            "google" => Some(Provider::Google),
            _ => None,
        }
    }
}

/// What a provider told us about the account at sign-in or link time.
#[derive(Debug, Clone)]
pub struct IdentityProfile {
    pub provider: Provider,
    /// The provider's stable id: GitHub's numeric user id as text, or Google's `sub`.
    pub subject: String,
    /// The GitHub login. None for Google.
    pub login: Option<String>,
    /// The email, only when the provider verified it.
    pub email: Option<String>,
    pub name: Option<String>,
    pub avatar_media_id: Option<i64>,
}

impl IdentityProfile {
    /// The name shown for the member: the provider's name, else the login, else the
    /// email's local part, else a neutral word.
    pub fn display_name(&self) -> String {
        self.name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .or_else(|| self.login.clone())
            .or_else(|| {
                self.email
                    .as_deref()
                    .and_then(|email| email.split('@').next())
                    .filter(|local| !local.is_empty())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "Member".to_string())
    }
}

/// A connected provider account, as listed on the account page.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LinkedIdentity {
    pub provider: String,
    pub login: Option<String>,
    pub email: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Signs in the user behind `profile`, creating the user when the identity is new.
/// Refreshes the name, avatar and identity fields from the provider. `is_admin`
/// promotes the user to admin and never demotes.
///
/// Banned users are returned too; callers decide how to refuse them.
pub async fn sign_in(
    pool: &PgPool,
    profile: &IdentityProfile,
    is_admin: bool,
) -> DbResult<UserRow> {
    let mut tx = pool.begin().await?;
    let existing = sqlx::query_scalar::<_, i64>(
        "SELECT user_id FROM user_identities WHERE provider = $1 AND subject = $2",
    )
    .bind(profile.provider.as_str())
    .bind(&profile.subject)
    .fetch_optional(&mut *tx)
    .await?;

    let user = match existing {
        Some(user_id) => {
            update_identity_fields(&mut tx, profile).await?;
            refresh_user(&mut tx, user_id, profile, is_admin).await?
        }
        None => {
            let user_id = sqlx::query_scalar::<_, i64>(
                "INSERT INTO users (display_name, role) VALUES ($1, 'user') RETURNING id",
            )
            .bind(profile.display_name())
            .fetch_one(&mut *tx)
            .await?;
            insert_identity(&mut tx, user_id, profile).await?;
            refresh_user(&mut tx, user_id, profile, is_admin).await?
        }
    };
    tx.commit().await?;
    Ok(user)
}

/// Connects `profile` to the signed-in user `user_id`. Linking never merges accounts:
/// the user row stays, and the identity is added to it.
///
/// Refused when the provider account already belongs to another user, or when this
/// user already has an identity for the same provider. Re-linking the user's own
/// identity succeeds and refreshes its fields.
pub async fn link(pool: &PgPool, user_id: i64, profile: &IdentityProfile) -> DbResult<()> {
    let mut tx = pool.begin().await?;
    // Locks the user row, which serialises link and unlink for this user. Banned users fail here.
    lock_active(&mut *tx, user_id).await?;

    let owner = sqlx::query_scalar::<_, i64>(
        "SELECT user_id FROM user_identities WHERE provider = $1 AND subject = $2",
    )
    .bind(profile.provider.as_str())
    .bind(&profile.subject)
    .fetch_optional(&mut *tx)
    .await?;

    match owner {
        Some(owner) if owner == user_id => {
            update_identity_fields(&mut tx, profile).await?;
        }
        Some(_) => {
            return Err(DbError::Conflict(format!(
                "That {} account is already connected to a different RecompHub account.",
                profile.provider.label()
            )));
        }
        None => {
            let has_provider = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM user_identities WHERE user_id = $1 AND provider = $2)",
            )
            .bind(user_id)
            .bind(profile.provider.as_str())
            .fetch_one(&mut *tx)
            .await?;
            if has_provider {
                return Err(DbError::Conflict(format!(
                    "A {} account is already connected to this account.",
                    profile.provider.label()
                )));
            }
            insert_identity(&mut tx, user_id, profile).await?;
            if profile.provider == Provider::GitHub {
                sqlx::query("UPDATE users SET github_login = $2 WHERE id = $1")
                    .bind(user_id)
                    .bind(&profile.login)
                    .execute(&mut *tx)
                    .await?;
            }
        }
    }
    tx.commit().await?;
    Ok(())
}

/// Disconnects `provider` from `user_id`. Refused when it is the user's only identity,
/// so the account always keeps a way to sign in. NotFound when the provider is not connected.
pub async fn unlink(pool: &PgPool, user_id: i64, provider: Provider) -> DbResult<()> {
    let mut tx = pool.begin().await?;
    // Locks the user row, which serialises link and unlink for this user. Banned users fail here.
    lock_active(&mut *tx, user_id).await?;

    let connected = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM user_identities WHERE user_id = $1 AND provider = $2)",
    )
    .bind(user_id)
    .bind(provider.as_str())
    .fetch_one(&mut *tx)
    .await?;
    if !connected {
        return Err(DbError::NotFound(format!(
            "{} is not connected to this account",
            provider.label()
        )));
    }

    let total =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM user_identities WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await?;
    if total <= 1 {
        return Err(DbError::Conflict(
            "Connect another sign-in method before disconnecting this one.".to_string(),
        ));
    }

    sqlx::query("DELETE FROM user_identities WHERE user_id = $1 AND provider = $2")
        .bind(user_id)
        .bind(provider.as_str())
        .execute(&mut *tx)
        .await?;
    if provider == Provider::GitHub {
        sqlx::query("UPDATE users SET github_login = NULL WHERE id = $1")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// The provider accounts connected to a user, oldest first.
pub async fn list_for_user(ex: impl PgExecutor<'_>, user_id: i64) -> DbResult<Vec<LinkedIdentity>> {
    let rows = sqlx::query_as::<_, LinkedIdentity>(
        "SELECT provider, login, email, created_at FROM user_identities
         WHERE user_id = $1 ORDER BY created_at, provider",
    )
    .bind(user_id)
    .fetch_all(ex)
    .await?;
    Ok(rows)
}

async fn insert_identity(
    conn: &mut PgConnection,
    user_id: i64,
    profile: &IdentityProfile,
) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO user_identities (provider, subject, user_id, login, email)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(profile.provider.as_str())
    .bind(&profile.subject)
    .bind(user_id)
    .bind(&profile.login)
    .bind(&profile.email)
    .execute(conn)
    .await?;
    Ok(())
}

async fn update_identity_fields(
    conn: &mut PgConnection,
    profile: &IdentityProfile,
) -> DbResult<()> {
    sqlx::query(
        "UPDATE user_identities SET login = $3, email = $4 WHERE provider = $1 AND subject = $2",
    )
    .bind(profile.provider.as_str())
    .bind(&profile.subject)
    .bind(&profile.login)
    .bind(&profile.email)
    .execute(conn)
    .await?;
    Ok(())
}

/// Copies the provider's profile onto the user. The GitHub login is only touched by
/// GitHub sign-ins, and the avatar only changes when the provider has one.
async fn refresh_user(
    conn: &mut PgConnection,
    user_id: i64,
    profile: &IdentityProfile,
    is_admin: bool,
) -> DbResult<UserRow> {
    let github_login = match profile.provider {
        Provider::GitHub => profile.login.as_deref(),
        Provider::Google => None,
    };
    let row = sqlx::query_as::<_, UserRow>(
        "UPDATE users SET
            display_name = $2,
            github_login = COALESCE($3, github_login),
            avatar_media_id = COALESCE($4, avatar_media_id),
            role = CASE WHEN $5 THEN 'admin' ELSE role END
         WHERE id = $1
         RETURNING *",
    )
    .bind(user_id)
    .bind(profile.display_name())
    .bind(github_login)
    .bind(profile.avatar_media_id)
    .bind(is_admin)
    .fetch_one(conn)
    .await?;
    Ok(row)
}
