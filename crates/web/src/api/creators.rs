//! Creator hub server functions: what to recompile, ports missing major platforms, and ports
//! that need gameplay videos. Every endpoint is public and returns plain data for the to-do lists.

use dioxus::prelude::*;
use recomphub_shared::dto::{CreatorsHub, Page, PortGapItem, RecompileItem, VideoGapItem};

#[cfg(feature = "server")]
use crate::server::session::{db_error, http_error};
#[cfg(feature = "server")]
use crate::server::state::pool;
#[cfg(feature = "server")]
use chrono::Datelike;
#[cfg(feature = "server")]
use dioxus::server::axum::http::StatusCode;
#[cfg(feature = "server")]
use recomphub_db::DbResult;
#[cfg(feature = "server")]
use recomphub_db::creators::{self, PortGapRow, RecompileRow, VideoGapRow};
#[cfg(feature = "server")]
use recomphub_shared::enums::TargetPlatform;

/// Rows per page on the full lists.
#[cfg(feature = "server")]
const PAGE_SIZE: i64 = 50;
/// Rows per section on the hub preview.
#[cfg(feature = "server")]
const HUB_SIZE: i64 = 5;

/// Top rows of all three lists, plus totals, for the creators hub.
#[get("/api/creators")]
pub async fn creators_hub() -> Result<CreatorsHub, ServerFnError> {
    let pool = pool()?;
    let recompile_total = db_result(creators::recompile_count(pool).await)?;
    let recompile = db_result(creators::recompile_page(pool, HUB_SIZE, 0).await)?;
    let ports_total = db_result(creators::port_gaps_count(pool, None).await)?;
    let ports = db_result(creators::port_gaps_page(pool, None, HUB_SIZE, 0).await)?;
    let videos_total = db_result(creators::video_gaps_count(pool, None).await)?;
    let videos = db_result(creators::video_gaps_page(pool, None, HUB_SIZE, 0).await)?;
    Ok(CreatorsHub {
        recompile: recompile.into_iter().map(recompile_item).collect(),
        recompile_total,
        ports: ports.into_iter().map(port_gap_item).collect(),
        ports_total,
        videos: videos.into_iter().map(video_gap_item).collect(),
        videos_total,
    })
}

/// Full "what to recompile" list, one page at a time.
#[post("/api/creators/recompile")]
pub async fn creators_recompile(page: i64) -> Result<Page<RecompileItem>, ServerFnError> {
    let pool = pool()?;
    let total = db_result(creators::recompile_count(pool).await)?;
    let (page, pages, offset) = paginate(page, total)?;
    let rows = db_result(creators::recompile_page(pool, PAGE_SIZE, offset).await)?;
    Ok(Page {
        items: rows.into_iter().map(recompile_item).collect(),
        page,
        pages,
        total,
    })
}

/// Full "ports missing platforms" list. `missing` is a major target slug, or empty for all.
#[post("/api/creators/platforms")]
pub async fn creators_port_gaps(
    missing: String,
    page: i64,
) -> Result<Page<PortGapItem>, ServerFnError> {
    let pool = pool()?;
    let filter = major_filter(&missing)?;
    let total = db_result(creators::port_gaps_count(pool, filter).await)?;
    let (page, pages, offset) = paginate(page, total)?;
    let rows = db_result(creators::port_gaps_page(pool, filter, PAGE_SIZE, offset).await)?;
    Ok(Page {
        items: rows.into_iter().map(port_gap_item).collect(),
        page,
        pages,
        total,
    })
}

/// Full "videos to make" list. `target` is a YouTube-searchable target slug, or empty for all.
#[post("/api/creators/videos")]
pub async fn creators_video_gaps(
    target: String,
    page: i64,
) -> Result<Page<VideoGapItem>, ServerFnError> {
    let pool = pool()?;
    let filter = video_filter(&target)?;
    let total = db_result(creators::video_gaps_count(pool, filter).await)?;
    let (page, pages, offset) = paginate(page, total)?;
    let rows = db_result(creators::video_gaps_page(pool, filter, PAGE_SIZE, offset).await)?;
    Ok(Page {
        items: rows.into_iter().map(video_gap_item).collect(),
        page,
        pages,
        total,
    })
}

#[cfg(feature = "server")]
fn db_result<T>(result: DbResult<T>) -> Result<T, ServerFnError> {
    result.map_err(db_error)
}

#[cfg(feature = "server")]
fn not_found(what: &str) -> ServerFnError {
    http_error(StatusCode::NOT_FOUND, format!("{what} not found"))
}

/// Clamps `page` to at least 1 and returns `(page, pages, offset)`. A page past the last is 404.
/// An empty result still has one page.
#[cfg(feature = "server")]
fn paginate(page: i64, total: i64) -> Result<(i64, i64, i64), ServerFnError> {
    let page = page.max(1);
    let pages = creators::page_count(total, PAGE_SIZE);
    if page > pages {
        return Err(not_found("page"));
    }
    Ok((page, pages, creators::offset_for(page, PAGE_SIZE)))
}

/// Empty means no filter. Any other value must be a major target.
#[cfg(feature = "server")]
fn major_filter(raw: &str) -> Result<Option<&str>, ServerFnError> {
    if raw.is_empty() {
        return Ok(None);
    }
    if creators::is_major_target(raw) {
        Ok(Some(raw))
    } else {
        Err(not_found("target"))
    }
}

/// Empty means no filter. Any other value must be a YouTube-searchable target.
#[cfg(feature = "server")]
fn video_filter(raw: &str) -> Result<Option<&'static str>, ServerFnError> {
    if raw.is_empty() {
        return Ok(None);
    }
    match TargetPlatform::from_slug(raw) {
        Some(t) if t.youtube_searchable() => Ok(Some(t.as_str())),
        _ => Err(not_found("target")),
    }
}

#[cfg(feature = "server")]
fn recompile_item(row: RecompileRow) -> RecompileItem {
    RecompileItem {
        game_id: row.game.id,
        game_slug: row.game.slug,
        game_name: row.game.name,
        release_year: row.game.first_release_date.map(|d| d.year()),
        original_platforms: row.original_platforms,
        want_count: row.game.want_count,
        has_decompilation: row.game.decomp_count > 0,
        popularity: row.game.popularity,
        sales: row.sale.signal(),
    }
}

#[cfg(feature = "server")]
fn port_gap_item(row: PortGapRow) -> PortGapItem {
    PortGapItem {
        port_id: row.port_id,
        port_slug: row.port_slug,
        port_name: row.port_name,
        game_slug: row.game_slug,
        game_name: row.game_name,
        repo_url: row.repo_url,
        github_stars: row.github_stars,
        targets: row.targets,
        missing: row.missing,
        popularity: row.popularity,
        sales: row.sale.signal(),
    }
}

#[cfg(feature = "server")]
fn video_gap_item(row: VideoGapRow) -> VideoGapItem {
    VideoGapItem {
        port_id: row.port_id,
        port_slug: row.port_slug,
        port_name: row.port_name,
        game_slug: row.game_slug,
        game_name: row.game_name,
        repo_url: row.repo_url,
        github_stars: row.github_stars,
        targets: row.targets,
        video_count: row.video_count,
        popularity: row.popularity,
        sales: row.sale.signal(),
    }
}
