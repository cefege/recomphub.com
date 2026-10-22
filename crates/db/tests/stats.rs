//! Platform parity snapshots, backfill and the `/stats` overview. Each test gets a fresh database.

use std::collections::HashSet;

use chrono::{Duration, NaiveDate, Utc};
use recomphub_db::games::{self, IgdbGame};
use recomphub_db::stats;
use recomphub_db::submissions::{self, ApproveOverrides, NewSubmission};
use recomphub_db::{DbResult, PgPool};
use recomphub_shared::SubmissionKind;
use serde_json::json;

async fn make_user(pool: &PgPool, login: &str, approved: i32) -> DbResult<i64> {
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO users (display_name, github_login, role, approved_count)
         VALUES ($1, $2, 'user', $3) RETURNING id",
    )
    .bind(login)
    .bind(login)
    .bind(approved)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// A published port for a new game, approved by a moderator.
async fn make_port(pool: &PgPool, owner: i64, igdb_id: i64, slug: &str) -> DbResult<i64> {
    let mut conn = pool.acquire().await?;
    let g = IgdbGame {
        igdb_id,
        name: slug.to_string(),
        slug: slug.to_string(),
        ..Default::default()
    };
    let game = games::sync_igdb(&mut conn, &g).await?;
    drop(conn);
    let sub = submissions::create(
        pool,
        &NewSubmission {
            kind: SubmissionKind::NewPort,
            source: "user".to_string(),
            submitter_id: Some(owner),
            payload: json!({
                "game_id": game,
                "name": format!("{slug} port"),
                "kind": "static_recomp",
                "repo_url": format!("https://github.com/a/{slug}"),
                "playability": "complete"
            }),
            target_game_id: None,
            target_port_id: None,
        },
    )
    .await?;
    let moderator = sqlx::query_scalar::<_, i64>(
        "INSERT INTO users (display_name, github_login, role) VALUES ('moderator', 'moderator', 'moderator') RETURNING id",
    )
    .fetch_one(pool)
    .await?;
    let approved =
        submissions::approve(pool, sub.id, moderator, &ApproveOverrides::default(), None).await?;
    Ok(approved
        .target_port_id
        .expect("approved submission sets the port id"))
}

/// Sets the declared platforms and the day the port was added.
async fn set_port(pool: &PgPool, port: i64, platforms: &[&str], added: NaiveDate) -> DbResult<()> {
    let platforms: Vec<String> = platforms.iter().map(|p| p.to_string()).collect();
    sqlx::query(
        "UPDATE ports SET declared_platforms = $2,
                          created_at = ($3::date + time '12:00') AT TIME ZONE 'UTC'
         WHERE id = $1",
    )
    .bind(port)
    .bind(platforms)
    .bind(added)
    .execute(pool)
    .await?;
    Ok(())
}

async fn stored(pool: &PgPool, day: NaiveDate, metric: &str, key: &str) -> DbResult<i64> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT value FROM stats_daily WHERE day = $1 AND metric = $2 AND key = $3",
    )
    .bind(day)
    .bind(metric)
    .bind(key)
    .fetch_one(pool)
    .await?)
}

async fn stored_row_count(pool: &PgPool) -> DbResult<i64> {
    Ok(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM stats_daily")
            .fetch_one(pool)
            .await?,
    )
}

#[sqlx::test(migrations = "./migrations")]
async fn snapshot_is_idempotent_and_backfills_past_days(pool: PgPool) -> DbResult<()> {
    let owner = make_user(&pool, "owner", 3).await?;
    let today = Utc::now().date_naive();
    let p1 = make_port(&pool, owner, 1, "game-a").await?;
    let p2 = make_port(&pool, owner, 2, "game-b").await?;
    set_port(&pool, p1, &["windows", "linux"], today - Duration::days(3)).await?;
    set_port(&pool, p2, &["linux"], today - Duration::days(1)).await?;

    let first = stats::run_snapshot(&pool, today).await?;
    assert_eq!(first.backfill_from, Some(today - Duration::days(3)));
    assert_eq!(first.backfill_days, 3);
    assert!(first.backfill_rows > 0);
    let rows = stored_row_count(&pool).await?;

    let second = stats::run_snapshot(&pool, today).await?;
    assert_eq!(
        second.backfill_rows, first.backfill_rows,
        "backfill is recomputed each run"
    );
    assert_eq!(stored_row_count(&pool).await?, rows);

    assert_eq!(
        stored(&pool, today - Duration::days(3), "ports", "all").await?,
        1
    );
    assert_eq!(
        stored(&pool, today - Duration::days(2), "ports", "all").await?,
        1
    );
    assert_eq!(
        stored(&pool, today - Duration::days(1), "ports", "all").await?,
        2
    );
    assert_eq!(stored(&pool, today, "ports", "all").await?, 2);
    assert_eq!(
        stored(&pool, today - Duration::days(3), "target_games", "linux").await?,
        1
    );
    assert_eq!(
        stored(&pool, today - Duration::days(1), "target_games", "linux").await?,
        2
    );
    assert_eq!(stored(&pool, today, "target_games", "windows").await?, 1);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn backfill_is_recomputed_and_snapshots_are_kept(pool: PgPool) -> DbResult<()> {
    let owner = make_user(&pool, "owner", 3).await?;
    let today = Utc::now().date_naive();
    let p1 = make_port(&pool, owner, 1, "game-a").await?;
    set_port(&pool, p1, &["windows"], today - Duration::days(2)).await?;
    stats::run_snapshot(&pool, today).await?;

    // A stale backfill row is recomputed; a stale snapshot row is kept.
    sqlx::query(
        "UPDATE stats_daily SET value = 999
         WHERE day = $1 AND metric = 'ports' AND key = 'all'",
    )
    .bind(today - Duration::days(2))
    .execute(&pool)
    .await?;
    sqlx::query(
        "UPDATE stats_daily SET value = 999, source = 'snapshot'
         WHERE day = $1 AND metric = 'ports' AND key = 'all'",
    )
    .bind(today - Duration::days(1))
    .execute(&pool)
    .await?;
    sqlx::query(
        "UPDATE stats_daily SET value = 0
         WHERE day = $1 AND metric = 'ports' AND key = 'all'",
    )
    .bind(today)
    .execute(&pool)
    .await?;

    stats::run_snapshot(&pool, today).await?;
    assert_eq!(
        stored(&pool, today - Duration::days(2), "ports", "all").await?,
        1
    );
    assert_eq!(
        stored(&pool, today - Duration::days(1), "ports", "all").await?,
        999
    );
    assert_eq!(stored(&pool, today, "ports", "all").await?, 1);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn timeline_uses_repo_creation_date_when_known(pool: PgPool) -> DbResult<()> {
    let owner = make_user(&pool, "owner", 3).await?;
    let today = Utc::now().date_naive();
    let p1 = make_port(&pool, owner, 1, "game-a").await?;
    set_port(&pool, p1, &["windows"], today - Duration::days(1)).await?;
    sqlx::query(
        "UPDATE ports SET repo_created_at = ($2::date + time '12:00') AT TIME ZONE 'UTC'
         WHERE id = $1",
    )
    .bind(p1)
    .bind(today - Duration::days(5))
    .execute(&pool)
    .await?;

    stats::run_snapshot(&pool, today).await?;
    assert_eq!(
        stored(&pool, today - Duration::days(5), "ports", "all").await?,
        1
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn overview_reports_parity_gaps_contributors_and_achievements(pool: PgPool) -> DbResult<()> {
    let owner = make_user(&pool, "owner", 3).await?;
    let today = Utc::now().date_naive();
    let p1 = make_port(&pool, owner, 1, "game-a").await?;
    let p2 = make_port(&pool, owner, 2, "game-b").await?;
    set_port(
        &pool,
        p1,
        &["windows", "macos", "linux", "web", "android", "ios"],
        today - Duration::days(1),
    )
    .await?;
    set_port(&pool, p2, &["linux"], today).await?;
    sqlx::query(
        "INSERT INTO port_authors (port_id, position, github_login, name, role)
         VALUES ($1, 0, 'Alice', 'Alice', 'owner')",
    )
    .bind(p1)
    .execute(&pool)
    .await?;

    let page = stats::stats_page(&pool, today).await?;
    let parity = &page.parity;
    assert_eq!(parity.ported_games, 2);
    assert_eq!(parity.ports, 2);
    assert_eq!(parity.slots_covered, 7);
    assert_eq!(parity.slots_total, 12);
    assert_eq!(parity.ports_in_period, 2);
    assert_eq!(parity.slots_in_period, 7);
    assert_eq!(parity.pct_30d, None, "no stored snapshot from 30 days ago");
    let linux = parity.targets.iter().find(|t| t.slug == "linux").unwrap();
    assert_eq!(linux.games, 2);
    assert_eq!(parity.targets.iter().map(|t| t.rank).max(), Some(11));

    assert_eq!(page.history.last().map(|h| h.day), Some(today));
    assert_eq!(page.history.last().map(|h| h.slots), Some(7));
    assert_eq!(page.gaps.len(), 1);
    assert_eq!(page.gaps[0].slug, "game-b");
    assert_eq!(page.gaps[0].missing.len(), 5);
    assert_eq!(page.most_covered[0].slug, "game-a");
    assert_eq!(page.most_covered[0].covered, 6);

    let contributor = parity.contributors.first().expect("alice is credited");
    assert_eq!(contributor.login, "alice");
    assert_eq!(contributor.count, 6);

    let keys: HashSet<String> = stats::author_achievements(&pool, "ALICE")
        .await?
        .into_iter()
        .map(|a| a.key)
        .collect();
    for expected in [
        "first_port",
        "platform_hopper",
        "penguin_pioneer",
        "open_door",
        "full_house",
    ] {
        assert!(keys.contains(expected), "missing {expected}: {keys:?}");
    }
    assert!(!keys.contains("prolific"));
    Ok(())
}
