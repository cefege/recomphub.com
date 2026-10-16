//! Author funding storage. Each test gets a fresh database with the migrations applied.

use chrono::{Duration, Utc};
use recomphub_db::author_funding::{self, FundingRecord, FundingSource};
use recomphub_db::{DbResult, PgPool};
use recomphub_shared::FundingPlatform;

async fn make_game(pool: &PgPool) -> DbResult<i64> {
    Ok(sqlx::query_scalar::<_, i64>(
        "INSERT INTO games (igdb_id, slug, name, popularity, want_count, port_count)
         VALUES (1, 'game', 'Game', 0, 0, 0) RETURNING id",
    )
    .fetch_one(pool)
    .await?)
}

async fn make_port(pool: &PgPool, game_id: i64, slug: &str, repo_url: &str) -> DbResult<i64> {
    Ok(sqlx::query_scalar::<_, i64>(
        "INSERT INTO ports (game_id, slug, name, kind, repo_url, declared_platforms,
                            github_stars, star_count, status)
         VALUES ($1, $2, $3, 'static_recomp', $4, '{}', 5, 5, 'published') RETURNING id",
    )
    .bind(game_id)
    .bind(slug)
    .bind(format!("Port {slug}"))
    .bind(repo_url)
    .fetch_one(pool)
    .await?)
}

async fn add_author(pool: &PgPool, port_id: i64, login: &str) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO port_authors (port_id, position, name, role, github_login)
         VALUES ($1, (SELECT COALESCE(MAX(position) + 1, 0) FROM port_authors WHERE port_id = $1),
                 $2, 'owner', $2)",
    )
    .bind(port_id)
    .bind(login)
    .execute(pool)
    .await?;
    Ok(())
}

fn record(platform: FundingPlatform, url: &str) -> FundingRecord {
    FundingRecord {
        platform,
        url: url.to_string(),
        source: FundingSource::Profile,
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn replace_links_is_case_insensitive_and_replaces_the_set(pool: PgPool) -> DbResult<()> {
    let mut conn = pool.acquire().await?;
    author_funding::replace_links(
        &mut conn,
        "Octocat",
        &[
            record(FundingPlatform::KoFi, "https://ko-fi.com/a"),
            record(FundingPlatform::Patreon, "https://www.patreon.com/a"),
        ],
    )
    .await?;
    drop(conn);

    let rows = author_funding::for_logins(&pool, &["OCTOCAT".to_string()]).await?;
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.login == "octocat"));

    let mut conn = pool.acquire().await?;
    author_funding::replace_links(
        &mut conn,
        "octocat",
        &[record(
            FundingPlatform::BuyMeACoffee,
            "https://www.buymeacoffee.com/b",
        )],
    )
    .await?;
    drop(conn);

    let rows = author_funding::for_logins(&pool, &["octocat".to_string()]).await?;
    let urls: Vec<&str> = rows.iter().map(|r| r.url.as_str()).collect();
    assert_eq!(urls, vec!["https://www.buymeacoffee.com/b"]);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn due_logins_respects_the_check_time(pool: PgPool) -> DbResult<()> {
    let game = make_game(&pool).await?;
    let port = make_port(&pool, game, "p1", "https://github.com/example/p1").await?;
    add_author(&pool, port, "Octocat").await?;
    add_author(&pool, port, "other").await?;

    let mut conn = pool.acquire().await?;
    author_funding::replace_links(&mut conn, "octocat", &[]).await?;
    drop(conn);

    let now = Utc::now();
    let stale_cutoff = now - Duration::days(1);
    let due = author_funding::due_logins(&pool, stale_cutoff, 100).await?;
    assert_eq!(due, vec!["other".to_string()]);

    let due = author_funding::due_logins(&pool, now + Duration::days(1), 100).await?;
    assert!(due.contains(&"octocat".to_string()));
    assert!(due.contains(&"other".to_string()));
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn owned_repo_urls_keeps_only_github_repos_of_the_owner(pool: PgPool) -> DbResult<()> {
    let game = make_game(&pool).await?;
    make_port(&pool, game, "gh", "https://github.com/octocat/gh").await?;
    make_port(&pool, game, "gl", "https://gitlab.com/octocat/gl").await?;
    make_port(&pool, game, "other", "https://github.com/someone/other").await?;

    let urls = author_funding::owned_repo_urls(&pool, "octocat", 3).await?;
    assert_eq!(urls, vec!["https://github.com/octocat/gh".to_string()]);
    Ok(())
}
