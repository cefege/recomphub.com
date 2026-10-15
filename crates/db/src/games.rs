//! Games: IGDB upserts, media links, listings, hubs, trending and search.

use crate::{DbError, DbResult, media};
use chrono::{DateTime, NaiveDate, Utc};
use sqlx::{PgConnection, PgExecutor};

/// A row of the `games` table. The `search` column is not mapped.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct GameRow {
    pub id: i64,
    pub igdb_id: i64,
    pub slug: String,
    pub name: String,
    pub alt_names: Vec<String>,
    pub summary: Option<String>,
    pub first_release_date: Option<NaiveDate>,
    pub developer: Option<String>,
    pub publisher: Option<String>,
    pub genres: Vec<String>,
    pub popularity: i32,
    pub cover_media_id: Option<i64>,
    pub og_media_id: Option<i64>,
    pub igdb_synced_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub port_count: i32,
    pub decomp_count: i32,
    pub targets: Vec<String>,
    pub best_playability: Option<String>,
    pub want_count: i32,
}

/// A game as returned by IGDB, already mapped to our fields.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct IgdbGame {
    pub igdb_id: i64,
    pub name: String,
    pub slug: String,
    pub alt_names: Vec<String>,
    pub summary: Option<String>,
    pub first_release_date: Option<NaiveDate>,
    pub developer: Option<String>,
    pub publisher: Option<String>,
    pub genres: Vec<String>,
    pub popularity: i32,
    /// IGDB platform ids (`platforms.igdb_id`).
    pub platform_igdb_ids: Vec<i32>,
    pub cover_url: Option<String>,
    pub og_url: Option<String>,
    /// Screenshot URLs. Only the first 6 are stored.
    pub screenshot_urls: Vec<String>,
}

/// Inserts or updates a game by `igdb_id`. Keeps the existing slug on update
/// and rebuilds the search vector.
pub async fn upsert_igdb(conn: &mut PgConnection, g: &IgdbGame) -> DbResult<i64> {
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO games (igdb_id, slug, name, alt_names, summary, first_release_date,
                            developer, publisher, genres, popularity, igdb_synced_at, search)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, now(),
                 to_tsvector('simple', $3::text || ' ' || array_to_string($4::text[], ' ')))
         ON CONFLICT (igdb_id) DO UPDATE SET
            name = EXCLUDED.name,
            alt_names = EXCLUDED.alt_names,
            summary = EXCLUDED.summary,
            first_release_date = EXCLUDED.first_release_date,
            developer = EXCLUDED.developer,
            publisher = EXCLUDED.publisher,
            genres = EXCLUDED.genres,
            popularity = EXCLUDED.popularity,
            igdb_synced_at = now(),
            updated_at = now(),
            search = EXCLUDED.search
         RETURNING id",
    )
    .bind(g.igdb_id)
    .bind(g.slug.clone())
    .bind(g.name.clone())
    .bind(g.alt_names.clone())
    .bind(g.summary.clone())
    .bind(g.first_release_date)
    .bind(g.developer.clone())
    .bind(g.publisher.clone())
    .bind(g.genres.clone())
    .bind(g.popularity)
    .fetch_one(&mut *conn)
    .await?;
    Ok(id)
}

/// Replaces the game's platform links. Ids that have no `platforms` row are skipped.
pub async fn set_platforms(
    conn: &mut PgConnection,
    game_id: i64,
    igdb_ids: Vec<i32>,
) -> DbResult<()> {
    sqlx::query("DELETE FROM game_platforms WHERE game_id = $1")
        .bind(game_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "INSERT INTO game_platforms (game_id, platform_id)
         SELECT $1, p.id FROM platforms p WHERE p.igdb_id = ANY($2::int[])
         ON CONFLICT DO NOTHING",
    )
    .bind(game_id)
    .bind(igdb_ids)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Sets the cover, OG and screenshot links. Screenshot position is its index in `screenshots`.
pub async fn set_media(
    conn: &mut PgConnection,
    game_id: i64,
    cover: Option<i64>,
    og: Option<i64>,
    screenshots: Vec<i64>,
) -> DbResult<()> {
    sqlx::query("UPDATE games SET cover_media_id = $2, og_media_id = $3 WHERE id = $1")
        .bind(game_id)
        .bind(cover)
        .bind(og)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM game_media WHERE game_id = $1")
        .bind(game_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "INSERT INTO game_media (game_id, media_id, position)
         SELECT $1, m.media_id, (m.ord - 1)::int
         FROM unnest($2::bigint[]) WITH ORDINALITY AS m(media_id, ord)",
    )
    .bind(game_id)
    .bind(screenshots)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Full IGDB sync for one game: upsert, platforms, queued media, and media links.
/// Returns the game id.
pub async fn sync_igdb(conn: &mut PgConnection, g: &IgdbGame) -> DbResult<i64> {
    let id = upsert_igdb(&mut *conn, g).await?;
    set_platforms(&mut *conn, id, g.platform_igdb_ids.clone()).await?;
    let cover = match &g.cover_url {
        Some(url) => Some(media::enqueue(&mut *conn, "cover", url).await?),
        None => None,
    };
    let og = match &g.og_url {
        Some(url) => Some(media::enqueue(&mut *conn, "og", url).await?),
        None => None,
    };
    let mut screenshots = Vec::new();
    for url in g.screenshot_urls.iter().take(6) {
        screenshots.push(media::enqueue(&mut *conn, "screenshot", url).await?);
    }
    set_media(&mut *conn, id, cover, og, screenshots).await?;
    Ok(id)
}

/// Recomputes the rollup columns of a game.
pub async fn refresh_rollup(ex: impl PgExecutor<'_>, game_id: i64) -> DbResult<()> {
    sqlx::query("SELECT refresh_game_rollup($1::bigint)")
        .bind(game_id)
        .execute(ex)
        .await?;
    Ok(())
}

/// A game by slug, if it exists.
pub async fn get_by_slug(ex: impl PgExecutor<'_>, slug: &str) -> DbResult<Option<GameRow>> {
    let row = sqlx::query_as::<_, GameRow>("SELECT * FROM games WHERE slug = $1")
        .bind(slug)
        .fetch_optional(ex)
        .await?;
    Ok(row)
}

/// A game by id. Returns `NotFound` when missing.
pub async fn get(ex: impl PgExecutor<'_>, id: i64) -> DbResult<GameRow> {
    sqlx::query_as::<_, GameRow>("SELECT * FROM games WHERE id = $1")
        .bind(id)
        .fetch_optional(ex)
        .await?
        .ok_or_else(|| DbError::NotFound(format!("game {id}")))
}

/// Sort order for game listings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GameSort {
    #[default]
    Popularity,
    Name,
    Ports,
    Wanted,
}

impl GameSort {
    /// Parses the `sort` query value. Unknown values fall back to popularity.
    pub fn from_query(s: &str) -> Self {
        match s {
            "name" => GameSort::Name,
            "ports" => GameSort::Ports,
            "wanted" => GameSort::Wanted,
            _ => GameSort::Popularity,
        }
    }

    fn order_by(self) -> &'static str {
        match self {
            GameSort::Popularity => "g.popularity DESC, g.id",
            GameSort::Name => "g.name, g.id",
            GameSort::Ports => "g.port_count DESC, g.popularity DESC, g.id",
            GameSort::Wanted => "g.want_count DESC, g.popularity DESC, g.id",
        }
    }
}

/// Filters for `list` and `count`.
#[derive(Debug, Clone, Default)]
pub struct GameFilter {
    /// Original platform (`platforms.id`).
    pub platform_id: Option<i32>,
    /// Target that a published port runs on (a `TargetPlatform` value).
    pub target: Option<String>,
    /// `Some(true)` for games with a port, `Some(false)` for games without.
    pub ported: Option<bool>,
    pub sort: GameSort,
}

const FILTER_SQL: &str = "($1::int IS NULL OR EXISTS (
        SELECT 1 FROM game_platforms gp WHERE gp.game_id = g.id AND gp.platform_id = $1::int))
    AND ($2::text IS NULL OR $2::text = ANY(g.targets))
    AND ($3::bool IS NULL OR ((g.port_count > 0) = $3::bool))";

/// One page of games matching `filter`.
pub async fn list(
    ex: impl PgExecutor<'_>,
    filter: &GameFilter,
    limit: i64,
    offset: i64,
) -> DbResult<Vec<GameRow>> {
    let sql = format!(
        "SELECT g.* FROM games g WHERE {FILTER_SQL} ORDER BY {} LIMIT $4 OFFSET $5",
        filter.sort.order_by()
    );
    let rows = sqlx::query_as::<_, GameRow>(&sql)
        .bind(filter.platform_id)
        .bind(filter.target.clone())
        .bind(filter.ported)
        .bind(limit)
        .bind(offset)
        .fetch_all(ex)
        .await?;
    Ok(rows)
}

/// Number of games matching `filter`.
pub async fn count(ex: impl PgExecutor<'_>, filter: &GameFilter) -> DbResult<i64> {
    let sql = format!("SELECT count(*) FROM games g WHERE {FILTER_SQL}");
    let n = sqlx::query_scalar::<_, i64>(&sql)
        .bind(filter.platform_id)
        .bind(filter.target.clone())
        .bind(filter.ported)
        .fetch_one(ex)
        .await?;
    Ok(n)
}

/// Games with no published port, ordered by want count and popularity.
pub async fn wanted(ex: impl PgExecutor<'_>, limit: i64, offset: i64) -> DbResult<Vec<GameRow>> {
    let rows = sqlx::query_as::<_, GameRow>(
        "SELECT * FROM games WHERE port_count = 0
         ORDER BY want_count DESC, popularity DESC, id LIMIT $1 OFFSET $2",
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(ex)
    .await?;
    Ok(rows)
}

/// Number of games with no published port.
pub async fn count_wanted(ex: impl PgExecutor<'_>) -> DbResult<i64> {
    let n = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM games WHERE port_count = 0")
        .fetch_one(ex)
        .await?;
    Ok(n)
}

/// Games on an original platform. Ported games come first, by the 7-day score,
/// then popularity, then wanted games.
pub async fn by_platform(
    ex: impl PgExecutor<'_>,
    platform_id: i32,
    limit: i64,
    offset: i64,
) -> DbResult<Vec<GameRow>> {
    let rows = sqlx::query_as::<_, GameRow>(
        "SELECT g.* FROM games g
         JOIN game_platforms gp ON gp.game_id = g.id AND gp.platform_id = $1
         LEFT JOIN game_score_ranking r ON r.game_id = g.id AND r.window_days = 7
         ORDER BY (g.port_count > 0) DESC, COALESCE(r.score, 0) DESC,
                  g.popularity DESC, g.want_count DESC, g.id
         LIMIT $2 OFFSET $3",
    )
    .bind(platform_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(ex)
    .await?;
    Ok(rows)
}

/// Number of games on an original platform.
pub async fn count_by_platform(ex: impl PgExecutor<'_>, platform_id: i32) -> DbResult<i64> {
    let n =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM game_platforms WHERE platform_id = $1")
            .bind(platform_id)
            .fetch_one(ex)
            .await?;
    Ok(n)
}

/// Up to `limit` ported games on the same original platform, most popular first.
pub async fn related(
    ex: impl PgExecutor<'_>,
    platform_id: i32,
    exclude_id: i64,
    limit: i64,
) -> DbResult<Vec<GameRow>> {
    let rows = sqlx::query_as::<_, GameRow>(
        "SELECT g.* FROM games g
         JOIN game_platforms gp ON gp.game_id = g.id AND gp.platform_id = $1
         WHERE g.port_count > 0 AND g.id <> $2
         ORDER BY g.popularity DESC, g.id LIMIT $3",
    )
    .bind(platform_id)
    .bind(exclude_id)
    .bind(limit)
    .fetch_all(ex)
    .await?;
    Ok(rows)
}

/// A game with its score for one ranking window.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RankedGame {
    #[sqlx(flatten)]
    pub game: GameRow,
    pub hits: i64,
    pub viewer_minutes: i64,
    pub score: f64,
    pub rank: i64,
    pub prev_rank: Option<i64>,
}

/// Ranked games for a window (1, 7, 30 or 365 days), ordered by rank.
pub async fn trending(
    ex: impl PgExecutor<'_>,
    window_days: i32,
    limit: i64,
    offset: i64,
) -> DbResult<Vec<RankedGame>> {
    let rows = sqlx::query_as::<_, RankedGame>(
        "SELECT g.*,
                r.hits::int8 AS hits,
                r.viewer_minutes::int8 AS viewer_minutes,
                r.score::float8 AS score,
                r.rank::int8 AS rank,
                r.prev_rank::int8 AS prev_rank
         FROM game_score_ranking r
         JOIN games g ON g.id = r.game_id
         WHERE r.window_days = $1
         ORDER BY r.rank, g.popularity DESC, g.id
         LIMIT $2 OFFSET $3",
    )
    .bind(window_days)
    .bind(limit)
    .bind(offset)
    .fetch_all(ex)
    .await?;
    Ok(rows)
}

/// Number of games with a score in a window.
pub async fn ranking_count(ex: impl PgExecutor<'_>, window_days: i32) -> DbResult<i64> {
    let n = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM game_score_ranking WHERE window_days = $1",
    )
    .bind(window_days)
    .fetch_one(ex)
    .await?;
    Ok(n)
}

/// Full-text or fuzzy-name matches, best first. At most `limit` rows.
pub async fn search(ex: impl PgExecutor<'_>, q: &str, limit: i64) -> DbResult<Vec<GameRow>> {
    let rows = sqlx::query_as::<_, GameRow>(
        "SELECT g.* FROM games g
         WHERE g.search @@ websearch_to_tsquery('simple', $1)
            OR similarity(g.name, $1) > 0.3
         ORDER BY similarity(g.name, $1) DESC, g.popularity DESC, g.id
         LIMIT $2",
    )
    .bind(q)
    .bind(limit)
    .fetch_all(ex)
    .await?;
    Ok(rows)
}

/// A row of the `platforms` table.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PlatformRow {
    pub id: i32,
    pub igdb_id: i32,
    pub slug: Option<String>,
    pub name: String,
    pub abbreviation: Option<String>,
    pub family: Option<String>,
    pub generation: Option<i32>,
}

/// Platform fields from IGDB, used to upsert `platforms`.
#[derive(Debug, Clone)]
pub struct PlatformUpsert {
    pub igdb_id: i32,
    pub slug: Option<String>,
    pub name: String,
    pub abbreviation: Option<String>,
    pub family: Option<String>,
    pub generation: Option<i32>,
}

/// A platform with how many games it has and how many of them have a port.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PlatformHub {
    #[sqlx(flatten)]
    pub platform: PlatformRow,
    pub game_count: i64,
    pub ported_count: i64,
}

/// Inserts or updates a platform by `igdb_id`. Returns its id.
pub async fn upsert_platform(ex: impl PgExecutor<'_>, p: &PlatformUpsert) -> DbResult<i32> {
    let id = sqlx::query_scalar::<_, i32>(
        "INSERT INTO platforms (igdb_id, slug, name, abbreviation, family, generation)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (igdb_id) DO UPDATE SET
            slug = EXCLUDED.slug, name = EXCLUDED.name, abbreviation = EXCLUDED.abbreviation,
            family = EXCLUDED.family, generation = EXCLUDED.generation
         RETURNING id",
    )
    .bind(p.igdb_id)
    .bind(p.slug.clone())
    .bind(p.name.clone())
    .bind(p.abbreviation.clone())
    .bind(p.family.clone())
    .bind(p.generation)
    .fetch_one(ex)
    .await?;
    Ok(id)
}

/// A platform by slug, if it exists.
pub async fn platform_by_slug(
    ex: impl PgExecutor<'_>,
    slug: &str,
) -> DbResult<Option<PlatformRow>> {
    let row = sqlx::query_as::<_, PlatformRow>("SELECT * FROM platforms WHERE slug = $1")
        .bind(slug)
        .fetch_optional(ex)
        .await?;
    Ok(row)
}

/// A platform by IGDB id, if it exists.
pub async fn platform_by_igdb_id(
    ex: impl PgExecutor<'_>,
    igdb_id: i32,
) -> DbResult<Option<PlatformRow>> {
    let row = sqlx::query_as::<_, PlatformRow>("SELECT * FROM platforms WHERE igdb_id = $1")
        .bind(igdb_id)
        .fetch_optional(ex)
        .await?;
    Ok(row)
}

/// IGDB ids of all platforms already stored.
pub async fn known_igdb_platform_ids(ex: impl PgExecutor<'_>) -> DbResult<Vec<i32>> {
    let ids = sqlx::query_scalar::<_, i32>("SELECT igdb_id FROM platforms ORDER BY igdb_id")
        .fetch_all(ex)
        .await?;
    Ok(ids)
}

/// Platforms with their game counts. With `only_ported`, keeps platforms with at least one ported game.
pub async fn platform_hubs(
    ex: impl PgExecutor<'_>,
    only_ported: bool,
) -> DbResult<Vec<PlatformHub>> {
    let rows = sqlx::query_as::<_, PlatformHub>(
        "SELECT p.id, p.igdb_id, p.slug, p.name, p.abbreviation, p.family, p.generation,
                count(g.id)::int8 AS game_count,
                (count(g.id) FILTER (WHERE g.port_count > 0))::int8 AS ported_count
         FROM platforms p
         JOIN game_platforms gp ON gp.platform_id = p.id
         JOIN games g ON g.id = gp.game_id
         GROUP BY p.id
         HAVING NOT $1::bool OR count(g.id) FILTER (WHERE g.port_count > 0) > 0
         ORDER BY ported_count DESC, game_count DESC, p.name",
    )
    .bind(only_ported)
    .fetch_all(ex)
    .await?;
    Ok(rows)
}

/// Original platforms of a game.
pub async fn platforms_for_game(
    ex: impl PgExecutor<'_>,
    game_id: i64,
) -> DbResult<Vec<PlatformRow>> {
    let rows = sqlx::query_as::<_, PlatformRow>(
        "SELECT p.* FROM platforms p
         JOIN game_platforms gp ON gp.platform_id = p.id
         WHERE gp.game_id = $1 ORDER BY p.name",
    )
    .bind(game_id)
    .fetch_all(ex)
    .await?;
    Ok(rows)
}

/// Media ids linked to a game as screenshots, in position order.
pub async fn screenshot_media_ids(ex: impl PgExecutor<'_>, game_id: i64) -> DbResult<Vec<i64>> {
    let ids = sqlx::query_scalar::<_, i64>(
        "SELECT media_id FROM game_media WHERE game_id = $1 ORDER BY position",
    )
    .bind(game_id)
    .fetch_all(ex)
    .await?;
    Ok(ids)
}

/// Site-wide totals shown on the home page.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SiteStats {
    pub games_with_port: i64,
    pub ports: i64,
    pub platforms_with_ports: i64,
}

/// Site-wide totals: games with a port, published non-decompilation ports, and platforms with a ported game.
pub async fn stats(ex: impl PgExecutor<'_>) -> DbResult<SiteStats> {
    let s = sqlx::query_as::<_, SiteStats>(
        "SELECT
            (SELECT count(*) FROM games WHERE port_count > 0)::int8 AS games_with_port,
            (SELECT count(*) FROM ports WHERE status = 'published' AND kind <> 'decompilation')::int8 AS ports,
            (SELECT count(DISTINCT gp.platform_id) FROM game_platforms gp
                JOIN games g ON g.id = gp.game_id WHERE g.port_count > 0)::int8 AS platforms_with_ports",
    )
    .fetch_one(ex)
    .await?;
    Ok(s)
}
