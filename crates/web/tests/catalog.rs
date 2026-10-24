//! Catalog server functions, seen from the outside: lookups that must 404,
//! sort order, status filters, and page boundaries.
//!
//! Runs against `DATABASE_URL`, which must name a dedicated `*_test`
//! database. The schema is dropped and rebuilt on every run. The server pool
//! is process-wide, so the scenarios live in one test function.

#![cfg(feature = "server")]

use dioxus::prelude::ServerFnError;
use recomphub::api::catalog::{self, GamesQuery};
use recomphub::server::state::init_pool;
use recomphub_db::PgPool;
use recomphub_db::games::{self, IgdbGame};
use recomphub_db::submissions::{self, ApproveOverrides, NewSubmission};
use recomphub_shared::enums::{HitWindow, SubmissionKind};
use serde_json::json;

fn status_of(err: &ServerFnError) -> Option<u16> {
    match err {
        ServerFnError::ServerError { code, .. } => Some(*code),
        _ => None,
    }
}

fn query(page: i64) -> GamesQuery {
    GamesQuery {
        page,
        ..Default::default()
    }
}

async fn fresh_database() -> PgPool {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for catalog tests");
    assert!(
        url.contains("_test"),
        "refusing to reset a database whose URL does not name a *_test database"
    );
    let pool = PgPool::connect(&url).await.expect("connect");
    sqlx::raw_sql("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .execute(&pool)
        .await
        .expect("reset schema");
    recomphub_db::migrate(&pool).await.expect("migrate");
    pool
}

fn igdb(igdb_id: i64, name: &str, slug: &str, popularity: i32) -> IgdbGame {
    IgdbGame {
        igdb_id,
        name: name.to_string(),
        slug: slug.to_string(),
        popularity,
        platform_igdb_ids: vec![18],
        ..Default::default()
    }
}

#[tokio::test]
async fn catalog_endpoints_return_expected_status_and_order() {
    let pool = fresh_database().await;
    assert!(init_pool(pool.clone()).is_ok(), "pool initialised twice");
    recomphub::server::config::install(recomphub::server::config::Config {
        database_url: String::new(),
        site_url: "http://localhost".into(),
        media_dir: "./data/media".into(),
        google_client_id: None,
        google_client_secret: None,
        github_token: None,
        admin_emails: vec![],
        amazon_tag: Some("funnybirthday-20".into()),
    });

    sqlx::query(
        "INSERT INTO platforms (igdb_id, slug, name) VALUES (18, 'nes', 'Nintendo Entertainment System')",
    )
    .execute(&pool)
    .await
    .expect("platform");

    let mut conn = pool.acquire().await.expect("conn");
    let alpha_id = games::sync_igdb(&mut conn, &igdb(1, "Alpha Quest", "alpha-quest", 100))
        .await
        .expect("alpha");
    games::sync_igdb(&mut conn, &igdb(2, "Beta Quest", "beta-quest", 50))
        .await
        .expect("beta");
    games::sync_igdb(&mut conn, &igdb(3, "Gamma Quest", "gamma-quest", 200))
        .await
        .expect("gamma");
    drop(conn);

    // New ports stay pending until a moderator approves them, including those from trusted members.
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users (display_name, github_login, approved_count) VALUES ('tester', 'tester', 3) RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .expect("user");
    let moderator: i64 = sqlx::query_scalar(
        "INSERT INTO users (display_name, github_login, role) VALUES ('moderator', 'moderator', 'moderator') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .expect("moderator");
    let submission = submissions::create(
        &pool,
        &NewSubmission {
            kind: SubmissionKind::NewPort,
            source: "user".to_string(),
            submitter_id: Some(user),
            payload: json!({
                "game_id": alpha_id,
                "name": "Alpha Port",
                "kind": "static_recomp",
                "repo_url": "https://github.com/tester/alpha",
                "playability": "complete"
            }),
            target_game_id: None,
            target_port_id: None,
        },
    )
    .await
    .expect("trusted port");
    assert_eq!(submission.status, "pending");
    submissions::approve(
        &pool,
        submission.id,
        moderator,
        &ApproveOverrides::default(),
        None,
    )
    .await
    .expect("approve port");

    // Unknown slug and unknown id are 404, not empty pages.
    let err = catalog::game_detail("no-such-game".to_string())
        .await
        .expect_err("unknown slug");
    assert_eq!(status_of(&err), Some(404));
    let err = catalog::game_ports(999_999)
        .await
        .expect_err("unknown game id");
    assert_eq!(status_of(&err), Some(404));

    // A known game shows its port.
    let detail = catalog::game_detail("alpha-quest".to_string())
        .await
        .expect("alpha detail");
    assert_eq!(detail.game.name, "Alpha Quest");
    assert_eq!(detail.ports.len(), 1);
    assert_eq!(detail.ports[0].name, "Alpha Port");
    assert!(detail.buy_offers[0].url.contains("tag=funnybirthday-20"));
    assert!(detail.buy_offers[0].url.contains("k=Alpha%20Quest%20NES"));

    // Default sort is popularity; page 0 is treated as page 1.
    let page = catalog::games_index(query(0)).await.expect("index");
    assert_eq!(page.page, 1);
    assert_eq!(page.total, 3);
    let slugs: Vec<&str> = page.items.iter().map(|g| g.slug.as_str()).collect();
    assert_eq!(slugs, ["gamma-quest", "alpha-quest", "beta-quest"]);

    // Name sort is alphabetical, not popularity.
    let by_name = catalog::games_index(GamesQuery {
        sort: Some("name".to_string()),
        page: 1,
        ..Default::default()
    })
    .await
    .expect("name sort");
    let names: Vec<&str> = by_name.items.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(names, ["Alpha Quest", "Beta Quest", "Gamma Quest"]);

    // "wanted" lists only games without a port.
    let wanted = catalog::games_index(GamesQuery {
        status: Some("wanted".to_string()),
        page: 1,
        ..Default::default()
    })
    .await
    .expect("wanted filter");
    let mut wanted_slugs: Vec<&str> = wanted.items.iter().map(|g| g.slug.as_str()).collect();
    wanted_slugs.sort_unstable();
    assert_eq!(wanted.total, 2);
    assert_eq!(wanted_slugs, ["beta-quest", "gamma-quest"]);

    // Unknown filter values are rejected, not ignored.
    let err = catalog::games_index(GamesQuery {
        status: Some("bogus".to_string()),
        page: 1,
        ..Default::default()
    })
    .await
    .expect_err("bad status");
    assert_eq!(status_of(&err), Some(400));
    let err = catalog::games_index(GamesQuery {
        platform: Some("no-such-platform".to_string()),
        page: 1,
        ..Default::default()
    })
    .await
    .expect_err("unknown platform");
    assert_eq!(status_of(&err), Some(404));

    // Three games fit on one page, so page 2 does not exist.
    let err = catalog::games_index(query(2))
        .await
        .expect_err("past last page");
    assert_eq!(status_of(&err), Some(404));

    // No hits yet: the ranking is empty, so page 1 is an empty page and
    // page 2 is past the end.
    let empty = catalog::trending(HitWindow::Week, 1)
        .await
        .expect("trending");
    assert_eq!(empty.total, 0);
    assert!(empty.items.is_empty());
    let err = catalog::trending(HitWindow::Week, 2)
        .await
        .expect_err("trending past last page");
    assert_eq!(status_of(&err), Some(404));

    // Home fills the top-10 from popularity when nothing is scored yet.
    let home = catalog::home(HitWindow::Week).await.expect("home");
    assert!(!home.window_has_scores);
    let trending: Vec<&str> = home.trending.iter().map(|r| r.game.slug.as_str()).collect();
    assert_eq!(trending, ["gamma-quest", "alpha-quest", "beta-quest"]);
    assert_eq!(home.wanted.len(), 2);
    // Batch card lookup keeps the caller's deduplicated order, skips missing ids,
    // and leaves the existing public media mapping untouched.
    let cards = catalog::game_cards_by_ids(vec![alpha_id, 999_999, alpha_id])
        .await
        .expect("batch cards");
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].id, alpha_id);
    assert!(cards[0].cover.is_none());
    let err = catalog::game_cards_by_ids(vec![0])
        .await
        .expect_err("invalid id");
    assert_eq!(status_of(&err), Some(400));
    let err = catalog::game_cards_by_ids((1..=51).collect())
        .await
        .expect_err("batch limit");
    assert_eq!(status_of(&err), Some(400));
}
