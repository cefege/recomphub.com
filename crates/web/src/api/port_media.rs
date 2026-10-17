//! Member contributions to a port's own gallery: uploaded screenshots and submitted gameplay
//! video links. Both are moderated before they appear publicly, even from a signed-in account.

use dioxus::prelude::*;
use recomphub_shared::dto::{PendingPortImageDto, PendingVideoDto};

#[cfg(feature = "server")]
use base64::Engine;
#[cfg(feature = "server")]
use base64::engine::general_purpose::STANDARD as BASE64;
#[cfg(feature = "server")]
use dioxus::server::axum::http::StatusCode;
#[cfg(feature = "server")]
use recomphub_db::media::{self, MediaRow};
#[cfg(feature = "server")]
use recomphub_db::port_media;
#[cfg(feature = "server")]
use recomphub_db::ports;
#[cfg(feature = "server")]
use recomphub_db::videos::{self, NewVideo};
#[cfg(feature = "server")]
use recomphub_ingest::{media as media_render, video_links};
#[cfg(feature = "server")]
use recomphub_shared::dto::{MediaRef, VideoDto};
#[cfg(feature = "server")]
use sha2::{Digest, Sha256};
#[cfg(feature = "server")]
use tokio::io::AsyncWriteExt;

#[cfg(feature = "server")]
use crate::server::session::{db_error, http_error, require_moderator, require_user};
#[cfg(feature = "server")]
use crate::server::{config, state::pool};

/// Largest decoded upload accepted, matching the download cap for GitHub-sourced images.
#[cfg(feature = "server")]
const MAX_UPLOAD_BYTES: usize = recomphub_ingest::media::MAX_DOWNLOAD_BYTES;

#[cfg(feature = "server")]
fn media_ref(m: &MediaRow) -> MediaRef {
    MediaRef {
        id: m.id,
        kind: m.kind.clone(),
        hash: m.hash.clone(),
        widths: m.widths.clone(),
        width: m.width,
        height: m.height,
        ready: m.status == "ready",
    }
}

/// Screenshots one account may upload per day. An unbounded upload writes to disk before any
/// moderator looks at it, so this caps how much one account can put on the media volume.
#[cfg(feature = "server")]
const MAX_UPLOADS_PER_DAY: i64 = 20;

/// Uploads a screenshot for a port: writes the bytes to disk and queues them for `media-sync`
/// to render (AVIF encoding is CPU-heavy; running it inline would hold the request open for as
/// long as the encode takes). It is not shown on the port page until a moderator approves the
/// rendered image.
#[post("/api/port_media/upload")]
pub async fn upload_port_image(
    port_id: i64,
    filename: String,
    data_base64: String,
) -> Result<(), ServerFnError> {
    let user = require_user().await?;
    let pool = pool()?;
    ports::get_row(pool, port_id).await.map_err(db_error)?;

    let used = port_media::uploads_today(pool, user.id)
        .await
        .map_err(db_error)?;
    if used >= MAX_UPLOADS_PER_DAY {
        return Err(http_error(
            StatusCode::TOO_MANY_REQUESTS,
            format!(
                "You can upload up to {MAX_UPLOADS_PER_DAY} screenshots a day; try again tomorrow."
            ),
        ));
    }

    let bytes = BASE64
        .decode(data_base64.as_bytes())
        .map_err(|_| http_error(StatusCode::BAD_REQUEST, "that file could not be read"))?;
    if bytes.is_empty() {
        return Err(http_error(StatusCode::BAD_REQUEST, "the file is empty"));
    }
    if bytes.len() > MAX_UPLOAD_BYTES {
        return Err(http_error(
            StatusCode::BAD_REQUEST,
            format!(
                "{filename} is larger than {} MB",
                MAX_UPLOAD_BYTES / (1024 * 1024)
            ),
        ));
    }
    if !media_render::looks_like_image(&bytes) {
        return Err(http_error(
            StatusCode::BAD_REQUEST,
            format!("{filename} does not look like an image"),
        ));
    }

    let hash: String = Sha256::digest(&bytes)[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    // Queue (or reuse) the media row first: when the exact same bytes were already rendered by
    // an earlier upload, the row comes back `ready` and there is nothing left to write or
    // render, so the disk write below is skipped entirely.
    let media_id = media::enqueue(pool, "port_screenshot", &format!("upload:{hash}"))
        .await
        .map_err(db_error)?;
    let already_rendered = media::get(pool, media_id)
        .await
        .map_err(db_error)?
        .is_some_and(|m| m.status == "ready");
    if !already_rendered {
        let path = media_render::upload_path(&config::get().media_dir, &hash);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| http_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        }
        let mut file = tokio::fs::File::create(&path)
            .await
            .map_err(|e| http_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        file.write_all(&bytes)
            .await
            .map_err(|e| http_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    port_media::insert_pending(pool, port_id, media_id, user.id)
        .await
        .map_err(db_error)?;
    Ok(())
}

/// Submits a gameplay video link for a port. Fetches its title, channel and thumbnail from
/// YouTube's public oEmbed endpoint (no API key, no quota) and queues it for review.
#[post("/api/port_media/submit_video")]
pub async fn submit_port_video(port_id: i64, url: String) -> Result<(), ServerFnError> {
    require_user().await?;
    let pool = pool()?;
    let port = ports::get_row(pool, port_id).await.map_err(db_error)?;
    let youtube_id = video_links::youtube_ids_in_markdown(&url)
        .into_iter()
        .next()
        .ok_or_else(|| {
            http_error(
                StatusCode::BAD_REQUEST,
                "that doesn't look like a YouTube video link",
            )
        })?;

    #[derive(serde::Deserialize)]
    struct OEmbed {
        title: String,
        author_name: Option<String>,
    }
    let http = reqwest::Client::builder()
        .user_agent("recomphub/1.0 (+https://recomphub.com)")
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| http_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let embed_url = format!(
        "https://www.youtube.com/oembed?url=https://www.youtube.com/watch?v={youtube_id}&format=json"
    );
    let oembed: OEmbed = http
        .get(&embed_url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|_| {
            http_error(
                StatusCode::BAD_REQUEST,
                "that video could not be found; check it is public",
            )
        })?
        .json()
        .await
        .map_err(|e| http_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let thumb_url = format!("https://i.ytimg.com/vi/{youtube_id}/hqdefault.jpg");
    let thumb_media_id = media::enqueue(pool, "video_thumb", &thumb_url)
        .await
        .map_err(db_error)?;

    videos::submit_user_video(
        pool,
        &NewVideo {
            game_id: port.game_id,
            port_id: Some(port_id),
            target: "any".to_string(),
            youtube_id,
            title: oembed.title,
            channel: oembed.author_name,
            published_at: None,
            thumb_media_id: Some(thumb_media_id),
        },
    )
    .await
    .map_err(db_error)?;
    Ok(())
}

/// Pending screenshots across the site, oldest first.
#[get("/api/port_media/pending_images")]
pub async fn pending_port_images() -> Result<Vec<PendingPortImageDto>, ServerFnError> {
    require_moderator().await?;
    let pool = pool()?;
    let rows = port_media::list_pending(pool).await.map_err(db_error)?;
    let media_ids: Vec<i64> = rows.iter().map(|r| r.item.media_id).collect();
    let media_rows = media::get_many(pool, media_ids).await.map_err(db_error)?;
    let by_id: std::collections::HashMap<i64, &MediaRow> =
        media_rows.iter().map(|m| (m.id, m)).collect();
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let image = media_ref(by_id.get(&r.item.media_id)?);
            Some(PendingPortImageDto {
                id: r.item.id,
                image,
                uploader_name: r.uploader_name,
                port_name: r.port_name,
                port_slug: r.port_slug,
                game_slug: r.game_slug,
                created_at: r.item.created_at,
            })
        })
        .collect())
}

/// Approves or rejects a pending screenshot.
#[post("/api/port_media/review_image")]
pub async fn review_port_image(id: i64, approve: bool) -> Result<(), ServerFnError> {
    let moderator = require_moderator().await?;
    port_media::review(pool()?, id, moderator.id, approve)
        .await
        .map_err(db_error)
}

/// Pending, member-submitted gameplay videos, oldest first.
#[get("/api/port_media/pending_videos")]
pub async fn pending_submitted_videos() -> Result<Vec<PendingVideoDto>, ServerFnError> {
    require_moderator().await?;
    let pool = pool()?;
    let rows = videos::list_pending(pool).await.map_err(db_error)?;
    let thumb_ids: Vec<i64> = rows.iter().filter_map(|r| r.video.thumb_media_id).collect();
    let thumbs = media::get_many(pool, thumb_ids).await.map_err(db_error)?;
    let by_id: std::collections::HashMap<i64, &MediaRow> =
        thumbs.iter().map(|m| (m.id, m)).collect();
    Ok(rows
        .into_iter()
        .map(|r| {
            let thumb = r
                .video
                .thumb_media_id
                .and_then(|id| by_id.get(&id))
                .map(|m| media_ref(m));
            PendingVideoDto {
                id: r.video.id,
                video: VideoDto {
                    id: r.video.id,
                    youtube_id: r.video.youtube_id,
                    title: r.video.title,
                    channel: r.video.channel,
                    target: r.video.target,
                    thumb,
                    hidden: r.video.hidden,
                },
                game_name: r.game_name,
                game_slug: r.game_slug,
                port_name: r.port_name,
                port_slug: r.port_slug,
                created_at: r.video.fetched_at,
            }
        })
        .collect())
}

/// Approves or rejects a pending, member-submitted video.
#[post("/api/port_media/review_video")]
pub async fn review_submitted_video(id: i64, approve: bool) -> Result<(), ServerFnError> {
    require_moderator().await?;
    videos::review_pending(pool()?, id, approve)
        .await
        .map_err(db_error)
}

/// Count of pending screenshots and videos together, for the admin nav badge.
#[get("/api/port_media/pending_count")]
pub async fn pending_media_count() -> Result<i64, ServerFnError> {
    if require_moderator().await.is_err() {
        return Ok(0);
    }
    let pool = pool()?;
    let images = port_media::count_pending(pool).await.map_err(db_error)?;
    let clips = videos::count_pending(pool).await.map_err(db_error)?;
    Ok(images + clips)
}
