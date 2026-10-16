//! Sales signals and the derived `games.sales_score` / `games.importance` columns.

use recomphub_db::games::{self, IgdbGame};
use recomphub_db::sales::{self, SalesScope, SalesSignal};
use recomphub_db::{DbResult, PgPool};

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

fn signal(game_id: i64, units_sold_millions: f64, sales_year: Option<i32>) -> SalesSignal {
    SalesSignal {
        game_id,
        scope: if sales_year.is_some() {
            SalesScope::Year
        } else {
            SalesScope::Overall
        },
        platform_slug: None,
        sales_year,
        rank: None,
        units_sold_millions,
        listed_title: "Listed".to_string(),
        source_url: "https://example.test/list".to_string(),
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn sales_score_is_log2_of_best_units_and_feeds_importance(pool: PgPool) -> DbResult<()> {
    let big = make_game(&pool, 1, "Big Seller", "big-seller").await?;
    let small = make_game(&pool, 2, "Small Seller", "small-seller").await?;
    sqlx::query("UPDATE games SET popularity = 100 WHERE id = $1")
        .bind(big)
        .execute(&pool)
        .await?;

    sales::replace_list(&pool, "Overall", &[signal(big, 80.0, None)]).await?;
    sales::replace_list(
        &pool,
        "Platform",
        &[signal(big, 10.0, None), signal(small, 1.0, None)],
    )
    .await?;
    sales::refresh_sales_scores(&pool).await?;

    let (big_score, big_importance): (i32, i32) =
        sqlx::query_as("SELECT sales_score, importance FROM games WHERE id = $1")
            .bind(big)
            .fetch_one(&pool)
            .await?;
    // The best figure across lists wins: round(300 * log2(1 + 80)) = 1902.
    assert_eq!(big_score, 1902);
    assert_eq!(big_importance, 100 + 1902);

    let small_score: i32 = sqlx::query_scalar("SELECT sales_score FROM games WHERE id = $1")
        .bind(small)
        .fetch_one(&pool)
        .await?;
    // round(300 * log2(1 + 1)) = 300.
    assert_eq!(small_score, 300);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn duplicate_entries_keep_the_larger_figure_and_replacing_clears_old_rows(
    pool: PgPool,
) -> DbResult<()> {
    let game = make_game(&pool, 3, "Duplicated", "duplicated").await?;

    let stored = sales::replace_list(
        &pool,
        "Year",
        &[signal(game, 2.0, Some(1990)), signal(game, 5.0, Some(1990))],
    )
    .await?;
    assert_eq!(stored, 1);
    sales::refresh_sales_scores(&pool).await?;
    let score: i32 = sqlx::query_scalar("SELECT sales_score FROM games WHERE id = $1")
        .bind(game)
        .fetch_one(&pool)
        .await?;
    // round(300 * log2(1 + 5)) = 775.
    assert_eq!(score, 775);

    let cleared = sales::replace_list(&pool, "Year", &[]).await?;
    assert_eq!(cleared, 0);
    sales::refresh_sales_scores(&pool).await?;
    let score: i32 = sqlx::query_scalar("SELECT sales_score FROM games WHERE id = $1")
        .bind(game)
        .fetch_one(&pool)
        .await?;
    assert_eq!(score, 0);
    Ok(())
}
