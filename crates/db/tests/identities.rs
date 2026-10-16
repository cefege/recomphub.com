//! Sign-in identity rules: first sign-in, subject keying, no email merging, banning and admin promotion.

use recomphub_db::identities::{self, IdentityProfile, Provider};
use recomphub_db::{DbResult, PgPool};

fn cloudflare(subject: &str, email: Option<&str>) -> IdentityProfile {
    IdentityProfile {
        provider: Provider::Cloudflare,
        subject: subject.to_string(),
        login: None,
        email: email.map(str::to_string),
        name: Some("Ada Lovelace".to_string()),
        avatar_media_id: None,
    }
}

async fn set_banned(pool: &PgPool, user_id: i64) -> DbResult<()> {
    sqlx::query("UPDATE users SET banned = true WHERE id = $1")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn first_sign_in_creates_user_and_identity(pool: PgPool) -> DbResult<()> {
    let user =
        identities::sign_in(&pool, &cloudflare("sub-1", Some("ada@example.com")), false).await?;
    assert_eq!(user.display_name, "Ada Lovelace");
    assert_eq!(user.role, "user");

    let again =
        identities::sign_in(&pool, &cloudflare("sub-1", Some("ada@example.com")), false).await?;
    assert_eq!(
        again.id, user.id,
        "the same subject signs into the same user"
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_shared_email_never_merges_two_accounts(pool: PgPool) -> DbResult<()> {
    let first =
        identities::sign_in(&pool, &cloudflare("sub-1", Some("ada@example.com")), false).await?;
    let second =
        identities::sign_in(&pool, &cloudflare("sub-2", Some("ada@example.com")), false).await?;
    assert_ne!(
        first.id, second.id,
        "a shared email must not merge two accounts"
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn admin_promotion_is_granted_and_never_revoked(pool: PgPool) -> DbResult<()> {
    let user =
        identities::sign_in(&pool, &cloudflare("sub-1", Some("mike@example.com")), true).await?;
    assert_eq!(user.role, "admin");
    let later =
        identities::sign_in(&pool, &cloudflare("sub-1", Some("mike@example.com")), false).await?;
    assert_eq!(
        later.role, "admin",
        "a sign-in without the grant keeps the role"
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn banned_users_are_returned_for_the_caller_to_refuse(pool: PgPool) -> DbResult<()> {
    let user = identities::sign_in(&pool, &cloudflare("sub-1", None), false).await?;
    set_banned(&pool, user.id).await?;
    let again = identities::sign_in(&pool, &cloudflare("sub-1", None), false).await?;
    assert!(again.banned);
    Ok(())
}
