//! Platform parity data for `/stats` and the home teaser. Public and read-only.

use dioxus::prelude::*;
use recomphub_shared::dto::{ParityDto, StatsPageDto};

#[cfg(feature = "server")]
use crate::server::session::db_error;
#[cfg(feature = "server")]
use crate::server::state::pool;
#[cfg(feature = "server")]
use chrono::Utc;
#[cfg(feature = "server")]
use recomphub_db::stats;

/// In-process cache for the stats reads. They rebuild the full timeline, so a few
/// minutes of staleness is cheaper than a rebuild on every home and `/stats` request.
#[cfg(feature = "server")]
mod cache {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    const TTL: Duration = Duration::from_secs(300);

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
static COVERAGE: cache::Cached<ParityDto> = cache::Cached::new();
#[cfg(feature = "server")]
static PAGE: cache::Cached<StatsPageDto> = cache::Cached::new();

/// Headline parity numbers: the home teaser and the top of `/stats`.
#[get("/api/stats/coverage")]
pub async fn stats_coverage() -> Result<ParityDto, ServerFnError> {
    if let Some(hit) = COVERAGE.get() {
        return Ok(hit);
    }
    let pool = pool()?;
    let fresh = stats::coverage_summary(pool, Utc::now().date_naive())
        .await
        .map_err(db_error)?;
    COVERAGE.put(fresh.clone());
    Ok(fresh)
}

/// Everything on `/stats`: parity, history, milestones, leaderboards and gaps.
#[get("/api/stats/page")]
pub async fn stats_page() -> Result<StatsPageDto, ServerFnError> {
    if let Some(hit) = PAGE.get() {
        return Ok(hit);
    }
    let pool = pool()?;
    let fresh = stats::stats_page(pool, Utc::now().date_naive())
        .await
        .map_err(db_error)?;
    PAGE.put(fresh.clone());
    Ok(fresh)
}
