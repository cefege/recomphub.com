#![cfg(feature = "server")]

//! Consumer-visible behaviour of the members and moderation server functions,
//! run against a throwaway Postgres database created for this test binary.

use chrono::{Duration, Utc};
use dioxus::fullstack::{FullstackContext, http};
use dioxus::prelude::ServerFnError;
use rand::Rng;
use recomphub::api::members::{me, my_marks, submit_report, toggle_star, toggle_want};
use recomphub::api::moderation::{
    admin_settings, admin_users, approve, moderation_log, moderation_queue, reject, search_games,
    set_banned, set_role, update_setting,
};
use recomphub::server::session::{SESSION_COOKIE, new_token};
use recomphub::server::state::init_pool;
use recomphub_db::identities::{self, IdentityProfile, Provider};
use recomphub_db::users_sessions;
use recomphub_shared::dto::ReviewOverrides;
use sqlx::{Connection, PgConnection, PgPool, postgres::PgPoolOptions};

struct TestDb {
    pool: PgPool,
    admin_url: String,
    name: String,
}

struct Viewer {
    id: i64,
    token: String,
}

struct Fixtures {
    game: i64,
    port: i64,
    other_port: i64,
}

#[tokio::test(flavor = "multi_thread")]
async fn members_and_moderation_layer() {
    let db = create_db().await;
    assert!(
        init_pool(db.pool.clone()).is_ok(),
        "server pool already initialised"
    );
    let outcome = tokio::spawn(scenarios(db.pool.clone())).await;
    drop_db(db).await;
    if let Err(err) = outcome {
        std::panic::resume_unwind(err.into_panic());
    }
}

async fn scenarios(pool: PgPool) {
    let fx = fixtures(&pool).await;
    signed_out(&fx).await;

    let plain = user(&pool, 1001, "plain-test", false).await;
    let moderator = user(&pool, 1002, "mod-test", false).await;
    sqlx::query("UPDATE users SET role = 'moderator' WHERE id = $1")
        .bind(moderator.id)
        .execute(&pool)
        .await
        .expect("promote moderator");
    let admin = user(&pool, 1003, "admin-test", true).await;
    let target = user(&pool, 1004, "target-test", false).await;

    plain_user(&fx, &plain).await;
    moderator_user(&fx, &moderator, &plain).await;
    admin_user(&fx, &admin, &plain, &target).await;
}

async fn signed_out(fx: &Fixtures) {
    expect_status(as_guest(toggle_star(fx.port)).await, 401);
    expect_status(as_guest(toggle_want(fx.game)).await, 401);
    expect_status(
        as_guest(submit_report(fx.game, None, "broken".into())).await,
        401,
    );
    expect_status(as_guest(moderation_queue()).await, 401);
    expect_status(as_guest(admin_settings()).await, 401);
    expect_status(as_guest(search_games("test".into())).await, 401);
    assert!(as_guest(me()).await.expect("me").is_none());
    let marks = as_guest(my_marks(vec![fx.port], vec![fx.game]))
        .await
        .expect("marks");
    assert!(marks.starred.is_empty() && marks.wanted.is_empty());
}

async fn plain_user(fx: &Fixtures, plain: &Viewer) {
    expect_status(as_user(plain, moderation_queue()).await, 403);
    expect_status(as_user(plain, moderation_log(1)).await, 403);
    expect_status(as_user(plain, admin_users("x".into())).await, 403);
    expect_status(as_user(plain, admin_settings()).await, 403);
    expect_status(
        as_user(plain, update_setting("catalog_target_size".into(), 6000)).await,
        403,
    );

    let signed_in = as_user(plain, me()).await.expect("me").expect("signed in");
    assert_eq!(signed_in.github_login.as_deref(), Some("plain-test"));

    let on = as_user(plain, toggle_star(fx.port)).await.expect("star on");
    assert!(on.starred);
    assert_eq!(on.count, 1);
    let marks = as_user(plain, my_marks(vec![fx.port], vec![]))
        .await
        .expect("marks");
    assert_eq!(marks.starred, vec![fx.port]);
    let off = as_user(plain, toggle_star(fx.port))
        .await
        .expect("star off");
    assert!(!off.starred);
    assert_eq!(off.count, 0);

    let want = as_user(plain, toggle_want(fx.game)).await.expect("want on");
    assert!(want.wanted);
    assert_eq!(want.count, 1);
    let marks = as_user(plain, my_marks(vec![], vec![fx.game]))
        .await
        .expect("marks");
    assert_eq!(marks.wanted, vec![fx.game]);
    let unwant = as_user(plain, toggle_want(fx.game))
        .await
        .expect("want off");
    assert!(!unwant.wanted);
    assert_eq!(unwant.count, 0);

    let hits = as_user(plain, search_games("test game".into()))
        .await
        .expect("search");
    assert!(!hits.is_empty());

    expect_status(
        as_user(plain, submit_report(fx.game, Some(fx.port), String::new())).await,
        400,
    );
    expect_status(
        as_user(
            plain,
            submit_report(fx.game, Some(fx.port), "x".repeat(1001)),
        )
        .await,
        400,
    );
    expect_status(
        as_user(
            plain,
            submit_report(fx.game, Some(fx.other_port), "wrong game".into()),
        )
        .await,
        404,
    );
    as_user(
        plain,
        submit_report(
            fx.game,
            Some(fx.port),
            "Broken link on the port page".into(),
        ),
    )
    .await
    .expect("report");
}

async fn moderator_user(fx: &Fixtures, moderator: &Viewer, plain: &Viewer) {
    let queue = as_user(moderator, moderation_queue()).await.expect("queue");
    let report = queue
        .iter()
        .find(|item| item.kind == "report")
        .expect("report is queued");
    assert_eq!(report.status, "pending");
    let first = report.id;
    as_user(moderator, reject(first, "Thanks, noted".into()))
        .await
        .expect("reject");
    let after = as_user(moderator, moderation_queue()).await.expect("queue");
    assert!(after.iter().all(|item| item.id != first));

    as_user(
        plain,
        submit_report(fx.game, Some(fx.port), "Second report".into()),
    )
    .await
    .expect("second report");
    let queue = as_user(moderator, moderation_queue()).await.expect("queue");
    let second = queue
        .iter()
        .find(|item| item.kind == "report")
        .expect("second report queued")
        .id;
    as_user(moderator, approve(second, ReviewOverrides::default()))
        .await
        .expect("approve");
    let after = as_user(moderator, moderation_queue()).await.expect("queue");
    assert!(after.iter().all(|item| item.id != second));

    expect_status(
        as_user(
            moderator,
            approve(
                second,
                ReviewOverrides {
                    kind: Some("not-a-kind".into()),
                    ..Default::default()
                },
            ),
        )
        .await,
        400,
    );
    expect_status(
        as_user(moderator, approve(999_999_999, ReviewOverrides::default())).await,
        404,
    );

    as_user(moderator, moderation_log(1))
        .await
        .expect("log page 1");
    expect_status(as_user(moderator, moderation_log(99)).await, 404);

    expect_status(as_user(moderator, admin_users("x".into())).await, 403);
    expect_status(as_user(moderator, set_banned(plain.id, true)).await, 403);
}

async fn admin_user(fx: &Fixtures, admin: &Viewer, plain: &Viewer, target: &Viewer) {
    let found = as_user(admin, admin_users("plain-test".into()))
        .await
        .expect("users");
    assert!(found.iter().any(|row| row.id == plain.id));
    let none = as_user(admin, admin_users("zz-nobody".into()))
        .await
        .expect("users");
    assert!(none.is_empty());

    let settings = as_user(admin, admin_settings()).await.expect("settings");
    assert!(
        settings
            .iter()
            .any(|row| row.key == "catalog_target_size" && row.value == 5000)
    );
    as_user(admin, update_setting("catalog_target_size".into(), 6000))
        .await
        .expect("update");
    let settings = as_user(admin, admin_settings()).await.expect("settings");
    assert!(
        settings
            .iter()
            .any(|row| row.key == "catalog_target_size" && row.value == 6000)
    );
    as_user(admin, update_setting("catalog_target_size".into(), 5000))
        .await
        .expect("restore");
    expect_status(
        as_user(admin, update_setting("no_such_setting".into(), 1)).await,
        400,
    );

    expect_status(as_user(admin, set_banned(admin.id, true)).await, 400);
    expect_status(
        as_user(admin, set_role(plain.id, "bogus".into())).await,
        400,
    );

    as_user(admin, set_role(plain.id, "moderator".into()))
        .await
        .expect("promote");
    as_user(plain, moderation_queue())
        .await
        .expect("promoted user sees the queue");
    as_user(admin, set_role(plain.id, "user".into()))
        .await
        .expect("demote");
    expect_status(as_user(plain, moderation_queue()).await, 403);

    as_user(admin, set_banned(target.id, true))
        .await
        .expect("ban");
    assert!(as_user(target, me()).await.expect("me").is_none());
    expect_status(as_user(target, toggle_star(fx.port)).await, 403);
    expect_status(as_user(target, toggle_want(fx.game)).await, 403);
    as_user(admin, set_banned(target.id, false))
        .await
        .expect("unban");
    assert!(as_user(target, me()).await.expect("me").is_some());
    as_user(target, toggle_want(fx.game))
        .await
        .expect("unbanned user can act");
}

async fn fixtures(pool: &PgPool) -> Fixtures {
    let game = insert_game(pool, 9001, "test-game-a", "Test Game A").await;
    let port = insert_port(pool, game, "test-port-a", "https://github.com/test/port-a").await;
    let other_game = insert_game(pool, 9002, "test-game-b", "Test Game B").await;
    let other_port = insert_port(
        pool,
        other_game,
        "test-port-b",
        "https://github.com/test/port-b",
    )
    .await;
    Fixtures {
        game,
        port,
        other_port,
    }
}

async fn insert_game(pool: &PgPool, igdb_id: i64, slug: &str, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO games (igdb_id, slug, name) VALUES ($1, $2, $3) RETURNING id")
        .bind(igdb_id)
        .bind(slug)
        .bind(name)
        .fetch_one(pool)
        .await
        .expect("insert game")
}

async fn insert_port(pool: &PgPool, game_id: i64, slug: &str, repo_url: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO ports (game_id, slug, name, kind, repo_url)
         VALUES ($1, $2, $2, 'static_recomp', $3) RETURNING id",
    )
    .bind(game_id)
    .bind(slug)
    .bind(repo_url)
    .fetch_one(pool)
    .await
    .expect("insert port")
}

async fn user(pool: &PgPool, github_id: i64, login: &str, is_admin: bool) -> Viewer {
    let profile = IdentityProfile {
        provider: Provider::GitHub,
        subject: github_id.to_string(),
        login: Some(login.to_string()),
        email: None,
        name: None,
        avatar_media_id: None,
    };
    let row = identities::sign_in(pool, &profile, is_admin)
        .await
        .expect("sign in user");
    let token = new_token();
    users_sessions::create_session(pool, &token, row.id, Utc::now() + Duration::days(1))
        .await
        .expect("create session");
    Viewer { id: row.id, token }
}

async fn as_guest<T>(call: impl Future<Output = T>) -> T {
    scoped(None, call).await
}

async fn as_user<T>(viewer: &Viewer, call: impl Future<Output = T>) -> T {
    scoped(Some(&viewer.token), call).await
}

/// Runs a server function inside a request context that carries the session cookie.
async fn scoped<T>(token: Option<&str>, call: impl Future<Output = T>) -> T {
    let mut builder = http::Request::builder().uri("/");
    if let Some(token) = token {
        builder = builder.header(http::header::COOKIE, format!("{SESSION_COOKIE}={token}"));
    }
    let (parts, ()) = builder.body(()).expect("request").into_parts();
    FullstackContext::new(parts).scope(call).await
}

fn expect_status<T>(result: Result<T, ServerFnError>, status: u16) {
    match result {
        Ok(_) => panic!("expected HTTP {status}, got Ok"),
        Err(err) => match err {
            ServerFnError::ServerError { code, .. } => {
                assert_eq!(code, status, "unexpected status for {err:?}")
            }
            other => panic!("expected HTTP {status}, got {other:?}"),
        },
    }
}

async fn create_db() -> TestDb {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for the test run");
    let (base, _) = url
        .rsplit_once('/')
        .expect("DATABASE_URL must end in a database name");
    let admin_url = format!("{base}/postgres");
    let name = format!("recomphub_t_{:016x}", rand::rng().random::<u64>());
    let mut admin = PgConnection::connect(&admin_url)
        .await
        .expect("connect to postgres");
    sqlx::query(&format!("CREATE DATABASE \"{name}\""))
        .execute(&mut admin)
        .await
        .expect("create test database");
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&format!("{base}/{name}"))
        .await
        .expect("connect to test database");
    recomphub_db::migrate(&pool)
        .await
        .expect("migrate test database");
    TestDb {
        pool,
        admin_url,
        name,
    }
}

async fn drop_db(db: TestDb) {
    let TestDb {
        pool,
        admin_url,
        name,
    } = db;
    pool.close().await;
    let mut admin = PgConnection::connect(&admin_url)
        .await
        .expect("connect to postgres");
    sqlx::query(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
        .execute(&mut admin)
        .await
        .expect("drop test database");
}
