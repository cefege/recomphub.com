//! Database behaviour tests. Each test gets a fresh database with the migrations applied.

use chrono::NaiveDate;
use recomphub_db::activity;
use recomphub_db::games::{self, IgdbGame};
use recomphub_db::hits;
use recomphub_db::media;
use recomphub_db::ports::{self, PortFull};
use recomphub_db::stars;
use recomphub_db::submissions::{self, ApproveOverrides, NewSubmission};
use recomphub_db::{DbResult, PgPool};
use recomphub_shared::SubmissionKind;
use serde_json::{Value, json};

async fn make_user(
    pool: &PgPool,
    github_id: i64,
    login: &str,
    role: &str,
    approved: i32,
) -> DbResult<i64> {
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO users (display_name, github_login, role, approved_count) VALUES ($1, $1, $2, $3) RETURNING id",
    )
    .bind(login.to_string())
    .bind(role.to_string())
    .bind(approved)
    .fetch_one(pool)
    .await?;
    sqlx::query("INSERT INTO user_identities (provider, subject, user_id, login) VALUES ('google', $1, $2, $3)")
        .bind(github_id.to_string())
        .bind(id)
        .bind(login.to_string())
        .execute(pool)
        .await?;
    Ok(id)
}

async fn make_game(pool: &PgPool, igdb_id: i64, name: &str, slug: &str) -> DbResult<i64> {
    let mut conn = pool.acquire().await?;
    let g = IgdbGame {
        igdb_id,
        name: name.to_string(),
        slug: slug.to_string(),
        ..Default::default()
    };
    games::sync_igdb(&mut conn, &g).await
}

fn port_json(game_id: i64, name: &str, repo: &str) -> Value {
    json!({
        "game_id": game_id,
        "name": name,
        "kind": "static_recomp",
        "repo_url": repo,
        "playability": "complete"
    })
}

fn new_port(submitter: i64, payload: Value) -> NewSubmission {
    NewSubmission {
        kind: SubmissionKind::NewPort,
        source: "user".to_string(),
        submitter_id: Some(submitter),
        payload,
        target_game_id: None,
        target_port_id: None,
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn manual_games_get_unique_slugs_and_are_never_refreshed_from_igdb(
    pool: PgPool,
) -> DbResult<()> {
    let synced = make_game(&pool, 9001, "Gods", "gods").await?;
    sqlx::query("UPDATE games SET igdb_synced_at = now() - interval '60 days' WHERE id = $1")
        .bind(synced)
        .execute(&pool)
        .await?;
    let mut conn = pool.acquire().await?;
    let manual = games::ManualGame {
        name: "Gods".to_string(),
        first_release_date: NaiveDate::from_ymd_opt(1991, 1, 1),
        source_url: "https://example.org/catalog/gods".to_string(),
        ..Default::default()
    };
    let first = games::insert_manual(&mut conn, &manual).await?;
    let second = games::insert_manual(&mut conn, &manual).await?;

    let first_row = games::get(&pool, first).await?;
    assert_eq!(first_row.igdb_id, None);
    assert_ne!(first_row.slug, games::get(&pool, synced).await?.slug);
    assert_ne!(games::get(&pool, second).await?.slug, first_row.slug);
    assert_eq!(games::stale_igdb_ids(&pool, 30).await?, vec![9001]);
    let source: Option<String> =
        sqlx::query_scalar("SELECT catalog_source_url FROM games WHERE id = $1")
            .bind(first)
            .fetch_one(&pool)
            .await?;
    assert_eq!(source.as_deref(), Some(manual.source_url.as_str()));
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn media_claims_recover_after_worker_disconnect_and_retry_later(
    pool: PgPool,
) -> DbResult<()> {
    let id = media::enqueue(
        &pool,
        "screenshot",
        "https://example.test/image.png",
        "fixture",
    )
    .await?;
    let mut first = pool.acquire().await?;
    let mut second = pool.acquire().await?;
    assert_eq!(
        media::claim_next(&mut first, 2).await?.map(|row| row.id),
        Some(id)
    );
    assert!(media::claim_next(&mut second, 2).await?.is_none());
    media::release_claim(&mut first, id).await?;
    assert_eq!(
        media::claim_next(&mut second, 2).await?.map(|row| row.id),
        Some(id)
    );
    media::release_claim(&mut second, id).await?;

    media::mark_failed(&pool, id, "temporary source failure").await?;
    assert!(media::next_pending(&pool, 1, 2).await?.is_empty());
    sqlx::query("UPDATE media SET next_attempt_at = now() - interval '1 second' WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await?;
    assert_eq!(media::next_pending(&pool, 1, 2).await?[0].id, id);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn trusted_new_port_requires_review(pool: PgPool) -> DbResult<()> {
    let game = make_game(&pool, 1, "Game A", "game-a").await?;
    let uid = make_user(&pool, 100, "trusted", "user", 3).await?;
    let sub = submissions::create(
        &pool,
        &new_port(
            uid,
            port_json(game, "Port A", "https://github.com/a/port-a"),
        ),
    )
    .await?;
    assert_eq!(sub.status, "pending");
    assert!(sub.target_port_id.is_none());
    let ports_found =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM ports WHERE repo_url = $1")
            .bind("https://github.com/a/port-a")
            .fetch_one(&pool)
            .await?;
    assert_eq!(ports_found, 0);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn untrusted_new_port_stays_pending_without_a_port(pool: PgPool) -> DbResult<()> {
    let game = make_game(&pool, 2, "Game B", "game-b").await?;
    let uid = make_user(&pool, 101, "newbie", "user", 0).await?;
    let sub = submissions::create(
        &pool,
        &new_port(
            uid,
            port_json(game, "Port B", "https://github.com/b/port-b"),
        ),
    )
    .await?;
    assert_eq!(sub.status, "pending");
    assert!(sub.target_port_id.is_none());
    let ports_found =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM ports WHERE repo_url = $1")
            .bind("https://github.com/b/port-b")
            .fetch_one(&pool)
            .await?;
    assert_eq!(ports_found, 0);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn approve_increments_submitter_approved_count(pool: PgPool) -> DbResult<()> {
    let game = make_game(&pool, 3, "Game C", "game-c").await?;
    let uid = make_user(&pool, 102, "newbie2", "user", 0).await?;
    let moderator = make_user(&pool, 103, "mod", "moderator", 0).await?;
    let sub = submissions::create(
        &pool,
        &new_port(
            uid,
            port_json(game, "Port C", "https://github.com/c/port-c"),
        ),
    )
    .await?;
    assert_eq!(sub.status, "pending");
    let approved =
        submissions::approve(&pool, sub.id, moderator, &ApproveOverrides::default(), None).await?;
    assert_eq!(approved.status, "approved");
    assert!(approved.target_port_id.is_some());
    let count = sqlx::query_scalar::<_, i32>("SELECT approved_count FROM users WHERE id = $1")
        .bind(uid)
        .fetch_one(&pool)
        .await?;
    assert_eq!(count, 1);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn revert_of_edit_restores_old_values(pool: PgPool) -> DbResult<()> {
    let game = make_game(&pool, 4, "Game D", "game-d").await?;
    let moderator = make_user(&pool, 104, "mod2", "moderator", 0).await?;
    let created = submissions::create(
        &pool,
        &new_port(
            moderator,
            port_json(game, "Port D", "https://github.com/d/port-d"),
        ),
    )
    .await?;
    assert_eq!(created.status, "pending");
    let approved = submissions::approve(
        &pool,
        created.id,
        moderator,
        &ApproveOverrides::default(),
        None,
    )
    .await?;
    let port_id = approved.target_port_id.expect("approved port created");

    let edit = submissions::create(
        &pool,
        &NewSubmission {
            kind: SubmissionKind::EditPort,
            source: "user".to_string(),
            submitter_id: Some(moderator),
            payload: json!({"name": "Renamed Port"}),
            target_game_id: None,
            target_port_id: Some(port_id),
        },
    )
    .await?;
    assert_eq!(edit.status, "auto_approved");
    let renamed = sqlx::query_scalar::<_, String>("SELECT name FROM ports WHERE id = $1")
        .bind(port_id)
        .fetch_one(&pool)
        .await?;
    assert_eq!(renamed, "Renamed Port");

    let reverted = submissions::revert(&pool, edit.id, moderator).await?;
    assert_eq!(reverted.status, "reverted");
    let restored = sqlx::query_scalar::<_, String>("SELECT name FROM ports WHERE id = $1")
        .bind(port_id)
        .fetch_one(&pool)
        .await?;
    assert_eq!(restored, "Port D");
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn record_hit_dedupes_per_visitor_per_day(pool: PgPool) -> DbResult<()> {
    let game = make_game(&pool, 5, "Game E", "game-e").await?;
    let day = NaiveDate::from_ymd_opt(2026, 1, 10).expect("valid date");
    assert!(hits::record_hit(&pool, game, b"visitor-a", day).await?);
    assert!(!hits::record_hit(&pool, game, b"visitor-a", day).await?);
    assert!(hits::record_hit(&pool, game, b"visitor-b", day).await?);
    let total = sqlx::query_scalar::<_, i32>(
        "SELECT hits FROM game_hits_daily WHERE game_id = $1 AND day = $2",
    )
    .bind(game)
    .bind(day)
    .fetch_one(&pool)
    .await?;
    assert_eq!(total, 2);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn live_minutes_outweigh_drive_by_hits_in_ranking(pool: PgPool) -> DbResult<()> {
    let a = make_game(&pool, 6, "Game F", "game-f").await?;
    let b = make_game(&pool, 7, "Game G", "game-g").await?;
    // Game A: 2 hits and 20 viewer-minutes, so score = 2 + 20/5 = 6.0.
    // Game B: 5 hits and no live viewing, so score = 5.0.
    sqlx::query("INSERT INTO game_hits_daily (game_id, day, hits) VALUES ($1, current_date, 2), ($2, current_date, 5)")
        .bind(a)
        .bind(b)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO game_live_daily (game_id, day, viewer_minutes, peak_viewers) VALUES ($1, current_date, 20, 1)")
        .bind(a)
        .execute(&pool)
        .await?;
    hits::refresh_ranking(&pool).await?;

    let (rank_a, score_a) = sqlx::query_as::<_, (i32, f64)>(
        "SELECT rank, score::float8 FROM game_score_ranking WHERE window_days = 1 AND game_id = $1",
    )
    .bind(a)
    .fetch_one(&pool)
    .await?;
    let (rank_b, score_b) = sqlx::query_as::<_, (i32, f64)>(
        "SELECT rank, score::float8 FROM game_score_ranking WHERE window_days = 1 AND game_id = $1",
    )
    .bind(b)
    .fetch_one(&pool)
    .await?;
    assert!((score_a - 6.0).abs() < 1e-9);
    assert!((score_b - 5.0).abs() < 1e-9);
    assert_eq!(rank_a, 1);
    assert_eq!(rank_b, 2);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn star_unstar_star_records_one_activity_row(pool: PgPool) -> DbResult<()> {
    let game = make_game(&pool, 8, "Game H", "game-h").await?;
    let moderator = make_user(&pool, 105, "mod3", "moderator", 0).await?;
    let created = submissions::create(
        &pool,
        &new_port(
            moderator,
            port_json(game, "Port H", "https://github.com/h/port-h"),
        ),
    )
    .await?;
    assert_eq!(created.status, "pending");
    let approved = submissions::approve(
        &pool,
        created.id,
        moderator,
        &ApproveOverrides::default(),
        None,
    )
    .await?;
    let port_id = approved.target_port_id.expect("approved port created");
    let fan = make_user(&pool, 106, "fan", "user", 0).await?;

    let first = stars::toggle_star(&pool, port_id, fan).await?;
    assert!(first.starred);
    assert_eq!(first.count, 1);
    let second = stars::toggle_star(&pool, port_id, fan).await?;
    assert!(!second.starred);
    assert_eq!(second.count, 0);
    let third = stars::toggle_star(&pool, port_id, fan).await?;
    assert!(third.starred);
    assert_eq!(third.count, 1);

    let rows = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM activity WHERE kind = 'starred' AND port_id = $1",
    )
    .bind(port_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(rows, 1);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn rollup_targets_prefer_declared_over_detected(pool: PgPool) -> DbResult<()> {
    let game = make_game(&pool, 9, "Game I", "game-i").await?;
    let mut declared = PortFull::default();
    declared.input.name = "Declared Port".to_string();
    declared.input.kind = "static_recomp".to_string();
    declared.input.repo_url = "https://github.com/i/declared".to_string();
    declared.input.declared_platforms = vec!["windows".to_string()];

    let mut detected = PortFull::default();
    detected.input.name = "Detected Port".to_string();
    detected.input.kind = "static_recomp".to_string();
    detected.input.repo_url = "https://github.com/i/detected".to_string();
    detected.input.detected_platforms = vec!["linux".to_string()];

    let mut conn = pool.acquire().await?;
    ports::insert_port(&mut conn, game, &declared, &["user".to_string()]).await?;
    ports::insert_port(&mut conn, game, &detected, &["user".to_string()]).await?;
    drop(conn);

    games::refresh_rollup(&pool, game).await?;
    let mut targets =
        sqlx::query_scalar::<_, Vec<String>>("SELECT targets FROM games WHERE id = $1")
            .bind(game)
            .fetch_one(&pool)
            .await?;
    targets.sort();
    assert_eq!(targets, vec!["linux".to_string(), "windows".to_string()]);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn toggle_want_then_record_activity(pool: PgPool) -> DbResult<()> {
    let game = make_game(&pool, 10, "Game J", "game-j").await?;
    let fan = make_user(&pool, 107, "wanter", "user", 0).await?;
    let state = stars::toggle_want(&pool, game, fan).await?;
    assert!(state.wanted);
    assert_eq!(state.count, 1);
    let recent = activity::recent(&pool, 10).await?;
    assert!(
        recent
            .iter()
            .any(|a| a.kind == "wanted" && a.game_id == game)
    );
    Ok(())
}
