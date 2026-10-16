//! Sign-in identity rules: sign-in, linking, unlinking and admin promotion.

use recomphub_db::identities::{self, IdentityProfile, Provider};
use recomphub_db::{DbError, DbResult, PgPool};

fn github(subject: &str, login: &str) -> IdentityProfile {
    IdentityProfile {
        provider: Provider::GitHub,
        subject: subject.to_string(),
        login: Some(login.to_string()),
        email: None,
        name: None,
        avatar_media_id: None,
    }
}

fn google(subject: &str, email: Option<&str>) -> IdentityProfile {
    IdentityProfile {
        provider: Provider::Google,
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
    let user = identities::sign_in(&pool, &github("42", "octo"), false).await?;
    assert_eq!(user.display_name, "octo");
    assert_eq!(user.github_login.as_deref(), Some("octo"));
    assert_eq!(user.role, "user");

    let again = identities::sign_in(&pool, &github("42", "octo-renamed"), false).await?;
    assert_eq!(
        again.id, user.id,
        "the same provider subject signs into the same user"
    );
    assert_eq!(again.github_login.as_deref(), Some("octo-renamed"));

    let users: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&pool)
        .await?;
    assert_eq!(users, 1);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn google_sign_in_never_merges_by_email(pool: PgPool) -> DbResult<()> {
    let first = identities::sign_in(&pool, &google("g-1", Some("ada@example.com")), false).await?;
    let second = identities::sign_in(&pool, &google("g-2", Some("ada@example.com")), false).await?;
    assert_ne!(
        first.id, second.id,
        "a shared email must not merge two accounts"
    );
    assert_eq!(first.github_login, None);
    assert_eq!(first.display_name, "Ada Lovelace");
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn link_adds_a_second_provider_and_sets_github_login(pool: PgPool) -> DbResult<()> {
    let user = identities::sign_in(&pool, &google("g-1", Some("ada@example.com")), false).await?;
    identities::link(&pool, user.id, &github("42", "octo")).await?;

    let linked = identities::list_for_user(&pool, user.id).await?;
    let providers: Vec<&str> = linked.iter().map(|l| l.provider.as_str()).collect();
    assert_eq!(providers, ["google", "github"]);

    let github_login: Option<String> =
        sqlx::query_scalar("SELECT github_login FROM users WHERE id = $1")
            .bind(user.id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(github_login.as_deref(), Some("octo"));
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn link_refuses_an_account_that_belongs_to_someone_else(pool: PgPool) -> DbResult<()> {
    let owner = identities::sign_in(&pool, &google("g-1", None), false).await?;
    let other = identities::sign_in(&pool, &google("g-2", None), false).await?;
    let err = identities::link(&pool, other.id, &google("g-1", None))
        .await
        .expect_err("subject already belongs to another user");
    assert!(matches!(err, DbError::Conflict(_)), "got {err:?}");

    let owners = identities::list_for_user(&pool, owner.id).await?;
    assert_eq!(owners.len(), 1, "the owner keeps exactly its own identity");
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn link_refuses_a_second_identity_of_the_same_provider(pool: PgPool) -> DbResult<()> {
    let user = identities::sign_in(&pool, &google("g-1", None), false).await?;
    let err = identities::link(&pool, user.id, &google("g-2", None))
        .await
        .expect_err("only one Google account per user");
    match err {
        DbError::Conflict(message) => {
            assert_eq!(
                message,
                "A Google account is already connected to this account."
            )
        }
        other => panic!("expected Conflict, got {other:?}"),
    }
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn relinking_your_own_identity_succeeds(pool: PgPool) -> DbResult<()> {
    let user = identities::sign_in(&pool, &github("42", "octo"), false).await?;
    identities::link(&pool, user.id, &github("42", "octo")).await?;
    assert_eq!(identities::list_for_user(&pool, user.id).await?.len(), 1);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn unlink_refuses_the_last_identity(pool: PgPool) -> DbResult<()> {
    let user = identities::sign_in(&pool, &github("42", "octo"), false).await?;
    let err = identities::unlink(&pool, user.id, Provider::GitHub)
        .await
        .expect_err("the last sign-in method stays");
    assert!(matches!(err, DbError::Conflict(_)), "got {err:?}");
    assert_eq!(identities::list_for_user(&pool, user.id).await?.len(), 1);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn unlink_github_clears_github_login_and_keeps_the_other_identity(
    pool: PgPool,
) -> DbResult<()> {
    let user = identities::sign_in(&pool, &github("42", "octo"), false).await?;
    identities::link(&pool, user.id, &google("g-1", Some("ada@example.com"))).await?;
    identities::unlink(&pool, user.id, Provider::GitHub).await?;

    let remaining = identities::list_for_user(&pool, user.id).await?;
    let providers: Vec<&str> = remaining.iter().map(|l| l.provider.as_str()).collect();
    assert_eq!(providers, ["google"]);

    let github_login: Option<String> =
        sqlx::query_scalar("SELECT github_login FROM users WHERE id = $1")
            .bind(user.id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(github_login, None);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn unlink_reports_a_provider_that_is_not_connected(pool: PgPool) -> DbResult<()> {
    let user = identities::sign_in(&pool, &google("g-1", None), false).await?;
    identities::link(&pool, user.id, &github("42", "octo")).await?;
    identities::unlink(&pool, user.id, Provider::GitHub).await?;
    let err = identities::unlink(&pool, user.id, Provider::GitHub)
        .await
        .expect_err("GitHub is already disconnected");
    assert!(matches!(err, DbError::NotFound(_)), "got {err:?}");
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn banned_users_cannot_link_or_unlink(pool: PgPool) -> DbResult<()> {
    let user = identities::sign_in(&pool, &google("g-1", None), false).await?;
    set_banned(&pool, user.id).await?;

    let link = identities::link(&pool, user.id, &github("42", "octo"))
        .await
        .expect_err("banned users cannot link");
    assert!(matches!(link, DbError::Banned), "got {link:?}");

    let unlink = identities::unlink(&pool, user.id, Provider::GitHub)
        .await
        .expect_err("banned users cannot unlink");
    assert!(matches!(unlink, DbError::Banned), "got {unlink:?}");
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn admin_flag_promotes_and_never_demotes(pool: PgPool) -> DbResult<()> {
    let plain = identities::sign_in(&pool, &google("g-1", Some("ada@example.com")), false).await?;
    assert_eq!(plain.role, "user");

    let promoted =
        identities::sign_in(&pool, &google("g-1", Some("ada@example.com")), true).await?;
    assert_eq!(promoted.role, "admin");

    let kept = identities::sign_in(&pool, &google("g-1", Some("ada@example.com")), false).await?;
    assert_eq!(
        kept.role, "admin",
        "a later sign-in without the flag keeps the role"
    );
    Ok(())
}

#[test]
fn display_name_falls_back_from_name_to_login_to_email() {
    let mut profile = github("42", "octo");
    profile.name = Some("  Octo Cat  ".to_string());
    assert_eq!(profile.display_name(), "Octo Cat");

    profile.name = Some("   ".to_string());
    assert_eq!(profile.display_name(), "octo");

    let email_only = IdentityProfile {
        provider: Provider::Google,
        subject: "g".to_string(),
        login: None,
        email: Some("ada@example.com".to_string()),
        name: None,
        avatar_media_id: None,
    };
    assert_eq!(email_only.display_name(), "ada");

    let nothing = IdentityProfile {
        email: None,
        ..email_only
    };
    assert_eq!(nothing.display_name(), "Member");
}

#[test]
fn provider_text_round_trips() {
    for provider in [Provider::GitHub, Provider::Google] {
        assert_eq!(Provider::parse(provider.as_str()), Some(provider));
    }
    assert_eq!(Provider::parse("facebook"), None);
}
