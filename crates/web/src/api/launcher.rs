//! Public, declarative native installer offers. This endpoint never exposes local installation state.

use dioxus::prelude::*;
use recomphub_shared::{InstallOffer, InstallOffersQuery, Page};

#[cfg(feature = "server")]
use crate::server::session::{db_error, http_error};
#[cfg(feature = "server")]
use crate::server::state::pool;
#[cfg(feature = "server")]
use dioxus::server::axum::http::StatusCode;
#[cfg(feature = "server")]
use std::collections::HashMap;

#[cfg(feature = "server")]
const PAGE_SIZE: i64 = 50;

/// Configured packages compatible with the requested native host. An empty result means that no
/// reviewed installer is configured, not that the catalog game is unsupported.
#[post("/api/v1/launcher/offers")]
pub async fn install_offers(
    query: InstallOffersQuery,
) -> Result<Page<InstallOffer>, ServerFnError> {
    query
        .host
        .validate()
        .map_err(|error| bad_request(error.to_string()))?;
    if query.page < 1 {
        return Err(bad_request("page must be positive"));
    }
    if query.port_id.is_some_and(|id| id <= 0) {
        return Err(bad_request("port id must be positive"));
    }

    let pool = pool()?;
    let mut tx = pool.begin().await.map_err(|error| db_error(error.into()))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await
        .map_err(|error| db_error(error.into()))?;
    let builds = recomphub_db::launcher::enabled(&mut *tx)
        .await
        .map_err(db_error)?;
    let reviewed = recomphub_ingest::launcher::recipes()
        .map_err(|error| http_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let builds = select_reviewed_builds(builds, &reviewed, &query);
    let ids: Vec<i64> = builds.iter().map(|build| build.port_id).collect();
    let ports = recomphub_db::ports::published_by_ids(&mut *tx, &ids)
        .await
        .map_err(db_error)?;
    let port_by_id: HashMap<i64, recomphub_db::ports::PortWithGame> =
        ports.into_iter().map(|port| (port.port.id, port)).collect();
    tx.commit().await.map_err(|error| db_error(error.into()))?;
    let mut eligible: Vec<_> = builds
        .into_iter()
        .filter_map(|build| {
            let port = port_by_id.get(&build.port_id)?;
            build_matches_port(&build.manifest, port.port.game_id, &port.port.repo_url)
                .then(|| (build, port.clone()))
        })
        .collect();
    eligible.sort_by(|(left, left_port), (right, right_port)| {
        left_port
            .game_name
            .cmp(&right_port.game_name)
            .then(left_port.port.name.cmp(&right_port.port.name))
            .then(left.manifest.recipe.id.cmp(&right.manifest.recipe.id))
    });

    let mut current_ports: HashMap<i64, recomphub_db::ports::PortWithGame> = HashMap::new();
    for (_, port) in &eligible {
        current_ports
            .entry(port.port.id)
            .or_insert_with(|| port.clone());
    }
    let current_ports: Vec<_> = current_ports.into_values().collect();
    let port_cards = super::catalog::port_cards(pool, current_ports.clone()).await?;
    let game_ids: Vec<i64> = current_ports.iter().map(|port| port.port.game_id).collect();
    let game_cards = super::catalog::game_cards(
        pool,
        &recomphub_db::games::by_ids(pool, &game_ids)
            .await
            .map_err(db_error)?,
    )
    .await?;
    let games: HashMap<i64, _> = game_cards.into_iter().map(|game| (game.id, game)).collect();
    let cards: HashMap<i64, _> = port_cards.into_iter().map(|port| (port.id, port)).collect();

    let mut offers: Vec<_> = eligible
        .into_iter()
        .filter_map(|(build, port)| {
            let port = cards.get(&port.port.id)?.clone();
            let game = games.get(&build.manifest.game_id)?.clone();
            let offer = InstallOffer {
                game,
                port,
                manifest: build.manifest,
                checked_at: build.checked_at,
            };
            offer.validate().ok()?;
            Some(offer)
        })
        .collect();
    offers.sort_by(|left, right| {
        left.game
            .name
            .cmp(&right.game.name)
            .then(left.port.name.cmp(&right.port.name))
            .then(left.manifest.recipe.id.cmp(&right.manifest.recipe.id))
    });
    let total = offers.len() as i64;
    let Some((start, end, pages)) = page_bounds(total, query.page) else {
        return Err(http_error(StatusCode::NOT_FOUND, "page"));
    };
    Ok(Page {
        items: offers.into_iter().skip(start).take(end - start).collect(),
        page: query.page,
        pages,
        total,
    })
}

#[cfg(feature = "server")]
fn select_reviewed_builds(
    builds: Vec<recomphub_db::launcher::LauncherBuild>,
    reviewed: &[recomphub_shared::InstallRecipe],
    query: &InstallOffersQuery,
) -> Vec<recomphub_db::launcher::LauncherBuild> {
    builds
        .into_iter()
        .filter(|build| {
            build.enabled
                && build.recipe_id == build.manifest.recipe.id
                && build.port_id == build.manifest.port_id
                && query.port_id.is_none_or(|id| id == build.port_id)
                && reviewed
                    .iter()
                    .any(|recipe| recipe == &build.manifest.recipe)
                && build.manifest.matches_host(&query.host).unwrap_or(false)
        })
        .collect()
}

#[cfg(feature = "server")]
fn page_bounds(total: i64, page: i64) -> Option<(usize, usize, i64)> {
    let pages = ((total + PAGE_SIZE - 1) / PAGE_SIZE).max(1);
    if page > pages {
        return None;
    }
    let start = ((page - 1) * PAGE_SIZE) as usize;
    Some((
        start,
        (start + PAGE_SIZE as usize).min(total as usize),
        pages,
    ))
}

#[cfg(feature = "server")]
fn build_matches_port(
    manifest: &recomphub_shared::InstallManifest,
    port_game_id: i64,
    port_repo_url: &str,
) -> bool {
    manifest.game_id == port_game_id
        && recomphub_shared::parse_github_repository_url(&manifest.recipe.repo_url).is_ok_and(
            |reviewed| {
                recomphub_shared::parse_github_repository_url(port_repo_url)
                    .is_ok_and(|current| reviewed.matches(&current))
            },
        )
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use chrono::Utc;
    use recomphub_shared::{CpuArch, HostOs, HostPlatform, InstallManifest};

    #[test]
    fn current_recipe_selection_filters_exact_recipe_and_pages_more_than_fifty() {
        let base = recomphub_ingest::launcher::recipes().unwrap()[0].clone();
        let mut reviewed = Vec::new();
        let mut builds = Vec::new();
        for index in 0..51 {
            let mut recipe = base.clone();
            recipe.id = format!("fixture-{index}");
            reviewed.push(recipe.clone());
            let asset_name = recipe.expected_asset_name("v1.0.0").unwrap();
            builds.push(recomphub_db::launcher::LauncherBuild {
                recipe_id: recipe.id.clone(),
                port_id: index + 1,
                manifest: InstallManifest {
                    schema_version: 1,
                    recipe,
                    port_id: index + 1,
                    game_id: 1,
                    release_id: 1,
                    asset_id: 1,
                    tag: "v1.0.0".into(),
                    version: "1.0.0".into(),
                    published_at: Utc::now(),
                    release_url: "https://github.com/banjorecomp/banjorecomp/releases/tag/v1.0.0".into(),
                    asset_name: asset_name.clone(),
                    download_url: format!("https://github.com/banjorecomp/banjorecomp/releases/download/v1.0.0/{asset_name}"),
                    size_bytes: 1,
                    sha256: "ab".repeat(32),
                },
                enabled: true,
                checked_at: Utc::now(),
            });
        }
        let mut stale = builds[0].clone();
        stale
            .manifest
            .recipe
            .requirements
            .push("Unreviewed change".into());
        builds.push(stale);
        let query = InstallOffersQuery {
            host: HostPlatform {
                os: HostOs::Macos,
                arch: CpuArch::Aarch64,
                os_version: "14.0".into(),
            },
            page: 1,
            port_id: None,
        };
        let selected = select_reviewed_builds(builds, &reviewed, &query);
        assert_eq!(selected.len(), 51);
        assert_eq!(page_bounds(selected.len() as i64, 1), Some((0, 50, 2)));
        assert_eq!(page_bounds(selected.len() as i64, 2), Some((50, 51, 2)));
        assert_eq!(page_bounds(selected.len() as i64, 3), None);
        let offer_manifest = &selected[0].manifest;
        assert!(build_matches_port(
            offer_manifest,
            1,
            "https://github.com/banjorecomp/banjorecomp"
        ));
        assert!(!build_matches_port(
            offer_manifest,
            2,
            "https://github.com/banjorecomp/banjorecomp"
        ));
        assert!(!build_matches_port(
            offer_manifest,
            1,
            "https://github.com/other/game"
        ));
    }
}

#[cfg(feature = "server")]
fn bad_request(message: impl Into<String>) -> ServerFnError {
    http_error(StatusCode::BAD_REQUEST, message)
}
