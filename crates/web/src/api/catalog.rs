//! Read-only catalog server functions: games, ports, platforms, rankings, activity,
//! author pages and search. Every endpoint here is public. Viewer-specific data
//! (stars, wants, moderation) is never rendered into server-side HTML.

#[cfg(feature = "server")]
use dioxus::fullstack::HeaderMap;
use dioxus::prelude::*;
use recomphub_shared::dto::{
    AuthorProfile, GameCard, GameDetail, HomeData, Page, PlatformRef, PortCard, PortDetail,
    RankedGame, SearchResults,
};
use recomphub_shared::enums::HitWindow;
use serde::{Deserialize, Serialize};

#[cfg(feature = "server")]
use std::collections::{HashMap, HashSet};
#[cfg(feature = "server")]
use std::str::FromStr;

#[cfg(feature = "server")]
use crate::server::session::{db_error, http_error};
#[cfg(feature = "server")]
use crate::server::state::pool;
#[cfg(feature = "server")]
use chrono::{Datelike, Utc};
#[cfg(feature = "server")]
use dioxus::server::axum::http::StatusCode;
#[cfg(feature = "server")]
use recomphub_db::activity::{self, ActivityRow};
#[cfg(feature = "server")]
use recomphub_db::author_funding;
#[cfg(feature = "server")]
use recomphub_db::game_faqs;
#[cfg(feature = "server")]
use recomphub_db::games::{
    self, GameFilter, GameRow, GameSort, PlatformRow, RankedGame as RankedRow,
};
#[cfg(feature = "server")]
use recomphub_db::hits;
#[cfg(feature = "server")]
use recomphub_db::media::{self, MediaRow};
#[cfg(feature = "server")]
use recomphub_db::port_descriptions;
#[cfg(feature = "server")]
use recomphub_db::ports::{self, AuthorRow, PortRow, PortWithGame};
#[cfg(feature = "server")]
use recomphub_db::stats;
#[cfg(feature = "server")]
use recomphub_db::videos::{self, VideoRow};
#[cfg(feature = "server")]
use recomphub_shared::dto::{
    AiEvidenceDto, AuthorDto, FundingLink, MaintenanceDto, MediaRef, RunsOnTile, SiteStatsDto,
    VideoDto,
};
#[cfg(feature = "server")]
use recomphub_shared::enums::{FundingPlatform, PortKind, TargetPlatform};
#[cfg(feature = "server")]
use recomphub_shared::maintenance::MaintenanceLevel;
#[cfg(feature = "server")]
use recomphub_shared::release::{ReleaseInput, ReleaseState, release_state};

/// Games per page on index, platform and author lists.
#[cfg(feature = "server")]
const PAGE_SIZE: i64 = 50;
/// Trending entries per page; the full ranking is capped at one page.
#[cfg(feature = "server")]
const TREND_PAGE_SIZE: i64 = 100;

/// Query for `/games`. `page` below 1 is treated as 1.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GamesQuery {
    pub platform: Option<String>,
    pub runs_on: Option<String>,
    pub status: Option<String>,
    pub sort: Option<String>,
    pub page: i64,
}

/// Canonical site origin used for canonical URLs and sitemaps.
#[get("/api/site-url")]
pub async fn site_url() -> Result<String, ServerFnError> {
    Ok(crate::server::config::get().site_url.clone())
}

/// Front page: trending top 10 for `window`, plus rails for new, wanted, activity and browse tiles.
#[post("/api/home")]
pub async fn home(window: HitWindow) -> Result<HomeData, ServerFnError> {
    let pool = pool()?;
    let days = window.days();
    let ranking_total = games::ranking_count(pool, days).await.map_err(db_error)?;
    let ranked = games::trending(pool, days, 10, 0).await.map_err(db_error)?;
    let window_has_scores = ranking_total > 0;
    let mut trending = ranked_cards(pool, ranked).await?;
    if trending.len() < 10 {
        let excluded: HashSet<i64> = trending.iter().map(|r| r.game.id).collect();
        let want = 10 - trending.len();
        let filter = GameFilter {
            sort: GameSort::Popularity,
            ..Default::default()
        };
        let candidates = games::list(pool, &filter, (want + excluded.len()) as i64, 0)
            .await
            .map_err(db_error)?;
        let fill: Vec<GameRow> = candidates
            .into_iter()
            .filter(|g| !excluded.contains(&g.id))
            .take(want)
            .collect();
        for card in game_cards(pool, &fill).await? {
            let rank = trending.len() as i64 + 1;
            trending.push(RankedGame {
                game: card,
                rank,
                prev_rank: None,
                score: 0.0,
                hits: 0,
                viewer_minutes: 0,
            });
        }
    }

    let just_released = port_cards(
        pool,
        ports::just_released(pool, 12, 0).await.map_err(db_error)?,
    )
    .await?;
    let wanted = game_cards(pool, &games::wanted(pool, 8, 0).await.map_err(db_error)?).await?;
    let activity =
        activity_items(pool, activity::recent(pool, 15).await.map_err(db_error)?).await?;

    let hubs = games::platform_hubs(pool, true).await.map_err(db_error)?;
    let mut platforms: Vec<PlatformRef> = hubs
        .iter()
        .map(|h| platform_dto(&h.platform, h.game_count, h.ported_count))
        .collect();
    platforms.sort_by_key(|p| std::cmp::Reverse(p.ported_count));

    let runs = ports::runs_on_counts(pool).await.map_err(db_error)?;
    let runs_on: Vec<RunsOnTile> = TargetPlatform::ALL
        .iter()
        .copied()
        .filter(|t| t.is_primary())
        .map(|t| RunsOnTile {
            target: t.slug().to_string(),
            label: t.label().to_string(),
            port_count: runs
                .iter()
                .find(|(k, _)| k.as_str() == t.as_str())
                .map_or(0, |(_, c)| *c),
        })
        .collect();

    let stats = games::stats(pool).await.map_err(db_error)?;
    let live = crate::api::live::live_presence();
    Ok(HomeData {
        trending,
        window_has_scores,
        just_released,
        activity,
        wanted,
        platforms,
        runs_on,
        stats: SiteStatsDto {
            games_with_port: stats.games_with_port,
            ports: stats.ports,
            platforms_with_ports: stats.platforms_with_ports,
        },
        live_top: live.top,
        online: live.online,
    })
}

/// Full trending ranking for one window, one page of `TREND_PAGE_SIZE`.
#[post("/api/trending")]
pub async fn trending(win: HitWindow, page: i64) -> Result<Page<RankedGame>, ServerFnError> {
    let pool = pool()?;
    let days = win.days();
    let total = games::ranking_count(pool, days)
        .await
        .map_err(db_error)?
        .min(TREND_PAGE_SIZE);
    let (page, pages, offset) = paginate(page, total, TREND_PAGE_SIZE)?;
    let rows = games::trending(pool, days, TREND_PAGE_SIZE, offset)
        .await
        .map_err(db_error)?;
    Ok(Page {
        items: ranked_cards(pool, rows).await?,
        page,
        pages,
        total,
    })
}

/// Games index with platform, runs-on, status and sort filters.
#[post("/api/games")]
pub async fn games_index(query: GamesQuery) -> Result<Page<GameCard>, ServerFnError> {
    let pool = pool()?;
    let platform_id = match non_empty(&query.platform) {
        Some(slug) => Some(platform_row(pool, slug).await?.id),
        None => None,
    };
    let target = match non_empty(&query.runs_on) {
        Some(slug) => Some(
            TargetPlatform::from_slug(slug)
                .ok_or_else(|| not_found("target"))?
                .as_str()
                .to_string(),
        ),
        None => None,
    };
    let ported = match non_empty(&query.status) {
        None => None,
        Some("ported") => Some(true),
        Some("wanted") => Some(false),
        Some(other) => return Err(bad_request(format!("unknown status: {other}"))),
    };
    let sort = GameSort::from_query(non_empty(&query.sort).unwrap_or(""));
    let filter = GameFilter {
        platform_id,
        target,
        ported,
        sort,
    };
    let total = games::count(pool, &filter).await.map_err(db_error)?;
    let (page, pages, offset) = paginate(query.page, total, PAGE_SIZE)?;
    let rows = games::list(pool, &filter, PAGE_SIZE, offset)
        .await
        .map_err(db_error)?;
    Ok(Page {
        items: game_cards(pool, &rows).await?,
        page,
        pages,
        total,
    })
}

/// Public metadata for up to 50 existing catalog games. Missing ids are omitted and the submitted
/// de-duplicated order is retained so private library state never enters public catalog DTOs.
#[post("/api/v1/games/cards")]
pub async fn game_cards_by_ids(game_ids: Vec<i64>) -> Result<Vec<GameCard>, ServerFnError> {
    let mut ids = Vec::with_capacity(game_ids.len().min(50));
    for game_id in game_ids {
        if game_id <= 0 {
            return Err(bad_request("game id must be positive"));
        }
        if !ids.contains(&game_id) {
            ids.push(game_id);
            if ids.len() > 50 {
                return Err(bad_request("at most 50 game ids are allowed"));
            }
        }
    }

    let pool = pool()?;
    let mut rows: HashMap<i64, GameRow> = games::by_ids(pool, &ids)
        .await
        .map_err(db_error)?
        .into_iter()
        .map(|game| (game.id, game))
        .collect();
    let ordered: Vec<GameRow> = ids.into_iter().filter_map(|id| rows.remove(&id)).collect();
    game_cards(pool, &ordered).await
}

/// One game by slug, with its ports, decompilations, videos, screenshots and related games.
/// Unknown slug returns 404.
#[post("/api/game")]
pub async fn game_detail(lookup: String) -> Result<GameDetail, ServerFnError> {
    let pool = pool()?;
    let game = games::get_by_slug(pool, lookup.trim())
        .await
        .map_err(db_error)?
        .ok_or_else(|| not_found("game"))?;
    let card = game_cards(pool, std::slice::from_ref(&game))
        .await?
        .remove(0);

    let screenshot_ids = games::screenshot_media_ids(pool, game.id)
        .await
        .map_err(db_error)?;
    let mut media_ids = screenshot_ids.clone();
    media_ids.extend(game.og_media_id);
    let media = media_map(pool, media_ids).await?;
    let screenshots: Vec<MediaRef> = screenshot_ids
        .iter()
        .filter_map(|id| media.get(id).cloned())
        .collect();
    let og = game.og_media_id.and_then(|id| media.get(&id).cloned());

    let platform_rows = games::platforms_for_game(pool, game.id)
        .await
        .map_err(db_error)?;
    let counts = platform_counts(pool).await?;
    let platforms: Vec<PlatformRef> = platform_rows
        .iter()
        .map(|p| platform_dto_counted(p, &counts))
        .collect();

    let ported = ports::list_for_game(pool, game.id)
        .await
        .map_err(db_error)?;
    let total_github_stars: i64 = ported.iter().map(|p| i64::from(p.github_stars)).sum();
    let ported_cards = port_cards(pool, with_game(ported, &game)).await?;

    let decomps = ports::list_decompilations(pool, game.id)
        .await
        .map_err(db_error)?;
    let decompilations = port_cards(pool, with_game(decomps, &game)).await?;

    let related_rows = match platform_rows.first() {
        Some(p) => games::related(pool, p.id, game.id, 12)
            .await
            .map_err(db_error)?,
        None => Vec::new(),
    };
    let related = game_cards(pool, &related_rows).await?;

    let video_rows = videos::list_for_game(pool, game.id, false)
        .await
        .map_err(db_error)?;
    let videos = video_dtos(pool, video_rows).await?;

    let has_pc_port = card
        .targets
        .iter()
        .any(|t| matches!(t.as_str(), "windows" | "linux" | "macos"));
    let platform_pairs: Vec<(Option<&str>, &str)> = platform_rows
        .iter()
        .map(|p| (p.slug.as_deref(), p.name.as_str()))
        .collect();
    let buy_offers = recomphub_shared::shop::game_offers(
        crate::server::config::get().amazon_tag.as_deref(),
        &game.name,
        &platform_pairs,
        has_pc_port,
    );

    Ok(GameDetail {
        game: card,
        summary: game.summary.clone(),
        alt_names: game.alt_names.clone(),
        developer: game.developer.clone(),
        publisher: game.publisher.clone(),
        genres: game.genres.clone(),
        first_release_date: game.first_release_date,
        platforms,
        screenshots,
        og,
        ports: ported_cards,
        decompilations,
        videos,
        related,
        total_github_stars,
        live_viewers: crate::api::live::live_viewers(game.id),
        faq: game_faqs::items(pool, game.id).await.map_err(db_error)?,
        buy_offers,
    })
}

/// Published ports of one game, as cards. Unknown game returns 404.
#[post("/api/game-ports")]
pub async fn game_ports(game_id: i64) -> Result<Vec<PortCard>, ServerFnError> {
    let pool = pool()?;
    let game = games::get(pool, game_id).await.map_err(db_error)?;
    let rows = ports::list_for_game(pool, game.id)
        .await
        .map_err(db_error)?;
    port_cards(pool, with_game(rows, &game)).await
}

/// Counts one page view of a game for the popularity score. Bots and
/// requests with no visitor key are accepted and not counted.
#[post("/api/hit", headers: HeaderMap)]
pub async fn record_hit(game_id: i64) -> Result<(), ServerFnError> {
    let pool = pool()?;
    games::get(pool, game_id).await.map_err(db_error)?;
    let Some(visitor) = crate::server::live::visitor_key(&headers).await? else {
        return Ok(());
    };
    let day = Utc::now().date_naive();
    hits::record_hit(pool, game_id, &visitor, day)
        .await
        .map_err(db_error)?;
    Ok(())
}

/// Author profile and their published ports. An author with no published ports
/// is 404, so the page never renders an empty profile.
#[post("/api/author")]
pub async fn author(login: String) -> Result<AuthorProfile, ServerFnError> {
    let pool = pool()?;
    let rows = ports::by_author(pool, &login).await.map_err(db_error)?;
    if rows.is_empty() {
        return Err(not_found("author"));
    }
    let mut name = login.clone();
    let mut avatar_id = None;
    'find: for row in &rows {
        let authors = ports::authors(pool, row.port.id).await.map_err(db_error)?;
        for a in &authors {
            let matches = a
                .github_login
                .as_deref()
                .is_some_and(|l| l.eq_ignore_ascii_case(&login));
            if matches {
                name = a.name.clone();
                avatar_id = a.avatar_media_id;
                break 'find;
            }
        }
    }
    let avatar = match avatar_id {
        Some(id) => media::get(pool, id)
            .await
            .map_err(db_error)?
            .as_ref()
            .map(media_ref),
        None => None,
    };
    let key = login.to_ascii_lowercase();
    let funding = funding_map(pool, std::slice::from_ref(&key))
        .await?
        .remove(&key)
        .unwrap_or_default();
    let achievements = stats::author_achievements(pool, &login)
        .await
        .map_err(db_error)?;
    Ok(AuthorProfile {
        login,
        name,
        avatar,
        ports: port_cards(pool, rows).await?,
        funding,
        achievements,
    })
}

/// All platforms with game and ported-game counts.
#[get("/api/platforms")]
pub async fn platforms() -> Result<Vec<PlatformRef>, ServerFnError> {
    let pool = pool()?;
    let hubs = games::platform_hubs(pool, false).await.map_err(db_error)?;
    Ok(hubs
        .iter()
        .map(|h| platform_dto(&h.platform, h.game_count, h.ported_count))
        .collect())
}

/// One platform by slug. Unknown slug returns 404.
#[post("/api/platform")]
pub async fn platform_ref(slug: String) -> Result<PlatformRef, ServerFnError> {
    let pool = pool()?;
    let row = platform_row(pool, &slug).await?;
    let counts = platform_counts(pool).await?;
    Ok(platform_dto_counted(&row, &counts))
}

/// Games on one platform, ported games first by ranking, then popularity, then wants.
#[post("/api/platform-games")]
pub async fn platform_games(slug: String, page: i64) -> Result<Page<GameCard>, ServerFnError> {
    let pool = pool()?;
    let platform = platform_row(pool, &slug).await?;
    let total = games::count_by_platform(pool, platform.id)
        .await
        .map_err(db_error)?;
    let (page, pages, offset) = paginate(page, total, PAGE_SIZE)?;
    let rows = games::by_platform(pool, platform.id, PAGE_SIZE, offset)
        .await
        .map_err(db_error)?;
    Ok(Page {
        items: game_cards(pool, &rows).await?,
        page,
        pages,
        total,
    })
}

/// Ports of one platform that run on one target. Unknown platform or target returns 404.
#[post("/api/platform-target")]
pub async fn platform_target(
    slug: String,
    target: String,
    page: i64,
) -> Result<Page<PortCard>, ServerFnError> {
    let pool = pool()?;
    let platform = platform_row(pool, &slug).await?;
    let target = TargetPlatform::from_slug(&target).ok_or_else(|| not_found("target"))?;
    let total = ports::count_by_target(pool, target.as_str(), Some(platform.id))
        .await
        .map_err(db_error)?;
    let (page, pages, offset) = paginate(page, total, PAGE_SIZE)?;
    let rows = ports::list_by_target(pool, target.as_str(), Some(platform.id), PAGE_SIZE, offset)
        .await
        .map_err(db_error)?;
    Ok(Page {
        items: port_cards(pool, rows).await?,
        page,
        pages,
        total,
    })
}

/// Every published port that runs on one target. Unknown target returns 404.
#[post("/api/runs-on")]
pub async fn runs_on(target: String, page: i64) -> Result<Page<PortCard>, ServerFnError> {
    let pool = pool()?;
    let target = TargetPlatform::from_slug(&target).ok_or_else(|| not_found("target"))?;
    let total = ports::count_by_target(pool, target.as_str(), None)
        .await
        .map_err(db_error)?;
    let (page, pages, offset) = paginate(page, total, PAGE_SIZE)?;
    let rows = ports::list_by_target(pool, target.as_str(), None, PAGE_SIZE, offset)
        .await
        .map_err(db_error)?;
    Ok(Page {
        items: port_cards(pool, rows).await?,
        page,
        pages,
        total,
    })
}

/// Games with no published port, by wants then popularity.
#[post("/api/wanted")]
pub async fn wanted(page: i64) -> Result<Page<GameCard>, ServerFnError> {
    let pool = pool()?;
    let total = games::count_wanted(pool).await.map_err(db_error)?;
    let (page, pages, offset) = paginate(page, total, PAGE_SIZE)?;
    let rows = games::wanted(pool, PAGE_SIZE, offset)
        .await
        .map_err(db_error)?;
    Ok(Page {
        items: game_cards(pool, &rows).await?,
        page,
        pages,
        total,
    })
}

/// Recently added or released ports, newest first.
#[post("/api/just-released")]
pub async fn just_released(page: i64) -> Result<Page<PortCard>, ServerFnError> {
    let pool = pool()?;
    let total = count_just_released(pool).await?;
    let (page, pages, offset) = paginate(page, total, PAGE_SIZE)?;
    let rows = ports::just_released(pool, PAGE_SIZE, offset)
        .await
        .map_err(db_error)?;
    Ok(Page {
        items: port_cards(pool, rows).await?,
        page,
        pages,
        total,
    })
}

/// Wire form of a stored port description.
#[cfg(feature = "server")]
fn about_dto(row: port_descriptions::PortDescriptionRow) -> recomphub_shared::dto::PortAbout {
    recomphub_shared::dto::PortAbout {
        technique_label: recomphub_shared::dto::technique_label(&row.technique).to_string(),
        summary: row.summary,
        technique: row.technique,
        technique_note: row.technique_note,
        features: row.features,
        requirements: row.requirements,
        limitations: row.limitations,
        setup: row.setup,
        generated_at: row.generated_at,
    }
}

/// One published port of a game, with its facts, other ports and the game's videos.
/// Unknown game or port slug returns 404.
#[post("/api/port")]
pub async fn port_detail(
    game_slug: String,
    port_slug: String,
) -> Result<PortDetail, ServerFnError> {
    let pool = pool()?;
    let found = ports::find_published(pool, &game_slug, &port_slug)
        .await
        .map_err(db_error)?
        .ok_or_else(|| not_found("port"))?;
    let game = games::get(pool, found.port.game_id)
        .await
        .map_err(db_error)?;
    let game_card = game_cards(pool, std::slice::from_ref(&game))
        .await?
        .remove(0);

    let p = &found.port;
    let port_id = p.id;
    let source_platform_id = p.source_platform_id;
    let homepage_url = p.homepage_url.clone();
    let description = p.description.clone();
    let requirements = p.requirements.clone();
    let toolchain = p.toolchain.clone();
    let license = p.license.clone();
    let latest_version_label = p.latest_version.clone();
    let last_commit_at = p.last_commit_at.or(p.pushed_at);
    let ai_source = p.ai_source.clone();
    let about = port_descriptions::get(pool, port_id)
        .await
        .map_err(db_error)?
        .map(about_dto);

    let platform_rows = games::platforms_for_game(pool, game.id)
        .await
        .map_err(db_error)?;
    let counts = platform_counts(pool).await?;
    let source_platform = source_platform_id
        .and_then(|sid| platform_rows.iter().find(|row| row.id == sid))
        .map(|row| platform_dto_counted(row, &counts));

    let weekly_commits = found.port.weekly_commits.clone();
    let port = port_cards(pool, vec![found]).await?.remove(0);

    let other_rows: Vec<PortRow> = ports::list_for_game(pool, game.id)
        .await
        .map_err(db_error)?
        .into_iter()
        .filter(|r| r.id != port_id)
        .collect();
    let other_ports = port_cards(pool, with_game(other_rows, &game)).await?;

    let mut video_rows = videos::list_for_game(pool, game.id, false)
        .await
        .map_err(db_error)?;
    // Videos attributed to this port first; the stable sort keeps the list order within each group.
    video_rows.sort_by_key(|v| v.port_id != Some(port_id));
    let videos = video_dtos(pool, video_rows).await?;
    let port_screenshots: Vec<MediaRef> =
        recomphub_db::port_media::approved_for_port(pool, port_id)
            .await
            .map_err(db_error)?
            .iter()
            .map(media_ref)
            .collect();
    // A port with its own contributed screenshots shows those; otherwise it falls back to the
    // game's existing screenshots rather than opening the hero on nothing but the cover.
    let screenshots = if port_screenshots.is_empty() {
        let ids = games::screenshot_media_ids(pool, game.id)
            .await
            .map_err(db_error)?;
        let media = media_map(pool, ids.clone()).await?;
        ids.iter().filter_map(|id| media.get(id).cloned()).collect()
    } else {
        port_screenshots
    };

    Ok(PortDetail {
        port,
        game: game_card,
        source_platform,
        homepage_url,
        description,
        about,
        requirements,
        toolchain,
        license,
        latest_version_label,
        last_commit_at,
        ai_source,
        other_ports,
        videos,
        screenshots,
        weekly_commits,
    })
}

/// Search: up to 20 games (full text and trigram) and up to 20 ports.
#[post("/api/search")]
pub async fn search(term: String) -> Result<SearchResults, ServerFnError> {
    let q = term.trim();
    if q.is_empty() {
        return Ok(SearchResults {
            games: Vec::new(),
            ports: Vec::new(),
        });
    }
    let pool = pool()?;
    let game_rows = games::search(pool, q, 20).await.map_err(db_error)?;
    let port_rows = ports::search_ports(pool, q, 20).await.map_err(db_error)?;
    Ok(SearchResults {
        games: game_cards(pool, &game_rows).await?,
        ports: port_cards(pool, port_rows).await?,
    })
}

// ---- server-side helpers -------------------------------------------------

#[cfg(feature = "server")]
fn not_found(what: &str) -> ServerFnError {
    http_error(StatusCode::NOT_FOUND, format!("{what} not found"))
}

#[cfg(feature = "server")]
fn bad_request(msg: impl Into<String>) -> ServerFnError {
    http_error(StatusCode::BAD_REQUEST, msg)
}

/// Clamps `page` to at least 1 and returns `(page, pages, offset)`.
/// A page beyond the last page is 404. An empty result still has one page.
#[cfg(feature = "server")]
fn paginate(page: i64, total: i64, size: i64) -> Result<(i64, i64, i64), ServerFnError> {
    let page = page.max(1);
    let pages = ((total + size - 1) / size).max(1);
    if page > pages {
        return Err(not_found("page"));
    }
    Ok((page, pages, (page - 1) * size))
}

#[cfg(feature = "server")]
fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

#[cfg(feature = "server")]
async fn platform_row(pool: &sqlx::PgPool, slug: &str) -> Result<PlatformRow, ServerFnError> {
    games::platform_by_slug(pool, slug)
        .await
        .map_err(db_error)?
        .ok_or_else(|| not_found("platform"))
}

/// Game and ported-game counts per platform id.
#[cfg(feature = "server")]
async fn platform_counts(pool: &sqlx::PgPool) -> Result<HashMap<i32, (i64, i64)>, ServerFnError> {
    Ok(games::platform_hubs(pool, false)
        .await
        .map_err(db_error)?
        .into_iter()
        .map(|h| (h.platform.id, (h.game_count, h.ported_count)))
        .collect())
}

#[cfg(feature = "server")]
fn platform_dto(p: &PlatformRow, game_count: i64, ported_count: i64) -> PlatformRef {
    PlatformRef {
        id: p.id,
        slug: p.slug.clone(),
        name: p.name.clone(),
        abbreviation: p.abbreviation.clone(),
        family: p.family.clone(),
        game_count,
        ported_count,
    }
}

#[cfg(feature = "server")]
fn platform_dto_counted(p: &PlatformRow, counts: &HashMap<i32, (i64, i64)>) -> PlatformRef {
    let (game_count, ported_count) = counts.get(&p.id).copied().unwrap_or((0, 0));
    platform_dto(p, game_count, ported_count)
}

#[cfg(feature = "server")]
async fn count_just_released(pool: &sqlx::PgPool) -> Result<i64, ServerFnError> {
    // Same filter as `ports::just_released`; db has no count for it.
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM ports p WHERE p.status = 'published' AND p.kind <> 'decompilation'",
    )
    .fetch_one(pool)
    .await
    .map_err(|e| db_error(e.into()))
}

#[cfg(feature = "server")]
fn media_ref(m: &MediaRow) -> MediaRef {
    let filename = m
        .public_filename
        .clone()
        .filter(|filename| !filename.is_empty());
    let ready = m.status == "ready"
        && m.render_version >= recomphub_ingest::media::MEDIA_RENDER_VERSION
        && !m.widths.is_empty()
        && filename.is_some();
    MediaRef {
        id: m.id,
        kind: m.kind.clone(),
        filename,
        widths: m.widths.clone(),
        width: m.width,
        height: m.height,
        ready,
    }
}

/// One batch query for any number of media ids.
#[cfg(feature = "server")]
async fn media_map(
    pool: &sqlx::PgPool,
    mut ids: Vec<i64>,
) -> Result<HashMap<i64, MediaRef>, ServerFnError> {
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = media::get_many(pool, ids).await.map_err(db_error)?;
    Ok(rows.iter().map(|m| (m.id, media_ref(m))).collect())
}

/// Maps stored target values to URL slugs. Unknown values are dropped.
#[cfg(feature = "server")]
fn target_slugs(raw: &[String]) -> Vec<String> {
    raw.iter()
        .filter_map(|t| TargetPlatform::from_str(t).ok())
        .map(|t| t.slug().to_string())
        .collect()
}

/// Video targets are a platform value or `any`; `any` passes through unchanged.
#[cfg(feature = "server")]
fn target_slug(raw: &str) -> String {
    TargetPlatform::from_str(raw)
        .map(|t| t.slug().to_string())
        .unwrap_or_else(|_| raw.to_string())
}

#[cfg(feature = "server")]
fn game_card(game: &GameRow, covers: &HashMap<i64, MediaRef>) -> GameCard {
    GameCard {
        id: game.id,
        slug: game.slug.clone(),
        name: game.name.clone(),
        release_year: game.first_release_date.map(|d| d.year()),
        cover: game.cover_media_id.and_then(|id| covers.get(&id).cloned()),
        port_count: game.port_count,
        want_count: game.want_count,
        targets: target_slugs(&game.targets),
        best_playability: game.best_playability.clone(),
        popularity: game.popularity,
    }
}

#[cfg(feature = "server")]
pub(crate) async fn game_cards(
    pool: &sqlx::PgPool,
    games: &[GameRow],
) -> Result<Vec<GameCard>, ServerFnError> {
    let covers = media_map(
        pool,
        games.iter().filter_map(|g| g.cover_media_id).collect(),
    )
    .await?;
    Ok(games.iter().map(|g| game_card(g, &covers)).collect())
}

#[cfg(feature = "server")]
async fn ranked_cards(
    pool: &sqlx::PgPool,
    rows: Vec<RankedRow>,
) -> Result<Vec<RankedGame>, ServerFnError> {
    let covers = media_map(
        pool,
        rows.iter().filter_map(|r| r.game.cover_media_id).collect(),
    )
    .await?;
    Ok(rows
        .iter()
        .map(|r| RankedGame {
            game: game_card(&r.game, &covers),
            rank: r.rank,
            prev_rank: r.prev_rank,
            score: r.score,
            hits: r.hits,
            viewer_minutes: r.viewer_minutes,
        })
        .collect())
}

/// Maintenance signals for a card. `None` until a GitHub sync has computed the level.
#[cfg(feature = "server")]
fn maintenance_dto(p: &PortRow) -> Option<MaintenanceDto> {
    let level = MaintenanceLevel::from_key(p.maintenance_level.as_deref()?)?;
    Some(MaintenanceDto {
        level,
        score: i32::from(p.maintenance_score.unwrap_or(0)),
        last_commit_at: p.last_commit_at,
        commits_30d: p.commits_30d,
        commits_90d: p.commits_90d,
        commits_365d: p.commits_365d,
        committers_180d: p.committers_180d,
        releases_365d: p.releases_365d,
    })
}

#[cfg(feature = "server")]
fn with_game(ports: Vec<PortRow>, game: &GameRow) -> Vec<PortWithGame> {
    ports
        .into_iter()
        .map(|port| PortWithGame {
            port,
            game_slug: game.slug.clone(),
            game_name: game.name.clone(),
            game_cover_media_id: game.cover_media_id,
        })
        .collect()
}

#[cfg(feature = "server")]
fn release_key(state: ReleaseState) -> &'static str {
    match state {
        ReleaseState::Archived => "archived",
        ReleaseState::Inactive => "inactive",
        ReleaseState::Released => "released",
        ReleaseState::Prerelease => "prerelease",
        ReleaseState::Unreleased => "unreleased",
    }
}

/// Stored funding links for the given lowercase logins, keyed by login. Unknown platforms are
/// dropped.
#[cfg(feature = "server")]
async fn funding_map(
    pool: &sqlx::PgPool,
    logins: &[String],
) -> Result<HashMap<String, Vec<FundingLink>>, ServerFnError> {
    let mut map: HashMap<String, Vec<FundingLink>> = HashMap::new();
    if logins.is_empty() {
        return Ok(map);
    }
    for row in author_funding::for_logins(pool, logins)
        .await
        .map_err(db_error)?
    {
        if let Ok(platform) = FundingPlatform::from_str(&row.platform) {
            map.entry(row.login).or_default().push(FundingLink {
                platform,
                url: row.url,
            });
        }
    }
    Ok(map)
}

/// Builds cards for ports, batching authors' avatars and covers into one media query.
/// A port with no push date counts as 0 days since its last push.
#[cfg(feature = "server")]
pub(crate) async fn port_cards(
    pool: &sqlx::PgPool,
    items: Vec<PortWithGame>,
) -> Result<Vec<PortCard>, ServerFnError> {
    let mut authors_by_port: Vec<Vec<AuthorRow>> = Vec::with_capacity(items.len());
    for item in &items {
        authors_by_port.push(ports::authors(pool, item.port.id).await.map_err(db_error)?);
    }
    let mut media_ids: Vec<i64> = items.iter().filter_map(|i| i.game_cover_media_id).collect();
    media_ids.extend(
        authors_by_port
            .iter()
            .flatten()
            .filter_map(|a| a.avatar_media_id),
    );
    let media = media_map(pool, media_ids).await?;
    let mut logins: Vec<String> = authors_by_port
        .iter()
        .flatten()
        .filter_map(|a| a.github_login.as_deref())
        .map(str::to_ascii_lowercase)
        .collect();
    logins.sort_unstable();
    logins.dedup();
    let funding = funding_map(pool, &logins).await?;

    let mut cards = Vec::with_capacity(items.len());
    for (item, authors) in items.into_iter().zip(authors_by_port) {
        let p = item.port;
        let evidence = p.evidence().map_err(db_error)?;
        let release = release_state(ReleaseInput {
            archived: p.archived,
            days_since_push: p.pushed_at.map_or(0, |t| (Utc::now() - t).num_days()),
            latest_is_prerelease: p.latest_is_prerelease,
        });
        let kind_label =
            PortKind::from_str(&p.kind).map_or_else(|_| p.kind.clone(), |k| k.label().to_string());
        let maintenance = maintenance_dto(&p);
        cards.push(PortCard {
            id: p.id,
            slug: p.slug.clone(),
            game_id: p.game_id,
            game_slug: item.game_slug,
            game_name: item.game_name,
            name: p.name.clone(),
            kind: p.kind.clone(),
            kind_label,
            targets: p.platforms().iter().map(|t| t.slug().to_string()).collect(),
            authors: authors
                .iter()
                .map(|a| AuthorDto {
                    github_login: a.github_login.clone(),
                    name: a.name.clone(),
                    role: a.role.clone(),
                    avatar: a.avatar_media_id.and_then(|id| media.get(&id).cloned()),
                    commits: a.commits,
                    funding: a
                        .github_login
                        .as_deref()
                        .and_then(|l| funding.get(&l.to_ascii_lowercase()))
                        .cloned()
                        .unwrap_or_default(),
                })
                .collect(),
            ai_usage: p.ai_usage.clone(),
            ai_tools: p.ai_tools.clone(),
            ai_evidence: evidence
                .into_iter()
                .map(|e| AiEvidenceDto {
                    signal: e.signal,
                    detail: e.detail,
                })
                .collect(),
            playability: p.playability.clone(),
            latest_version: p.latest_version.clone(),
            latest_release_url: p.latest_release_url.clone(),
            latest_release_at: p.latest_release_at,
            release_state: release_key(release).to_string(),
            release_state_label: release.label().to_string(),
            github_stars: p.github_stars,
            star_count: p.star_count,
            repo_url: p.repo_url.clone(),
            cover: item
                .game_cover_media_id
                .and_then(|id| media.get(&id).cloned()),
            maintenance,
        });
    }
    Ok(cards)
}

#[cfg(feature = "server")]
async fn video_dtos(
    pool: &sqlx::PgPool,
    rows: Vec<VideoRow>,
) -> Result<Vec<VideoDto>, ServerFnError> {
    let thumbs = media_map(pool, rows.iter().filter_map(|v| v.thumb_media_id).collect()).await?;
    Ok(rows
        .into_iter()
        .map(|v| VideoDto {
            id: v.id,
            youtube_id: v.youtube_id,
            title: v.title,
            channel: v.channel,
            target: target_slug(&v.target),
            thumb: v.thumb_media_id.and_then(|id| thumbs.get(&id).cloned()),
            hidden: v.hidden,
        })
        .collect())
}

#[cfg(feature = "server")]
async fn activity_items(
    pool: &sqlx::PgPool,
    rows: Vec<ActivityRow>,
) -> Result<Vec<recomphub_shared::live::ActivityItem>, ServerFnError> {
    let covers = media_map(
        pool,
        rows.iter().filter_map(|r| r.game_cover_media_id).collect(),
    )
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let version = r
                .data
                .get("version")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let game_cover = r
                .game_cover_media_id
                .and_then(|id| covers.get(&id).cloned());
            recomphub_shared::live::ActivityItem {
                id: r.id,
                kind: r.kind,
                created_at: r.created_at,
                game_id: r.game_id,
                game_slug: r.game_slug,
                game_name: r.game_name,
                game_cover,
                port_id: r.port_id,
                port_slug: r.port_slug,
                port_name: r.port_name,
                version,
            }
        })
        .collect())
}
