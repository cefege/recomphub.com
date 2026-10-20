//! Public Discord stats for the "Join Discord" CTAs: an approximate member and online count.
//! Reads Discord's unauthenticated invite lookup (`GET /invites/{code}?with_counts=true`), so it
//! needs no bot token and no server-widget setting. Cached in-process, since several CTAs can
//! share one page load and Discord's numbers only need to be fresh to a few minutes.

#[cfg(feature = "server")]
use crate::app::DISCORD_INVITE_CODE;
#[cfg(feature = "server")]
use crate::server::session::http_error;
use dioxus::prelude::*;
#[cfg(feature = "server")]
use dioxus::server::axum::http::StatusCode;
use recomphub_shared::dto::DiscordStatsDto;

#[cfg(feature = "server")]
mod cache {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    const TTL: Duration = Duration::from_secs(600);

    pub struct Cached<T>(Mutex<Option<(Instant, T)>>);

    impl<T: Clone> Cached<T> {
        pub const fn new() -> Self {
            Self(Mutex::new(None))
        }

        pub fn get(&self) -> Option<T> {
            let guard = self.0.lock().ok()?;
            guard
                .as_ref()
                .filter(|(at, _)| at.elapsed() < TTL)
                .map(|(_, value)| value.clone())
        }

        pub fn put(&self, value: T) {
            if let Ok(mut guard) = self.0.lock() {
                *guard = Some((Instant::now(), value));
            }
        }
    }
}

#[cfg(feature = "server")]
static STATS: cache::Cached<DiscordStatsDto> = cache::Cached::new();

/// Approximate member and online counts for the RecompHub Discord.
#[get("/api/discord/stats")]
pub async fn discord_stats() -> Result<DiscordStatsDto, ServerFnError> {
    if let Some(hit) = STATS.get() {
        return Ok(hit);
    }
    let http = reqwest::Client::builder()
        .user_agent("recomphub/1.0 (+https://recomphub.com)")
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| http_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let url = format!("https://discord.com/api/v10/invites/{DISCORD_INVITE_CODE}?with_counts=true");
    let body: serde_json::Value = http
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| http_error(StatusCode::BAD_GATEWAY, e.to_string()))?
        .json()
        .await
        .map_err(|e| http_error(StatusCode::BAD_GATEWAY, e.to_string()))?;
    let stats = DiscordStatsDto {
        member_count: body["approximate_member_count"].as_i64().unwrap_or(0),
        online_count: body["approximate_presence_count"].as_i64().unwrap_or(0),
    };
    STATS.put(stats);
    Ok(stats)
}
