#![cfg(feature = "server")]

//! Database-backed public launcher-offer contracts. DATABASE_URL must point at a
//! disposable test database; this binary creates and drops its own isolated DB.

use chrono::Utc;
use recomphub::api::launcher::install_offers;
use recomphub::server::state::init_pool;
use recomphub_db::{
    PgPool,
    games::IgdbGame,
    ports::{self, PortFull, PortInput},
};
use recomphub_shared::{CpuArch, HostOs, HostPlatform, InstallManifest, InstallOffersQuery};
use sqlx::{Connection, PgConnection, postgres::PgPoolOptions};

struct TestDb {
    pool: PgPool,
    admin_url: String,
    name: String,
}

#[tokio::test]
async fn public_offers_suppress_stale_build_after_current_catalog_changes() {
    let db = create_db().await;
    init_pool(db.pool.clone()).expect("install process-global test pool");
    let (game, port) = create_port(
        &db.pool,
        730001,
        "banjo-offer-fixture",
        "Banjo offer fixture",
        "https://github.com/BanjoRecomp/BanjoRecomp",
    )
    .await;

    let reviewed = recomphub_ingest::launcher::recipes().expect("reviewed recipes");
    let recipe = reviewed
        .iter()
        .find(|r| r.id == "banjo-recompiled-macos")
        .expect("reviewed macOS recipe");
    let offer = manifest(recipe, port, game);
    recomphub_db::launcher::upsert(&db.pool, &offer, true, Utc::now())
        .await
        .expect("publish valid build");
    let windows_recipe = reviewed
        .iter()
        .find(|recipe| recipe.id == "banjo-recompiled-windows")
        .expect("reviewed Windows recipe");
    let windows_offer = manifest(windows_recipe, port, game);
    recomphub_db::launcher::upsert(&db.pool, &windows_offer, true, Utc::now())
        .await
        .expect("publish Windows build");
    // One corrupt persisted build must not poison the independently valid offers.
    let mut damaged = serde_json::to_value(&offer).expect("serialize build");
    damaged["schema_version"] = serde_json::json!(999);
    damaged["recipe"]["id"] = serde_json::json!("damaged-fixture");
    sqlx::query("INSERT INTO launcher_builds (recipe_id, port_id, manifest, enabled, checked_at) VALUES ($1, $2, $3, true, NOW())")
        .bind("damaged-fixture").bind(port).bind(damaged).execute(&db.pool)
        .await.expect("insert damaged fixture row");

    let host = HostPlatform {
        os: HostOs::Macos,
        arch: CpuArch::Aarch64,
        os_version: "14.0".into(),
    };
    let page = install_offers(InstallOffersQuery {
        host: host.clone(),
        page: 1,
        port_id: None,
    })
    .await
    .expect("offer page");
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].port.id, port);
    assert_eq!(page.items[0].manifest.recipe.id, "banjo-recompiled-macos");
    let windows_host = HostPlatform {
        os: HostOs::Windows,
        arch: CpuArch::X86_64,
        os_version: "10.0".into(),
    };
    let windows = install_offers(InstallOffersQuery {
        host: windows_host,
        page: 1,
        port_id: Some(port),
    })
    .await
    .expect("Windows offers");
    assert_eq!(windows.total, 1);
    assert_eq!(
        windows.items[0].manifest.recipe.id,
        "banjo-recompiled-windows"
    );
    let missing = install_offers(InstallOffersQuery {
        host: host.clone(),
        page: 1,
        port_id: Some(port + 1),
    })
    .await
    .expect("no matching port");
    assert_eq!(missing.total, 0);
    assert!(missing.items.is_empty());
    assert!(
        install_offers(InstallOffersQuery {
            host: host.clone(),
            page: 0,
            port_id: None
        })
        .await
        .is_err()
    );
    sqlx::query(
        "UPDATE ports SET repo_url = 'https://github.com/BaNjOrEcOmP/BaNjOrEcOmP' WHERE id = $1",
    )
    .bind(port)
    .execute(&db.pool)
    .await
    .expect("change repository casing");
    let mixed_case = install_offers(InstallOffersQuery {
        host: host.clone(),
        page: 1,
        port_id: None,
    })
    .await
    .expect("case-insensitive current repository agreement");
    assert_eq!(mixed_case.total, 1);

    // Each current-catalog mismatch suppresses the stale build immediately.
    let other_game = create_game(&db.pool, 730002, "banjo-offer-other", "Unrelated fixture").await;
    sqlx::query("UPDATE ports SET game_id = $2 WHERE id = $1")
        .bind(port)
        .bind(other_game)
        .execute(&db.pool)
        .await
        .expect("reassign port game");
    let page = install_offers(InstallOffersQuery {
        host: host.clone(),
        page: 1,
        port_id: None,
    })
    .await
    .expect("offer page after reassignment");
    assert!(page.items.is_empty());

    sqlx::query("UPDATE ports SET game_id = $2, repo_url = 'https://github.com/example/changed' WHERE id = $1").bind(port).bind(game).execute(&db.pool).await.expect("reassign port repository");
    let page = install_offers(InstallOffersQuery {
        host: host.clone(),
        page: 1,
        port_id: None,
    })
    .await
    .expect("offer page after repository change");
    assert!(page.items.is_empty());

    sqlx::query("UPDATE ports SET repo_url = 'https://github.com/BanjoRecomp/BanjoRecomp', status = 'hidden' WHERE id = $1").bind(port).execute(&db.pool).await.expect("hide port");
    let page = install_offers(InstallOffersQuery {
        host,
        page: 1,
        port_id: None,
    })
    .await
    .expect("offer page after hide");
    assert!(page.items.is_empty());

    drop_db(db).await;
}

async fn create_game(pool: &PgPool, igdb_id: i64, slug: &str, name: &str) -> i64 {
    let mut conn = pool.acquire().await.expect("connection");
    recomphub_db::games::upsert_igdb(
        &mut conn,
        &IgdbGame {
            igdb_id,
            slug: slug.into(),
            name: name.into(),
            ..Default::default()
        },
    )
    .await
    .expect("insert fixture game")
}

fn manifest(
    recipe: &recomphub_shared::InstallRecipe,
    port_id: i64,
    game_id: i64,
) -> InstallManifest {
    let tag = "v1.0.2";
    let asset_name = recipe
        .expected_asset_name(tag)
        .expect("reviewed asset template");
    let (asset_id, size_bytes, sha256) = if recipe.os == HostOs::Windows {
        (
            497090208,
            18_669_475,
            "fc34aad48e06681878bedd13b88d5332a2f21f6a98cafc931c94d54892a4dd92",
        )
    } else {
        (
            516056378,
            17_704_672,
            "5e0bbcf671064f05d7bf61c39b9c8f9eb851b0f9de4545882a607b78de8f8072",
        )
    };
    InstallManifest {
        schema_version: recomphub_shared::INSTALL_MANIFEST_SCHEMA_VERSION,
        recipe: recipe.clone(),
        port_id,
        game_id,
        release_id: 363285793,
        asset_id,
        tag: tag.into(),
        version: "1.0.2".into(),
        published_at: Utc::now(),
        release_url: format!("https://github.com/BanjoRecomp/BanjoRecomp/releases/tag/{tag}"),
        download_url: format!(
            "https://github.com/BanjoRecomp/BanjoRecomp/releases/download/{tag}/{asset_name}"
        ),
        asset_name,
        size_bytes,
        sha256: sha256.into(),
    }
}

/// Local-only catalog fixture for a real worker `launcher-sync` run. This is
/// intentionally ignored by default and writes only to the explicitly supplied
/// disposable database. Run with:
/// `DATABASE_URL=... cargo test -p recomphub --features server --test launcher seed_local_launcher_fixture -- --ignored --exact`
#[tokio::test]
#[ignore = "explicitly seeds labeled local catalog rows"]
async fn seed_local_launcher_fixture() {
    #[derive(serde::Deserialize)]
    struct CuratedPort {
        repo_url: String,
        game: String,
        kind: String,
    }
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must target disposable local DB");
    require_local_test_database(&url);
    let pool = PgPool::connect(&url)
        .await
        .expect("connect local fixture DB");
    recomphub_db::migrate(&pool)
        .await
        .expect("migrate isolated fixture DB");
    let curated: Vec<CuratedPort> =
        serde_json::from_str(include_str!("../../../seed/curated-ports.json"))
            .expect("parse curated seed");
    let banjo = curated
        .into_iter()
        .find(|entry| entry.repo_url == "https://github.com/BanjoRecomp/BanjoRecomp")
        .expect("Banjo seed entry");
    let mut conn = pool.acquire().await.expect("connection");
    let game_id = recomphub_db::games::upsert_igdb(
        &mut conn,
        &IgdbGame {
            igdb_id: 973001,
            slug: "recomphub-local-fixture-banjo-kazooie".into(),
            name: format!("LOCAL FIXTURE: {}", banjo.game),
            platform_igdb_ids: vec![18],
            ..Default::default()
        },
    )
    .await
    .expect("create labeled fixture game");
    let port_id = ports::insert_port(
        &mut conn,
        game_id,
        &PortFull {
            input: PortInput {
                name: "LOCAL FIXTURE: Banjo Recompiled".into(),
                slug: Some("local-fixture-banjo-recompiled".into()),
                kind: banjo.kind,
                repo_url: banjo.repo_url,
                playability: "playable".into(),
                declared_platforms: vec!["macos".into(), "windows".into()],
                ..Default::default()
            },
            authors: vec![],
        },
        &["local-only-fixture".into()],
    )
    .await
    .expect("create fixture managed port");
    let source_game_id = recomphub_db::games::upsert_igdb(
        &mut conn,
        &IgdbGame {
            igdb_id: 973002,
            slug: "recomphub-local-fixture-banjo-source".into(),
            name: "LOCAL FIXTURE: Banjo Source Project".into(),
            platform_igdb_ids: vec![18],
            ..Default::default()
        },
    )
    .await
    .expect("create source-only fixture game");
    ports::insert_port(
        &mut conn,
        source_game_id,
        &PortFull {
            input: PortInput {
                name: "LOCAL FIXTURE: source-only project".into(),
                slug: Some("local-fixture-source-only".into()),
                kind: "decompilation".into(),
                repo_url: "https://github.com/recomphub-local-fixtures/banjo-source-only".into(),
                playability: "unknown".into(),
                ..Default::default()
            },
            authors: vec![],
        },
        &["local-only-fixture".into()],
    )
    .await
    .expect("create source-only fixture port");
    println!(
        "Created local fixture catalog rows: game_id={game_id}, port_id={port_id}, source_game_id={source_game_id}"
    );
}

async fn create_port(
    pool: &PgPool,
    igdb_id: i64,
    slug: &str,
    name: &str,
    repo: &str,
) -> (i64, i64) {
    let mut tx = pool.begin().await.expect("transaction");
    let game_id = recomphub_db::games::upsert_igdb(
        &mut tx,
        &IgdbGame {
            igdb_id,
            slug: slug.into(),
            name: name.into(),
            platform_igdb_ids: vec![18],
            ..Default::default()
        },
    )
    .await
    .expect("insert game");
    let port_id = ports::insert_port(
        &mut tx,
        game_id,
        &PortFull {
            input: PortInput {
                name: format!("{name} Recompiled"),
                slug: Some(format!("{slug}-port")),
                kind: "static_recomp".into(),
                repo_url: repo.into(),
                playability: "playable".into(),
                declared_platforms: vec!["macos".into()],
                ..Default::default()
            },
            authors: vec![],
        },
        &["launcher-integration-fixture".into()],
    )
    .await
    .expect("insert published port");
    tx.commit().await.expect("commit fixture");
    (game_id, port_id)
}

async fn create_db() -> TestDb {
    let url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL must be set to disposable test DB");
    require_local_test_database(&url);
    let (base, _) = url.rsplit_once('/').expect("database URL");
    let admin_url = format!("{base}/postgres");
    let name = format!("recomphub_launcher_{:016x}", rand::random::<u64>());
    let mut admin = PgConnection::connect(&admin_url)
        .await
        .expect("connect admin");
    sqlx::query(&format!("CREATE DATABASE \"{name}\""))
        .execute(&mut admin)
        .await
        .expect("create isolated DB");
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&format!("{base}/{name}"))
        .await
        .expect("connect isolated DB");
    recomphub_db::migrate(&pool)
        .await
        .expect("migrate isolated DB");
    TestDb {
        pool,
        admin_url,
        name,
    }
}

async fn drop_db(db: TestDb) {
    db.pool.close().await;
    let mut admin = PgConnection::connect(&db.admin_url)
        .await
        .expect("reconnect admin");
    sqlx::query(&format!(
        "DROP DATABASE IF EXISTS \"{}\" WITH (FORCE)",
        db.name
    ))
    .execute(&mut admin)
    .await
    .expect("drop isolated DB");
}

fn require_local_test_database(value: &str) {
    let url = reqwest::Url::parse(value).expect("parse isolated database URL");
    let loopback = url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    });
    let database = url.path().trim_start_matches('/');
    assert!(
        matches!(url.scheme(), "postgres" | "postgresql")
            && loopback
            && url.query().is_none()
            && url.fragment().is_none()
            && (database.ends_with("_test") || database == "recomphub_desktop"),
        "DATABASE_URL must name an explicit loopback test/desktop fixture database"
    );
}
