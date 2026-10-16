//! Sign-in identities: which Google accounts belong to which RecompHub user.
//!
//! An identity is keyed by Google's stable OpenID `sub`. Email is stored for the admin list and
//! admin matching only, never as a key. Nothing here merges accounts by email.

use sqlx::{PgConnection, PgPool};

use crate::DbResult;
use crate::users_sessions::UserRow;

/// The only sign-in provider; the stored `provider` column value.
const PROVIDER: &str = "google";

/// What Google told us about the account at sign-in time.
#[derive(Debug, Clone)]
pub struct IdentityProfile {
    /// Google's stable account id (`sub`).
    pub subject: String,
    /// The account email, when Google shares one.
    pub email: Option<String>,
    /// The account's display name, when it has one.
    pub name: Option<String>,
    pub avatar_media_id: Option<i64>,
}

impl IdentityProfile {
    /// The name shown for the member: the account name, else the email's local part, else a
    /// neutral word.
    pub fn display_name(&self) -> String {
        self.name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
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
/// Refreshes the name, email and avatar from Google. `is_admin` promotes the user to admin and
/// never demotes.
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
    .bind(PROVIDER)
    .bind(&profile.subject)
    .fetch_optional(&mut *tx)
    .await?;

    let user_id = match existing {
        Some(user_id) => {
            sqlx::query(
                "UPDATE user_identities SET email = $3 WHERE provider = $1 AND subject = $2",
            )
            .bind(PROVIDER)
            .bind(&profile.subject)
            .bind(&profile.email)
            .execute(&mut *tx)
            .await?;
            user_id
        }
        None => {
            let user_id = sqlx::query_scalar::<_, i64>(
                "INSERT INTO users (display_name, role) VALUES ($1, 'user') RETURNING id",
            )
            .bind(profile.display_name())
            .fetch_one(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO user_identities (provider, subject, user_id, email)
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(PROVIDER)
            .bind(&profile.subject)
            .bind(user_id)
            .bind(&profile.email)
            .execute(&mut *tx)
            .await?;
            user_id
        }
    };
    let user = refresh_user(&mut tx, user_id, profile, is_admin).await?;
    tx.commit().await?;
    Ok(user)
}

/// Copies Google's profile onto the user. The avatar only changes when Google has one.
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
            role = CASE WHEN $4 THEN 'admin' ELSE role END
         WHERE id = $1
         RETURNING *",
    )
    .bind(user_id)
    .bind(profile.display_name())
    .bind(profile.avatar_media_id)
    .bind(is_admin)
    .fetch_one(conn)
    .await?;
    Ok(row)
}
