//! Moderator queue, admin tools, and the game search used by the submit and moderation pages.

use dioxus::prelude::*;
use recomphub_shared::dto::{
    GameChoice, LogItem, Page, QueueItem, ReviewOverrides, SettingRow, UserAdminRow,
};

#[cfg(feature = "server")]
use recomphub_shared::dto::ChoicePlatform;

#[cfg(feature = "server")]
use crate::server::session::{
    db_error, http_error, require_admin, require_moderator, require_user,
};
#[cfg(feature = "server")]
use crate::server::state::pool;
#[cfg(feature = "server")]
use chrono::Datelike;
#[cfg(feature = "server")]
use dioxus::server::axum::http::StatusCode;
#[cfg(feature = "server")]
use recomphub_db::submissions::{ApproveOverrides, PendingRow};
#[cfg(feature = "server")]
use recomphub_ingest::igdb::{IgdbClient, ensure_platforms};
#[cfg(feature = "server")]
use recomphub_shared::enums::{PortKind, Role};

/// Page size for the moderation log.
#[cfg(feature = "server")]
const LOG_PAGE_SIZE: i64 = 50;
/// Upper bound on the pending queue returned in one response.
#[cfg(feature = "server")]
const QUEUE_LIMIT: i64 = 500;

#[cfg(feature = "server")]
fn sql_error(e: sqlx::Error) -> ServerFnError {
    db_error(recomphub_db::DbError::Sql(e))
}

#[cfg(feature = "server")]
pub(crate) fn igdb_client() -> Result<IgdbClient, ServerFnError> {
    IgdbClient::from_env().map_err(|e| {
        http_error(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("IGDB is not configured: {e}"),
        )
    })
}

#[cfg(feature = "server")]
pub(crate) fn igdb_error(e: anyhow::Error) -> ServerFnError {
    http_error(StatusCode::BAD_GATEWAY, format!("IGDB request failed: {e}"))
}

#[cfg(feature = "server")]
fn queue_item(row: PendingRow) -> Result<QueueItem, ServerFnError> {
    let payload_json = serde_json::to_string(&row.sub.payload)
        .map_err(|e| http_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let before_json = match &row.sub.before {
        Some(v) => Some(
            serde_json::to_string(v)
                .map_err(|e| http_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        ),
        None => None,
    };
    let flag = |key: &str| {
        row.sub
            .payload
            .get(key)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    };
    Ok(QueueItem {
        id: row.sub.id,
        kind: row.sub.kind.clone(),
        status: row.sub.status.clone(),
        source: row.sub.source.clone(),
        submitter_login: row.submitter_login,
        submitter_approved: row.submitter_approved,
        payload_json,
        before_json,
        needs_game_match: flag("needs_game_match"),
        submitter_is_author: flag("submitter_is_author"),
        created_at: row.sub.created_at,
    })
}

/// Pending submissions, oldest first.
#[get("/api/moderation/queue")]
pub async fn moderation_queue() -> Result<Vec<QueueItem>, ServerFnError> {
    require_moderator().await?;
    let rows = recomphub_db::submissions::list_pending(pool()?, QUEUE_LIMIT, 0)
        .await
        .map_err(db_error)?;
    rows.into_iter().map(queue_item).collect()
}

/// Approve a pending submission, optionally assigning its game or creating it from IGDB.
#[post("/api/moderation/approve")]
pub async fn approve(item_id: i64, overrides: ReviewOverrides) -> Result<(), ServerFnError> {
    let reviewer = require_moderator().await?;
    let pool = pool()?;
    let kind =
        match overrides.kind {
            Some(k) => Some(k.parse::<PortKind>().map_err(|_| {
                http_error(StatusCode::BAD_REQUEST, format!("Unknown port kind: {k}"))
            })?),
            None => None,
        };
    let mut ov = ApproveOverrides {
        game_id: overrides.game_id,
        new_game: None,
        kind,
    };
    if overrides.game_id.is_none()
        && let Some(igdb_id) = overrides.igdb_id
    {
        let client = igdb_client()?;
        let game = client
            .fetch_game(igdb_id)
            .await
            .map_err(igdb_error)?
            .ok_or_else(|| {
                http_error(StatusCode::NOT_FOUND, format!("IGDB has no game {igdb_id}"))
            })?;
        ensure_platforms(pool, &client, &game.platform_igdb_ids)
            .await
            .map_err(igdb_error)?;
        ov.new_game = Some(game);
    }
    recomphub_db::submissions::approve(pool, item_id, reviewer.id, &ov)
        .await
        .map_err(db_error)?;
    Ok(())
}

/// Reject a pending submission. The note is optional and may be empty.
#[post("/api/moderation/reject")]
pub async fn reject(item_id: i64, text: String) -> Result<(), ServerFnError> {
    let reviewer = require_moderator().await?;
    let note = text.trim();
    let note = (!note.is_empty()).then_some(note);
    recomphub_db::submissions::reject(pool()?, item_id, reviewer.id, note)
        .await
        .map_err(db_error)?;
    Ok(())
}

/// Undo an applied submission.
#[post("/api/moderation/revert")]
pub async fn revert(id: i64) -> Result<(), ServerFnError> {
    let reviewer = require_moderator().await?;
    recomphub_db::submissions::revert(pool()?, id, reviewer.id)
        .await
        .map_err(db_error)?;
    Ok(())
}

/// Applied and reverted submissions, newest first, 50 per page.
#[get("/api/moderation/log")]
pub async fn moderation_log(page: i64) -> Result<Page<LogItem>, ServerFnError> {
    require_moderator().await?;
    let pool = pool()?;
    let page = page.max(1);
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM submissions WHERE status IN ('approved', 'auto_approved', 'reverted')",
    )
    .fetch_one(pool)
    .await
    .map_err(sql_error)?;
    let pages = ((total + LOG_PAGE_SIZE - 1) / LOG_PAGE_SIZE).max(1);
    if page > pages {
        return Err(http_error(
            StatusCode::NOT_FOUND,
            "No such page in the log.",
        ));
    }
    let rows = recomphub_db::submissions::list_log(pool, LOG_PAGE_SIZE, (page - 1) * LOG_PAGE_SIZE)
        .await
        .map_err(db_error)?;
    let items = rows
        .into_iter()
        .map(|r| LogItem {
            id: r.id,
            kind: r.kind,
            status: r.status,
            source: r.source,
            target_port_id: r.target_port_id,
            created_at: r.created_at,
            reviewed_at: r.reviewed_at,
        })
        .collect();
    Ok(Page {
        items,
        page,
        pages,
        total,
    })
}

/// Hide a gameplay video from its game page.
#[post("/api/moderation/hide_video")]
pub async fn hide_video(video_id: i64) -> Result<(), ServerFnError> {
    require_moderator().await?;
    recomphub_db::videos::set_hidden(pool()?, video_id, true)
        .await
        .map_err(db_error)?;
    Ok(())
}

/// Users whose login or name matches the term, for the admin search.
#[get("/api/admin/users")]
pub async fn admin_users(term: String) -> Result<Vec<UserAdminRow>, ServerFnError> {
    require_admin().await?;
    let rows = recomphub_db::users_sessions::search(pool()?, term.trim(), 50)
        .await
        .map_err(db_error)?;
    Ok(rows
        .into_iter()
        .map(|u| UserAdminRow {
            id: u.id,
            login: u.login,
            name: u.name,
            role: u.role,
            banned: u.banned,
            approved_count: u.approved_count,
        })
        .collect())
}

/// Set a user's role to `user`, `moderator` or `admin`.
#[post("/api/admin/set_role")]
pub async fn set_role(user_id: i64, value: String) -> Result<(), ServerFnError> {
    require_admin().await?;
    let role = value
        .parse::<Role>()
        .map_err(|_| http_error(StatusCode::BAD_REQUEST, format!("Unknown role: {value}")))?;
    recomphub_db::users_sessions::set_role(pool()?, user_id, role)
        .await
        .map_err(db_error)?;
    Ok(())
}

/// Ban or unban a user. Admins cannot ban themselves, which would lock out the last admin.
#[post("/api/admin/set_banned")]
pub async fn set_banned(user_id: i64, next: bool) -> Result<(), ServerFnError> {
    let admin = require_admin().await?;
    if next && admin.id == user_id {
        return Err(http_error(
            StatusCode::BAD_REQUEST,
            "Admins cannot ban their own account.",
        ));
    }
    recomphub_db::users_sessions::set_banned(pool()?, user_id, next)
        .await
        .map_err(db_error)?;
    Ok(())
}

/// Every site setting. Each value is a whole number, so a non-integer is a server error.
#[get("/api/admin/settings")]
pub async fn admin_settings() -> Result<Vec<SettingRow>, ServerFnError> {
    require_admin().await?;
    let rows = recomphub_db::settings::all(pool()?)
        .await
        .map_err(db_error)?;
    rows.into_iter()
        .map(|(key, value)| {
            let value = value.as_i64().ok_or_else(|| {
                http_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Setting {key} is not a whole number"),
                )
            })?;
            Ok(SettingRow { key, value })
        })
        .collect()
}

/// Update one site setting. The db layer validates the key and the value.
#[post("/api/admin/update_setting")]
pub async fn update_setting(key: String, value: i64) -> Result<(), ServerFnError> {
    require_admin().await?;
    recomphub_db::settings::set(pool()?, &key, &serde_json::json!(value))
        .await
        .map_err(db_error)?;
    Ok(())
}

/// Games already in the local catalog whose title matches the query.
#[post("/api/games/search")]
pub async fn search_games(text: String) -> Result<Vec<GameChoice>, ServerFnError> {
    require_user().await?;
    let q = text.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let pool = pool()?;
    let games = recomphub_db::games::search(pool, q, 20)
        .await
        .map_err(db_error)?;
    let mut out = Vec::with_capacity(games.len());
    for g in games {
        let platforms = recomphub_db::games::platforms_for_game(pool, g.id)
            .await
            .map_err(db_error)?
            .into_iter()
            .map(|p| ChoicePlatform {
                id: Some(p.id),
                igdb_id: Some(p.igdb_id.into()),
                name: p.name,
            })
            .collect();
        out.push(GameChoice {
            game_id: Some(g.id),
            igdb_id: Some(g.igdb_id),
            name: g.name,
            release_year: g.first_release_date.map(|d| d.year()),
            platforms,
        });
    }
    Ok(out)
}

/// Games on IGDB matching the query. Results are not stored; picking one creates the game on approval.
#[post("/api/igdb/search")]
pub async fn search_igdb(text: String) -> Result<Vec<GameChoice>, ServerFnError> {
    require_user().await?;
    let q = text.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let client = igdb_client()?;
    let hits = client.search_games(q).await.map_err(igdb_error)?;
    let pool = pool()?;
    let mut platform_ids: Vec<i32> = hits
        .iter()
        .flat_map(|h| h.game.platform_igdb_ids.iter().copied())
        .collect();
    platform_ids.sort_unstable();
    platform_ids.dedup();
    ensure_platforms(pool, &client, &platform_ids)
        .await
        .map_err(igdb_error)?;
    let known: Vec<(i32, i32, String)> = sqlx::query_as(
        "SELECT id, igdb_id, name FROM platforms WHERE igdb_id = ANY($1) ORDER BY name",
    )
    .bind(&platform_ids)
    .fetch_all(pool)
    .await
    .map_err(sql_error)?;
    Ok(hits
        .into_iter()
        .map(|h| {
            let game = h.game;
            let platforms = known
                .iter()
                .filter(|(_, igdb_id, _)| game.platform_igdb_ids.contains(igdb_id))
                .map(|(id, igdb_id, name)| ChoicePlatform {
                    id: Some(*id),
                    igdb_id: Some((*igdb_id).into()),
                    name: name.clone(),
                })
                .collect();
            GameChoice {
                game_id: None,
                igdb_id: Some(game.igdb_id),
                name: game.name,
                release_year: game.first_release_date.map(|d| d.year()),
                platforms,
            }
        })
        .collect())
}
