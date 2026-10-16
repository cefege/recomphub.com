//! Creator hub query tests. Each test gets a fresh database with the migrations applied.

use recomphub_db::creators::{self, MAJOR_TARGETS};
use recomphub_db::{DbResult, PgPool};
use recomphub_shared::dto::SalesSignal;

async fn make_game(
    pool: &PgPool,
    igdb_id: i64,
    slug: &str,
    popularity: i32,
    want_count: i32,
    port_count: i32,
) -> DbResult<i64> {
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO games (igdb_id, slug, name, popularity, want_count, port_count)
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(igdb_id)
    .bind(slug)
    .bind(format!("Game {slug}"))
    .bind(popularity)
    .bind(want_count)
    .bind(port_count)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

async fn make_port(
    pool: &PgPool,
    game_id: i64,
    slug: &str,
    declared: &[&str],
    stars: i32,
    kind: &str,
    status: &str,
) -> DbResult<i64> {
    let declared: Vec<String> = declared.iter().map(|s| s.to_string()).collect();
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO ports (game_id, slug, name, kind, repo_url, declared_platforms,
                            github_stars, star_count, status)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $7, $8) RETURNING id",
    )
    .bind(game_id)
    .bind(slug)
    .bind(format!("Port {slug}"))
    .bind(kind)
    .bind(format!("https://github.com/example/{slug}"))
    .bind(declared)
    .bind(stars)
    .bind(status)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

async fn make_video(
    pool: &PgPool,
    game_id: i64,
    port_id: Option<i64>,
    target: &str,
    youtube_id: &str,
    hidden: bool,
) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO videos (game_id, port_id, target, youtube_id, title, hidden)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(game_id)
    .bind(port_id)
    .bind(target)
    .bind(youtube_id)
    .bind(format!("Video {youtube_id}"))
    .bind(hidden)
    .execute(pool)
    .await?;
    Ok(())
}

fn slugs_of_games(rows: &[creators::RecompileRow]) -> Vec<String> {
    rows.iter().map(|r| r.game.slug.clone()).collect()
}

#[sqlx::test(migrations = "./migrations")]
async fn recompile_lists_only_unported_games_by_wants_then_importance(
    pool: PgPool,
) -> DbResult<()> {
    make_game(&pool, 1, "a", 1, 5, 0).await?;
    make_game(&pool, 2, "b", 9, 5, 0).await?;
    make_game(&pool, 3, "c", 100, 50, 1).await?;
    make_game(&pool, 4, "d", 100, 0, 0).await?;

    let rows = creators::recompile_page(&pool, 10, 0).await?;
    assert_eq!(slugs_of_games(&rows), vec!["b", "a", "d"]);
    assert_eq!(creators::recompile_count(&pool).await?, 3);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn port_gaps_exclude_complete_hidden_decomp_and_unknown_ports(pool: PgPool) -> DbResult<()> {
    let g1 = make_game(&pool, 1, "g1", 10, 0, 1).await?;
    let g2 = make_game(&pool, 2, "g2", 5, 0, 1).await?;
    // Windows, Linux and macOS: missing web, android and ios.
    let p1 = make_port(
        &pool,
        g1,
        "p1",
        &["windows", "linux", "macos"],
        1,
        "static_recomp",
        "published",
    )
    .await?;
    // Windows only: missing the other five majors, but importance is lower.
    let p2 = make_port(
        &pool,
        g2,
        "p2",
        &["windows"],
        1,
        "static_recomp",
        "published",
    )
    .await?;
    // Has every major target: no gap.
    make_port(
        &pool,
        g1,
        "full",
        &MAJOR_TARGETS,
        1,
        "static_recomp",
        "published",
    )
    .await?;
    // Hidden, decompilation, and no platform data: all excluded.
    make_port(
        &pool,
        g1,
        "hidden",
        &["windows"],
        1,
        "static_recomp",
        "hidden",
    )
    .await?;
    make_port(
        &pool,
        g1,
        "decomp",
        &["windows"],
        1,
        "decompilation",
        "published",
    )
    .await?;
    make_port(&pool, g1, "unknown", &[], 1, "static_recomp", "published").await?;

    let all = creators::port_gaps_page(&pool, None, 10, 0).await?;
    let ids: Vec<i64> = all.iter().map(|r| r.port_id).collect();
    assert_eq!(ids, vec![p1, p2]);
    assert_eq!(creators::port_gaps_count(&pool, None).await?, 2);

    let linux = creators::port_gaps_page(&pool, Some("linux"), 10, 0).await?;
    assert_eq!(
        linux.iter().map(|r| r.port_id).collect::<Vec<_>>(),
        vec![p2]
    );
    assert_eq!(creators::port_gaps_count(&pool, Some("linux")).await?, 1);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn video_gaps_count_game_level_and_port_videos_per_target(pool: PgPool) -> DbResult<()> {
    let g = make_game(&pool, 1, "g", 10, 0, 2).await?;
    let p1 = make_port(
        &pool,
        g,
        "p1",
        &["linux", "windows"],
        1,
        "static_recomp",
        "published",
    )
    .await?;
    let p2 = make_port(
        &pool,
        g,
        "p2",
        &["windows"],
        1,
        "static_recomp",
        "published",
    )
    .await?;

    // A game-level video counts for both ports overall, but not for a specific target.
    make_video(&pool, g, None, "any", "any1", false).await?;
    // A hidden video is never counted.
    make_video(&pool, g, None, "any", "hidden1", true).await?;
    // A Linux video linked to p1 counts for p1 on Linux and overall.
    make_video(&pool, g, Some(p1), "linux", "lin1", false).await?;

    // Overall: p1 has 2 (game-level + linked), so it is not a gap. p2 has 1.
    let overall = creators::video_gaps_page(&pool, None, 10, 0).await?;
    assert_eq!(
        overall.iter().map(|r| r.port_id).collect::<Vec<_>>(),
        vec![p2]
    );
    assert_eq!(overall[0].video_count, 1);
    assert_eq!(creators::video_gaps_count(&pool, None).await?, 1);

    // Linux: p1 has 1 linux video, so it needs more. p2 does not run on Linux.
    let linux = creators::video_gaps_page(&pool, Some("linux"), 10, 0).await?;
    assert_eq!(
        linux.iter().map(|r| r.port_id).collect::<Vec<_>>(),
        vec![p1]
    );
    assert_eq!(linux[0].video_count, 1);
    assert_eq!(creators::video_gaps_count(&pool, Some("linux")).await?, 1);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn sales_and_popularity_reach_every_list(pool: PgPool) -> DbResult<()> {
    let g = make_game(&pool, 1, "seller", 40, 3, 0).await?;
    sqlx::query(
        "INSERT INTO platforms (igdb_id, slug, name, abbreviation)
         VALUES (8, 'ps2', 'PlayStation 2', 'PS2')",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO sales_signals (game_id, list_title, scope, platform_slug, sales_year, rank,
                                    units_sold_millions, listed_title, source_url)
         VALUES ($1, 'Best-selling PS2 games', 'platform', 'ps2', NULL, 3, 21.4, 'Seller',
                 'https://example.org/ps2'),
                ($1, 'Best-selling video games', 'overall', NULL, NULL, 40, 5.0, 'Seller',
                 'https://example.org/all')",
    )
    .bind(g)
    .execute(&pool)
    .await?;
    let best = Some(SalesSignal {
        units_sold_millions: 21.4,
        scope: "platform".to_string(),
        platform: Some("PS2".to_string()),
        sales_year: None,
        rank: Some(3),
    });

    let recompile = creators::recompile_page(&pool, 10, 0).await?;
    assert_eq!(recompile.len(), 1);
    assert_eq!(recompile[0].game.popularity, 40);
    assert_eq!(recompile[0].sale.signal(), best);

    make_port(
        &pool,
        g,
        "seller-port",
        &["windows"],
        5,
        "static_recomp",
        "published",
    )
    .await?;
    let gaps = creators::port_gaps_page(&pool, None, 10, 0).await?;
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].popularity, 40);
    assert_eq!(gaps[0].sale.signal(), best);

    let videos = creators::video_gaps_page(&pool, None, 10, 0).await?;
    assert_eq!(videos.len(), 1);
    assert_eq!(videos[0].popularity, 40);
    assert_eq!(videos[0].sale.signal(), best);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn sql_platform_target_matches_rust_mapping(pool: PgPool) -> DbResult<()> {
    let slugs = [
        "win", "mac", "linux", "browser", "android", "iphone", "ipad", "switch", "psp", "3ds",
        "wiiu", "psvita", "dc",
    ];
    for slug in slugs {
        let sql = sqlx::query_scalar::<_, Option<String>>("SELECT platform_target($1)")
            .bind(slug)
            .fetch_one(&pool)
            .await?;
        let rust = recomphub_shared::TargetPlatform::from_platform_slug(slug)
            .map(|t| t.as_str().to_string());
        assert_eq!(sql, rust, "slug {slug}");
    }
    Ok(())
}
