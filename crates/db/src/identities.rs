//! Sign-in identities: which external accounts belong to which RecompHub user.
//!
//! An identity is keyed by the provider's stable subject: the Google OpenID `sub`, the GitHub
//! user id, or the Cloudflare Access token's `sub` claim. Email is stored for display and admin
//! matching only, never as a key. Nothing here merges accounts by email.

use sqlx::{PgConnection, PgPool};

use crate::DbResult;
use crate::users_sessions::UserRow;

/// The sign-in provider. Google and GitHub sign members in; Cloudflare Access signs in the
/// admin. The stored text is the `provider` column value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Google,
    GitHub,
    Cloudflare,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Google => "google",
            Provider::GitHub => "github",
            Provider::Cloudflare => "cloudflare",
        }
    }
}

/// What the provider told us about the account at sign-in time.
#[derive(Debug, Clone)]
pub struct IdentityProfile {
    pub provider: Provider,
    /// The provider's stable account id (`sub` for Google and Access, user id for GitHub).
    pub subject: String,
    /// The GitHub login for GitHub identities; unused otherwise.
    pub login: Option<String>,
    /// The account email, when the provider shares one.
    pub email: Option<String>,
    /// The provider's display name, when it has one.
    pub name: Option<String>,
    pub avatar_media_id: Option<i64>,
}

impl IdentityProfile {
    /// The name shown for the member: the provider's name, else the email's local part,
    /// else a neutral word.
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

/// Signs in the user behind `profile`, creating the user when the identity is new.
/// Refreshes the name, identity fields and avatar from the provider. `is_admin`
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

/// Copies the provider's profile onto the user. The avatar only changes when the provider
/// has one. `github_login` follows the GitHub identity only; Cloudflare sign-ins leave it
/// alone, so the author link keeps its verified GitHub login.
async fn refresh_user(
    conn: &mut PgConnection,
    user_id: i64,
    profile: &IdentityProfile,
    is_admin: bool,
) -> DbResult<UserRow> {
    let row = sqlx::query_as::<_, UserRow>(
        "UPDATE users SET
            display_name = $2,
            avatar_media_id = COALESCE($3, avatar_media_id),
            github_login = COALESCE($5, github_login),
            role = CASE WHEN $4 THEN 'admin' ELSE role END
         WHERE id = $1
         RETURNING *",
    )
    .bind(user_id)
    .bind(profile.display_name())
    .bind(profile.avatar_media_id)
    .bind(is_admin)
    .bind(match profile.provider {
        Provider::GitHub => profile.login.as_deref(),
        Provider::Google | Provider::Cloudflare => None,
    })
    .fetch_one(conn)
    .await?;
    Ok(row)
}
