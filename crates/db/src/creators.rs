//! Creator hub queries: what to recompile next, ports missing major platforms, and ports
//! that need gameplay videos. Each list is a prioritised to-do list with stable pagination.

use recomphub_shared::dto::SalesSignal;

use crate::games::GameRow;
use crate::{DbResult, PgExecutor};

/// Ranking importance for a game, as a SQL expression over the `games g` alias:
/// IGDB popularity plus the best-seller sales score (see migration 0003).
pub const IMPORTANCE_SQL: &str = "g.importance";

/// Major platforms a port should reach (shared with the parity stats). A port missing any of
/// these shows up in the "missing platforms" list.
pub use recomphub_shared::MAJOR_TARGETS;

/// Ports with fewer gameplay videos than this count as needing videos.
pub const FEW_VIDEOS: i64 = 2;

/// Whether `target` is one of the major targets checked by the missing-platforms list.
pub fn is_major_target(target: &str) -> bool {
    MAJOR_TARGETS.contains(&target)
}

/// The strongest best-seller entry per game: the one with the most units sold, then the best
/// rank. Joined as `best` with at most one row per game, so a game without signals keeps its row.
const BEST_SALE_JOIN: &str = "LEFT JOIN LATERAL (
            SELECT s.units_sold_millions AS units, s.scope, s.sales_year, s.rank,
                   COALESCE(pl.abbreviation, pl.name) AS platform
            FROM sales_signals s
            LEFT JOIN platforms pl ON pl.slug = s.platform_slug
            WHERE s.game_id = g.id
            ORDER BY s.units_sold_millions DESC, s.rank NULLS LAST, s.id
            LIMIT 1
        ) best ON true";

/// Columns from [`BEST_SALE_JOIN`], named to match [`SaleColumns`].
const BEST_SALE_COLUMNS: &str = "best.units AS best_units, best.scope AS best_scope,
            best.platform AS best_platform, best.sales_year AS best_year, best.rank AS best_rank";

/// The best-seller columns of a game. All `None` when the game has no sales signal.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SaleColumns {
    pub best_units: Option<f64>,
    pub best_scope: Option<String>,
    pub best_platform: Option<String>,
    pub best_year: Option<i32>,
    pub best_rank: Option<i32>,
}

impl SaleColumns {
    /// The best-seller entry as a DTO, or `None` when the game has no sales signal.
    pub fn signal(&self) -> Option<SalesSignal> {
        Some(SalesSignal {
            units_sold_millions: self.best_units?,
            scope: self.best_scope.clone()?,
            platform: self.best_platform.clone(),
            sales_year: self.best_year,
            rank: self.best_rank,
        })
    }
}

/// A game with no port yet, ranked by want count then importance.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RecompileRow {
    #[sqlx(flatten)]
    pub game: GameRow,
    pub original_platforms: Vec<String>,
    #[sqlx(flatten)]
    pub sale: SaleColumns,
}

/// A published port that lacks one or more major platforms.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PortGapRow {
    pub port_id: i64,
    pub port_slug: String,
    pub port_name: String,
    pub repo_url: String,
    pub github_stars: i32,
    pub game_slug: String,
    pub game_name: String,
    pub targets: Vec<String>,
    pub missing: Vec<String>,
    /// IGDB rating count (`games.popularity`).
    pub popularity: i32,
    #[sqlx(flatten)]
    pub sale: SaleColumns,
}

/// A published port with few gameplay videos, overall or for one target.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct VideoGapRow {
    pub port_id: i64,
    pub port_slug: String,
    pub port_name: String,
    pub repo_url: String,
    pub github_stars: i32,
    pub game_slug: String,
    pub game_name: String,
    pub targets: Vec<String>,
    pub video_count: i64,
    /// IGDB rating count (`games.popularity`).
    pub popularity: i32,
    #[sqlx(flatten)]
    pub sale: SaleColumns,
}

fn majors() -> Vec<String> {
    MAJOR_TARGETS.iter().map(|t| t.to_string()).collect()
}

/// Games with `port_count = 0`, most wanted first, importance breaks ties.
pub async fn recompile_page(
    ex: impl PgExecutor<'_>,
    limit: i64,
    offset: i64,
) -> DbResult<Vec<RecompileRow>> {
    let sql = format!(
        "SELECT g.*,
                ARRAY(SELECT pl.name FROM game_platforms gp
                      JOIN platforms pl ON pl.id = gp.platform_id
                      WHERE gp.game_id = g.id ORDER BY pl.name) AS original_platforms,
                {BEST_SALE_COLUMNS}
         FROM games g
         {BEST_SALE_JOIN}
         WHERE g.port_count = 0
         ORDER BY g.want_count DESC, ({IMPORTANCE_SQL}) DESC, g.id
         LIMIT $1 OFFSET $2"
    );
    Ok(sqlx::query_as::<_, RecompileRow>(&sql)
        .bind(limit)
        .bind(offset)
        .fetch_all(ex)
        .await?)
}

/// Number of games with no port.
pub async fn recompile_count(ex: impl PgExecutor<'_>) -> DbResult<i64> {
    Ok(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM games WHERE port_count = 0")
            .fetch_one(ex)
            .await?,
    )
}

/// Published ports missing at least one major target. `missing_filter` keeps only ports that
/// lack that one target. Ranked by importance times the number of missing targets.
pub async fn port_gaps_page(
    ex: impl PgExecutor<'_>,
    missing_filter: Option<&str>,
    limit: i64,
    offset: i64,
) -> DbResult<Vec<PortGapRow>> {
    let sql = format!(
        "WITH base AS (
            SELECT p.id AS port_id, p.slug AS port_slug, p.name AS port_name, p.repo_url,
                   p.github_stars, p.star_count, g.slug AS game_slug, g.name AS game_name,
                   g.popularity, {BEST_SALE_COLUMNS},
                   ({IMPORTANCE_SQL})::float8 AS importance,
                   port_platforms(p) AS targets,
                   ARRAY(SELECT platform_target(pl.slug) FROM game_platforms gp JOIN platforms pl ON pl.id = gp.platform_id
                         WHERE gp.game_id = g.id AND platform_target(pl.slug) IS NOT NULL) AS originals
            FROM ports p
            JOIN games g ON g.id = p.game_id
            {BEST_SALE_JOIN}
            WHERE p.status = 'published' AND p.kind <> 'decompilation'
              AND cardinality(port_platforms(p)) > 0
        ), gaps AS (
            SELECT b.*,
                   ARRAY(SELECT m FROM unnest($1::text[]) AS m
                         WHERE NOT (m = ANY(b.targets) OR m = ANY(b.originals))) AS missing
            FROM base b
        )
        SELECT port_id, port_slug, port_name, repo_url, github_stars, game_slug, game_name,
               targets, missing, popularity, best_units, best_scope, best_platform, best_year,
               best_rank
        FROM gaps
        WHERE cardinality(missing) > 0
          AND ($2::text IS NULL OR $2 = ANY(missing))
        ORDER BY importance * cardinality(missing) DESC, star_count DESC, port_id
        LIMIT $3 OFFSET $4"
    );
    Ok(sqlx::query_as::<_, PortGapRow>(&sql)
        .bind(majors())
        .bind(missing_filter)
        .bind(limit)
        .bind(offset)
        .fetch_all(ex)
        .await?)
}

/// Number of ports in [`port_gaps_page`] for the same filter.
pub async fn port_gaps_count(
    ex: impl PgExecutor<'_>,
    missing_filter: Option<&str>,
) -> DbResult<i64> {
    let sql = "WITH base AS (
            SELECT p.id AS port_id, port_platforms(p) AS targets,
                   ARRAY(SELECT platform_target(pl.slug) FROM game_platforms gp JOIN platforms pl ON pl.id = gp.platform_id
                         WHERE gp.game_id = p.game_id AND platform_target(pl.slug) IS NOT NULL) AS originals
            FROM ports p
            WHERE p.status = 'published' AND p.kind <> 'decompilation'
              AND cardinality(port_platforms(p)) > 0
        ), gaps AS (
            SELECT b.port_id,
                   ARRAY(SELECT m FROM unnest($1::text[]) AS m
                         WHERE NOT (m = ANY(b.targets) OR m = ANY(b.originals))) AS missing
            FROM base b
        )
        SELECT count(*) FROM gaps
        WHERE cardinality(missing) > 0
          AND ($2::text IS NULL OR $2 = ANY(missing))";
    Ok(sqlx::query_scalar::<_, i64>(sql)
        .bind(majors())
        .bind(missing_filter)
        .fetch_one(ex)
        .await?)
}

/// Published ports with fewer than [`FEW_VIDEOS`] visible gameplay videos. With `target`, only
/// videos for that target count. A video counts for a port when it is linked to that port, or when
/// it is game-level (`port_id IS NULL`). Ranked by importance, then stars, then latest release.
pub async fn video_gaps_page(
    ex: impl PgExecutor<'_>,
    target: Option<&str>,
    limit: i64,
    offset: i64,
) -> DbResult<Vec<VideoGapRow>> {
    let sql = format!(
        "WITH base AS (
            SELECT p.id AS port_id, p.slug AS port_slug, p.name AS port_name, p.repo_url,
                   p.github_stars, p.star_count, p.latest_release_at, p.game_id,
                   g.slug AS game_slug, g.name AS game_name, g.popularity,
                   {BEST_SALE_COLUMNS},
                   ({IMPORTANCE_SQL})::float8 AS importance,
                   port_platforms(p) AS targets
            FROM ports p
            JOIN games g ON g.id = p.game_id
            {BEST_SALE_JOIN}
            WHERE p.status = 'published' AND p.kind <> 'decompilation'
              AND cardinality(port_platforms(p)) > 0
              AND ($1::text IS NULL OR $1 = ANY(port_platforms(p)))
        ), counted AS (
            SELECT b.*,
                   (SELECT count(*) FROM videos v
                    WHERE v.game_id = b.game_id AND NOT v.hidden
                      AND (v.port_id = b.port_id OR v.port_id IS NULL)
                      AND ($1::text IS NULL OR v.target = $1)) AS video_count
            FROM base b
        )
        SELECT port_id, port_slug, port_name, repo_url, github_stars, game_slug, game_name,
               targets, video_count, popularity, best_units, best_scope, best_platform,
               best_year, best_rank
        FROM counted
        WHERE video_count < {FEW_VIDEOS}
        ORDER BY importance DESC, star_count DESC, latest_release_at DESC NULLS LAST, port_id
        LIMIT $2 OFFSET $3"
    );
    Ok(sqlx::query_as::<_, VideoGapRow>(&sql)
        .bind(target)
        .bind(limit)
        .bind(offset)
        .fetch_all(ex)
        .await?)
}

/// Number of ports in [`video_gaps_page`] for the same target.
pub async fn video_gaps_count(ex: impl PgExecutor<'_>, target: Option<&str>) -> DbResult<i64> {
    let sql = format!(
        "WITH base AS (
            SELECT p.id AS port_id, p.game_id
            FROM ports p
            WHERE p.status = 'published' AND p.kind <> 'decompilation'
              AND cardinality(port_platforms(p)) > 0
              AND ($1::text IS NULL OR $1 = ANY(port_platforms(p)))
        ), counted AS (
            SELECT b.port_id,
                   (SELECT count(*) FROM videos v
                    WHERE v.game_id = b.game_id AND NOT v.hidden
                      AND (v.port_id = b.port_id OR v.port_id IS NULL)
                      AND ($1::text IS NULL OR v.target = $1)) AS video_count
            FROM base b
        )
        SELECT count(*) FROM counted WHERE video_count < {FEW_VIDEOS}"
    );
    Ok(sqlx::query_scalar::<_, i64>(&sql)
        .bind(target)
        .fetch_one(ex)
        .await?)
}

/// Row offset for a 1-based page number. Page numbers below 1 count as page 1.
pub fn offset_for(page: i64, size: i64) -> i64 {
    (page.max(1) - 1) * size
}

/// Number of pages for `total` rows at `size` rows per page. Never less than 1.
pub fn page_count(total: i64, size: i64) -> i64 {
    ((total + size - 1) / size).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_is_zero_on_first_page_and_clamps_below_one() {
        assert_eq!(offset_for(1, 50), 0);
        assert_eq!(offset_for(0, 50), 0);
        assert_eq!(offset_for(3, 50), 100);
    }

    #[test]
    fn page_count_is_at_least_one() {
        assert_eq!(page_count(0, 50), 1);
        assert_eq!(page_count(50, 50), 1);
        assert_eq!(page_count(51, 50), 2);
    }
}
