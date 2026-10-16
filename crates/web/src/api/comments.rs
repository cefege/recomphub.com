//! Port comments and star ratings: server functions for the port page.

use dioxus::prelude::*;
use recomphub_shared::dto::{CommentDto, CommentQuery, Page, RatingSummary};

#[cfg(feature = "server")]
use crate::api::members::media_ref;
#[cfg(feature = "server")]
use crate::server::session::{
    current_user_row, db_error, http_error, require_moderator, require_user,
};
#[cfg(feature = "server")]
use crate::server::state::pool;
#[cfg(feature = "server")]
use dioxus::server::axum::http::StatusCode;
#[cfg(feature = "server")]
use recomphub_db::PgPool;
#[cfg(feature = "server")]
use recomphub_db::comments::{
    self, CommentFilter, CommentRow, CommentSort as DbSort, MAX_BODY_CHARS, MAX_REASON_CHARS,
    POST_LIMIT, POST_WINDOW_MINUTES,
};
#[cfg(feature = "server")]
use recomphub_db::media::{self};
#[cfg(feature = "server")]
use recomphub_shared::TargetPlatform;
#[cfg(feature = "server")]
use recomphub_shared::dto::CommentSort;
#[cfg(feature = "server")]
use recomphub_shared::dto::MediaRef;
#[cfg(feature = "server")]
use std::collections::HashMap;

#[cfg(feature = "server")]
const PAGE_SIZE: i64 = 20;

#[cfg(feature = "server")]
fn bad_request(message: impl Into<String>) -> ServerFnError {
    http_error(StatusCode::BAD_REQUEST, message)
}

/// Maps the posting rate limit to HTTP 429 with comment-specific wording.
#[cfg(feature = "server")]
fn comment_error(err: recomphub_db::DbError) -> ServerFnError {
    match err {
        recomphub_db::DbError::RateLimited(secs) => http_error(
            StatusCode::TOO_MANY_REQUESTS,
            format!(
                "you can post {POST_LIMIT} comments every {POST_WINDOW_MINUTES} minutes; \
                 try again in {secs} seconds"
            ),
        ),
        other => db_error(other),
    }
}

/// Trims, normalises line endings and checks a comment body.
#[cfg(feature = "server")]
fn normalize_body(raw: &str) -> Result<String, ServerFnError> {
    let text = raw.replace("\r\n", "\n").replace('\r', "\n");
    let text = text.trim();
    if text.is_empty() {
        return Err(bad_request("write something before posting"));
    }
    if text.chars().count() > MAX_BODY_CHARS {
        return Err(bad_request(format!(
            "comments are limited to {MAX_BODY_CHARS} characters"
        )));
    }
    if text
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(bad_request("comments cannot contain control characters"));
    }
    Ok(text.to_string())
}

/// Validates the platform a commenter tested on. Input is a URL slug; stored as `as_str()`, matching `videos.target`.
#[cfg(feature = "server")]
fn normalize_target(raw: Option<String>) -> Result<Option<String>, ServerFnError> {
    let Some(slug) = raw.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let target = TargetPlatform::from_slug(&slug).ok_or_else(|| bad_request("unknown platform"))?;
    Ok(Some(target.as_str().to_string()))
}

#[cfg(feature = "server")]
fn normalize_reason(raw: Option<String>) -> Result<Option<String>, ServerFnError> {
    let Some(reason) = raw.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if reason.chars().count() > MAX_REASON_CHARS {
        return Err(bad_request(format!(
            "reasons are limited to {MAX_REASON_CHARS} characters"
        )));
    }
    Ok(Some(reason))
}

#[cfg(feature = "server")]
fn db_sort(sort: CommentSort) -> DbSort {
    match sort {
        CommentSort::Newest => DbSort::Newest,
        CommentSort::Highest => DbSort::Highest,
        CommentSort::Lowest => DbSort::Lowest,
    }
}

/// Resolves the avatars of the comment authors on this page in one query.
#[cfg(feature = "server")]
async fn avatar_map(
    pool: &PgPool,
    rows: &[CommentRow],
) -> Result<HashMap<i64, MediaRef>, ServerFnError> {
    let ids: Vec<i64> = rows
        .iter()
        .filter_map(|r| r.author_avatar_media_id)
        .collect();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let media = media::get_many(pool, ids).await.map_err(db_error)?;
    Ok(media.iter().map(|m| (m.id, media_ref(m))).collect())
}

#[cfg(feature = "server")]
fn to_dto(row: CommentRow, avatars: &HashMap<i64, MediaRef>) -> CommentDto {
    CommentDto {
        id: row.id,
        author_id: row.user_id,
        name: row.author_name,
        github_login: row.author_github_login,
        avatar: row
            .author_avatar_media_id
            .and_then(|id| avatars.get(&id).cloned()),
        body: row.body,
        tested_target: row
            .tested_target
            .as_deref()
            .and_then(|s| s.parse::<TargetPlatform>().ok())
            .map(|t| t.slug().to_string()),
        created_at: row.created_at,
        edited_at: row.edited_at,
        hidden: row.hidden,
        hidden_reason: row.hidden_reason,
        rating: row.author_rating,
    }
}

#[cfg(feature = "server")]
async fn comment_dto(pool: &PgPool, id: i64) -> Result<CommentDto, ServerFnError> {
    let row = comments::get(pool, id).await.map_err(db_error)?;
    let avatars = avatar_map(pool, std::slice::from_ref(&row)).await?;
    Ok(to_dto(row, &avatars))
}

#[cfg(feature = "server")]
async fn summary_of(pool: &PgPool, port_id: i64) -> Result<RatingSummary, ServerFnError> {
    let distribution = comments::rating_distribution(pool, port_id)
        .await
        .map_err(db_error)?;
    Ok(RatingSummary::from_distribution(distribution))
}

/// One page of a port's comments. Hidden comments are included only for moderators.
#[post("/api/comments/list")]
pub async fn list_comments(query: CommentQuery) -> Result<Page<CommentDto>, ServerFnError> {
    if query.include_hidden {
        require_moderator().await?;
    }
    if let Some(stars) = query.stars
        && !(1..=5).contains(&stars)
    {
        return Err(bad_request("stars must be between 1 and 5"));
    }
    let pool = pool()?;
    let page = query.page.max(1);
    let filter = CommentFilter {
        port_id: query.port_id,
        sort: db_sort(query.sort),
        stars: query.stars,
        include_hidden: query.include_hidden,
    };
    let total = comments::count(pool, &filter).await.map_err(db_error)?;
    let rows = comments::list(pool, &filter, (page - 1) * PAGE_SIZE, PAGE_SIZE)
        .await
        .map_err(db_error)?;
    let avatars = avatar_map(pool, &rows).await?;
    Ok(Page {
        items: rows.into_iter().map(|r| to_dto(r, &avatars)).collect(),
        page,
        pages: (total + PAGE_SIZE - 1).div_euclid(PAGE_SIZE).max(1),
        total,
    })
}

/// Posts a comment on a published port. Rate limits apply.
#[post("/api/comments/create")]
pub async fn create_comment(
    port_id: i64,
    body: String,
    tested_target: Option<String>,
) -> Result<CommentDto, ServerFnError> {
    let user = require_user().await?;
    let body = normalize_body(&body)?;
    let target = normalize_target(tested_target)?;
    let pool = pool()?;
    let id = comments::create(pool, port_id, user.id, &body, target.as_deref())
        .await
        .map_err(comment_error)?;
    comment_dto(pool, id).await
}

/// Edits the signed-in author's comment. Hidden comments cannot be edited.
#[post("/api/comments/edit")]
pub async fn edit_comment(
    comment_id: i64,
    body: String,
    tested_target: Option<String>,
) -> Result<CommentDto, ServerFnError> {
    let user = require_user().await?;
    let pool = pool()?;
    let row = comments::get(pool, comment_id).await.map_err(db_error)?;
    if row.user_id != user.id {
        return Err(http_error(
            StatusCode::FORBIDDEN,
            "only the author can edit this comment",
        ));
    }
    if row.hidden {
        return Err(bad_request(
            "a moderator hid this comment, so it cannot be edited",
        ));
    }
    let body = normalize_body(&body)?;
    let target = normalize_target(tested_target)?;
    comments::edit(pool, comment_id, &body, target.as_deref())
        .await
        .map_err(db_error)?;
    comment_dto(pool, comment_id).await
}

/// Deletes the signed-in author's comment. Ratings are kept.
#[post("/api/comments/delete")]
pub async fn delete_comment(comment_id: i64) -> Result<(), ServerFnError> {
    let user = require_user().await?;
    let pool = pool()?;
    let row = comments::get(pool, comment_id).await.map_err(db_error)?;
    if row.user_id != user.id {
        return Err(http_error(
            StatusCode::FORBIDDEN,
            "only the author can delete this comment",
        ));
    }
    comments::delete(pool, comment_id).await.map_err(db_error)
}

/// Hides or unhides a comment (moderators only). Hiding can also remove the author's rating
/// of the port.
#[post("/api/comments/hide")]
pub async fn set_comment_hidden(
    comment_id: i64,
    hidden: bool,
    reason: Option<String>,
    remove_rating: bool,
) -> Result<CommentDto, ServerFnError> {
    let moderator = require_moderator().await?;
    let reason = normalize_reason(reason)?;
    let pool = pool()?;
    comments::set_hidden(
        pool,
        comment_id,
        moderator.id,
        hidden,
        reason.as_deref(),
        remove_rating,
    )
    .await
    .map_err(db_error)?;
    comment_dto(pool, comment_id).await
}

/// Public star summary for a port. Carries no viewer data, so it is safe to render on the server.
#[post("/api/ratings/summary")]
pub async fn port_ratings(port_id: i64) -> Result<RatingSummary, ServerFnError> {
    summary_of(pool()?, port_id).await
}

/// The signed-in viewer's rating of a port, or `None` when signed out or unrated.
#[post("/api/ratings/mine")]
pub async fn my_rating(port_id: i64) -> Result<Option<i16>, ServerFnError> {
    let Some(user) = current_user_row().await? else {
        return Ok(None);
    };
    comments::my_rating(pool()?, port_id, user.id)
        .await
        .map_err(db_error)
}

/// Sets the signed-in viewer's rating of a port (1 to 5 stars) and returns the new summary.
#[post("/api/ratings/set")]
pub async fn rate_port(port_id: i64, stars: i16) -> Result<RatingSummary, ServerFnError> {
    let user = require_user().await?;
    let pool = pool()?;
    comments::rate(pool, port_id, user.id, stars)
        .await
        .map_err(db_error)?;
    summary_of(pool, port_id).await
}

/// Removes the signed-in viewer's rating of a port and returns the new summary.
#[post("/api/ratings/clear")]
pub async fn clear_rating(port_id: i64) -> Result<RatingSummary, ServerFnError> {
    let user = require_user().await?;
    let pool = pool()?;
    comments::unrate(pool, port_id, user.id)
        .await
        .map_err(db_error)?;
    summary_of(pool, port_id).await
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use chrono::Utc;

    fn row(tested_target: Option<&str>) -> CommentRow {
        CommentRow {
            id: 1,
            port_id: 2,
            user_id: 3,
            body: "Plays well".to_string(),
            tested_target: tested_target.map(str::to_string),
            created_at: Utc::now(),
            edited_at: None,
            hidden: false,
            hidden_reason: None,
            author_name: "Vita Tester".to_string(),
            author_github_login: None,
            author_avatar_media_id: None,
            author_rating: None,
        }
    }

    #[test]
    fn slug_input_is_stored_in_as_str_form() {
        assert_eq!(
            normalize_target(Some("ps-vita".to_string())).unwrap(),
            Some("ps_vita".to_string())
        );
        assert_eq!(
            normalize_target(Some("3ds".to_string())).unwrap(),
            Some("n3ds".to_string())
        );
        assert_eq!(normalize_target(Some("  ".to_string())).unwrap(), None);
        // The stored spelling is not a valid input slug.
        assert!(normalize_target(Some("ps_vita".to_string())).is_err());
    }

    #[test]
    fn stored_form_comes_back_as_slug() {
        let dto = to_dto(row(Some("ps_vita")), &HashMap::new());
        assert_eq!(dto.tested_target.as_deref(), Some("ps-vita"));
        let dto = to_dto(row(Some("n3ds")), &HashMap::new());
        assert_eq!(dto.tested_target.as_deref(), Some("3ds"));
    }
}
