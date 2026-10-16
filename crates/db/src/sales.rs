//! Best-selling list entries per game (Wikipedia lists), folded into `games.sales_score`.
//!
//! `games.importance` is generated from `popularity + sales_score`, so ordering code
//! only needs `importance`; this module owns the sales half.

use crate::DbResult;
use sqlx::{PgExecutor, PgPool};
use std::collections::HashMap;

/// Which kind of list a signal came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SalesScope {
    /// A list of one platform's games; matched against that platform.
    Platform,
    /// A list not tied to one platform.
    Overall,
    /// A per-year list; `sales_year` holds the year the sales were recorded in.
    Year,
}

impl SalesScope {
    pub fn as_str(self) -> &'static str {
        match self {
            SalesScope::Platform => "platform",
            SalesScope::Overall => "overall",
            SalesScope::Year => "year",
        }
    }
}

/// One matched list entry, ready to store.
#[derive(Debug, Clone)]
pub struct SalesSignal {
    pub game_id: i64,
    pub scope: SalesScope,
    pub platform_slug: Option<String>,
    pub sales_year: Option<i32>,
    pub rank: Option<i32>,
    pub units_sold_millions: f64,
    pub listed_title: String,
    pub source_url: String,
}

/// A game as the sales matcher sees it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CatalogEntry {
    pub id: i64,
    pub name: String,
    pub alt_names: Vec<String>,
    pub release_year: Option<i32>,
    pub importance: i32,
    pub platform_slugs: Vec<String>,
}

/// Replaces every stored signal for `list_title` with `signals`, in one transaction.
/// A game listed more than once for the same list and sales year keeps its larger figure.
/// Returns the number of rows stored.
pub async fn replace_list(
    pool: &PgPool,
    list_title: &str,
    signals: &[SalesSignal],
) -> DbResult<usize> {
    let mut best: HashMap<(i64, Option<i32>), &SalesSignal> = HashMap::new();
    for s in signals {
        best.entry((s.game_id, s.sales_year))
            .and_modify(|cur| {
                if s.units_sold_millions > cur.units_sold_millions {
                    *cur = s;
                }
            })
            .or_insert(s);
    }
    let mut rows: Vec<&SalesSignal> = best.into_values().collect();
    rows.sort_by_key(|s| (s.game_id, s.sales_year));

    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM sales_signals WHERE list_title = $1")
        .bind(list_title)
        .execute(&mut *tx)
        .await?;
    for s in &rows {
        sqlx::query(
            "INSERT INTO sales_signals
                (game_id, list_title, scope, platform_slug, sales_year, rank,
                 units_sold_millions, listed_title, source_url)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(s.game_id)
        .bind(list_title)
        .bind(s.scope.as_str())
        .bind(s.platform_slug.as_deref())
        .bind(s.sales_year)
        .bind(s.rank)
        .bind(s.units_sold_millions)
        .bind(&s.listed_title)
        .bind(&s.source_url)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(rows.len())
}

/// Recomputes `games.sales_score` from every stored signal:
/// `round(300 * log2(1 + best units in millions))`. One million copies scores 300 (about the
/// 90th percentile of IGDB popularity), 10 million about 1040 and 80 million about 1900, so big
/// sellers rise to the top without burying games IGDB players rate heavily. Games without
/// signals are reset to 0. Returns rows changed.
pub async fn refresh_sales_scores(ex: impl PgExecutor<'_>) -> DbResult<u64> {
    let res = sqlx::query(
        "UPDATE games g
         SET sales_score = b.score
         FROM (
             SELECT g2.id,
                    COALESCE(ROUND(300 * LN(1 + MAX(s.units_sold_millions)) / LN(2))::int, 0) AS score
             FROM games g2
             LEFT JOIN sales_signals s ON s.game_id = g2.id
             GROUP BY g2.id
         ) b
         WHERE b.id = g.id AND g.sales_score IS DISTINCT FROM b.score",
    )
    .execute(ex)
    .await?;
    Ok(res.rows_affected())
}

/// Every game with the fields the sales matcher needs, in one query.
pub async fn catalog_index(ex: impl PgExecutor<'_>) -> DbResult<Vec<CatalogEntry>> {
    Ok(sqlx::query_as::<_, CatalogEntry>(
        "SELECT g.id, g.name, g.alt_names,
                EXTRACT(YEAR FROM g.first_release_date)::int AS release_year,
                g.importance,
                ARRAY(SELECT p.slug
                      FROM game_platforms gp
                      JOIN platforms p ON p.id = gp.platform_id
                      WHERE gp.game_id = g.id AND p.slug IS NOT NULL) AS platform_slugs
         FROM games g",
    )
    .fetch_all(ex)
    .await?)
}
