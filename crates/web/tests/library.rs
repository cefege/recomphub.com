#![cfg(feature = "server")]

//! Account-library contracts, run only against a disposable Postgres database.

use chrono::{Duration, Utc};
use dioxus::fullstack::{FullstackContext, http};
use dioxus::prelude::ServerFnError;
use rand::Rng;
use recomphub::api::library::{favorite_game_ids, merge_game_favorites, set_game_favorite};
use recomphub::server::session::{SESSION_COOKIE, new_token};
use recomphub::server::state::init_pool;
use recomphub_db::identities::{self, IdentityProfile};
use recomphub_db::users_sessions;
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

#[tokio::test(flavor = "multi_thread")]
async fn private_game_favorites_are_idempotent_and_isolated() {
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
    let game_a = insert_game(&pool, 6001, "source-only", "Source-only Game").await;
    let game_b = insert_game(&pool, 6002, "no-port", "No-port Game").await;
    let game_c = insert_game(&pool, 6003, "native-port", "Native-port Game").await;
    let alice = user(&pool, 7001, "alice-library").await;
    let bob = user(&pool, 7002, "bob-library").await;

    expect_status(as_guest(favorite_game_ids()).await, 401);
    expect_status(as_guest(set_game_favorite(game_a, true)).await, 401);
    expect_status(as_guest(merge_game_favorites(vec![game_a])).await, 401);

    expect_status(as_user(&alice, set_game_favorite(0, true)).await, 400);
    expect_status(
        as_user(&alice, merge_game_favorites((1..=201).collect())).await,
        400,
    );

    let saved = as_user(&alice, set_game_favorite(game_a, true))
        .await
        .expect("save source-only game");
    assert!(saved.favorite);
    sqlx::query("UPDATE game_favorites SET created_at = now() - interval '2 days' WHERE user_id = $1 AND game_id = $2")
        .bind(alice.id)
        .bind(game_a)
        .execute(&pool)
        .await
        .expect("age favorite");
    as_user(&alice, set_game_favorite(game_a, true))
        .await
        .expect("repeat save");
    let timestamp: chrono::DateTime<Utc> = sqlx::query_scalar(
        "SELECT created_at FROM game_favorites WHERE user_id = $1 AND game_id = $2",
    )
    .bind(alice.id)
    .bind(game_a)
    .fetch_one(&pool)
    .await
    .expect("favorite timestamp");
    assert!(timestamp < Utc::now() - Duration::days(1));

    let merged = as_user(&alice, merge_game_favorites(vec![game_b, 999_999, game_b]))
        .await
        .expect("merge favorites");
    assert_eq!(merged.accepted_game_ids, [game_b]);
    assert_eq!(merged.missing_game_ids, [999_999]);
    let ids = as_user(&alice, favorite_game_ids())
        .await
        .expect("list favorites");
    assert_eq!(ids, [game_b, game_a]);

    as_user(&alice, set_game_favorite(game_a, false))
        .await
        .expect("remove favorite");
    as_user(&alice, set_game_favorite(game_a, false))
        .await
        .expect("repeat removal");
    assert_eq!(
        as_user(&alice, favorite_game_ids())
            .await
            .expect("list after removal"),
        [game_b]
    );
    assert!(
        as_user(&bob, favorite_game_ids())
            .await
            .expect("isolated list")
            .is_empty()
    );

    expect_status(as_user(&alice, set_game_favorite(999_999, true)).await, 404);
    as_user(&alice, set_game_favorite(game_c, true))
        .await
        .expect("save cascade game");
    sqlx::query("DELETE FROM games WHERE id = $1")
        .bind(game_c)
        .execute(&pool)
        .await
        .expect("delete game");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM game_favorites WHERE game_id = $1")
            .bind(game_c)
            .fetch_one(&pool)
            .await
            .expect("cascade count"),
        0
    );

    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(alice.id)
        .execute(&pool)
        .await
        .expect("delete user");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM game_favorites WHERE user_id = $1")
            .bind(alice.id)
            .fetch_one(&pool)
            .await
            .expect("user cascade count"),
        0
    );
    sqlx::query("UPDATE users SET banned = true WHERE id = $1")
        .bind(bob.id)
        .execute(&pool)
        .await
        .expect("ban bob");
    expect_status(as_user(&bob, set_game_favorite(game_b, true)).await, 403);
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

async fn user(pool: &PgPool, github_id: i64, login: &str) -> Viewer {
    let profile = IdentityProfile {
        subject: github_id.to_string(),
        email: None,
        name: Some(login.to_string()),
        avatar_media_id: None,
    };
    let row = identities::sign_in(pool, &profile, false)
        .await
        .expect("sign in");
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
        Err(ServerFnError::ServerError { code, .. }) => assert_eq!(code, status),
        Err(other) => panic!("expected HTTP {status}, got {other:?}"),
    }
}

async fn create_db() -> TestDb {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for the test run");
    let (base, _) = url.rsplit_once('/').expect("database URL");
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
