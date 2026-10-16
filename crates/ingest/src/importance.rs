//! Sales sync: fetches the Wikipedia best-selling lists, matches their entries to catalogue games,
//! stores one `sales_signals` set per list, and refreshes `games.sales_score`.
//!
//! Matching is local first, against the catalogue names and alternative names. Only a local miss
//! goes to IGDB through [`igdb::match_remote`], which may import the game. Ties between same-named
//! games go to the one with the highest `importance`.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};
use recomphub_db::PgPool;
use recomphub_db::sales::{self, CatalogEntry, SalesSignal};
use recomphub_shared::normalize_title;
use tracing::{debug, warn};

use crate::best_sellers::{self, FetchedList, ListEntry, SourceList};
use crate::igdb::{self, IgdbClient, MatchResult};

/// What one sales sync run did, for logs.
#[derive(Debug, Default)]
pub struct SalesSyncReport {
    pub lists: Vec<ListReport>,
    /// Titles of lists that failed to fetch or sync. Their stored rows are kept.
    pub failed: Vec<String>,
    pub games_rescored: u64,
}

#[derive(Debug, Default)]
pub struct ListReport {
    pub title: &'static str,
    pub entries: usize,
    pub matched_local: usize,
    pub matched_remote: usize,
    pub unmatched: usize,
    pub stored: usize,
}

/// Catalogue lookup by normalized name and alternative names.
struct LocalIndex {
    games: Vec<CatalogEntry>,
    by_title: HashMap<String, Vec<usize>>,
    ids: HashSet<i64>,
}

impl LocalIndex {
    async fn load(pool: &PgPool) -> Result<Self> {
        let games = sales::catalog_index(pool).await?;
        let mut by_title: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, game) in games.iter().enumerate() {
            let names = std::iter::once(&game.name).chain(game.alt_names.iter());
            for name in names {
                let key = normalize_title(name);
                if key.is_empty() {
                    continue;
                }
                let indexes = by_title.entry(key).or_default();
                if !indexes.contains(&index) {
                    indexes.push(index);
                }
            }
        }
        let ids = games.iter().map(|g| g.id).collect();
        Ok(Self {
            games,
            by_title,
            ids,
        })
    }

    /// The game with `target` as a normalized name, restricted to `platform_slug` when given.
    /// Ties go to the highest importance, then the lowest id.
    fn find(&self, target: &str, platform_slug: Option<&str>) -> Option<i64> {
        self.by_title
            .get(target)?
            .iter()
            .map(|&index| &self.games[index])
            .filter(|game| {
                platform_slug.is_none_or(|slug| game.platform_slugs.iter().any(|s| s == slug))
            })
            .max_by_key(|game| (game.importance, std::cmp::Reverse(game.id)))
            .map(|game| game.id)
    }
}

/// Fetches every best-selling list, stores its signals, and refreshes `games.sales_score`.
///
/// A list that fails is recorded in [`SalesSyncReport::failed`] and the others still run. The
/// caller decides whether a partial run is an error.
pub async fn sales_sync(pool: &PgPool, client: &IgdbClient) -> Result<SalesSyncReport> {
    let http = best_sellers::http_client()?;
    let platforms = igdb::resolve_platform_slugs(pool, client).await?;
    let fetched = best_sellers::fetch_all(&http).await;

    let mut index = LocalIndex::load(pool).await?;
    let mut remote_cache: HashMap<(String, Option<&'static str>), Option<i64>> = HashMap::new();
    let mut report = SalesSyncReport::default();

    for FetchedList { list, entries } in fetched {
        let synced = match entries {
            Ok(entries) => {
                sync_list(
                    pool,
                    client,
                    &platforms,
                    &mut index,
                    &mut remote_cache,
                    list,
                    entries,
                )
                .await
            }
            Err(err) => Err(err),
        };
        match synced {
            Ok(list_report) => report.lists.push(list_report),
            Err(err) => {
                warn!(list = list.title, error = %format!("{err:#}"), "best-seller list failed; keeping stored rows");
                report.failed.push(list.title.to_owned());
            }
        }
    }

    report.games_rescored = sales::refresh_sales_scores(pool).await?;
    Ok(report)
}

async fn sync_list(
    pool: &PgPool,
    client: &IgdbClient,
    platforms: &HashMap<String, i32>,
    index: &mut LocalIndex,
    remote_cache: &mut HashMap<(String, Option<&'static str>), Option<i64>>,
    list: SourceList,
    entries: Vec<ListEntry>,
) -> Result<ListReport> {
    // Checked before anything is stored, so an empty parse can never clear the stored rows.
    if entries.is_empty() {
        bail!(
            "{} parsed to zero entries; refusing to clear its stored rows",
            list.title
        );
    }
    let platform_igdb_id = match list.platform_slug {
        Some(slug) => Some(
            *platforms
                .get(slug)
                .with_context(|| format!("no IGDB platform for slug {slug}"))?,
        ),
        None => None,
    };

    let mut report = ListReport {
        title: list.title,
        entries: entries.len(),
        ..ListReport::default()
    };
    let mut signals = Vec::with_capacity(entries.len());
    let source_url = best_sellers::page_url(list.title);

    for entry in entries {
        let target = normalize_title(&entry.title);
        if target.is_empty() {
            report.unmatched += 1;
            continue;
        }

        let game_id = match index.find(&target, list.platform_slug) {
            Some(id) => {
                report.matched_local += 1;
                Some(id)
            }
            None => {
                let key = (target, list.platform_slug);
                let remote = match remote_cache.get(&key) {
                    Some(cached) => *cached,
                    None => {
                        let outcome =
                            match igdb::match_remote(pool, client, &entry.title, platform_igdb_id)
                                .await?
                            {
                                MatchResult::Matched(id) => Some(id),
                                MatchResult::Ambiguous | MatchResult::NotFound => None,
                            };
                        if let Some(id) = outcome
                            && !index.ids.contains(&id)
                        {
                            *index = LocalIndex::load(pool).await?;
                        }
                        remote_cache.insert(key, outcome);
                        outcome
                    }
                };
                if remote.is_some() {
                    report.matched_remote += 1;
                } else {
                    report.unmatched += 1;
                    debug!(list = list.title, title = %entry.title, "no catalogue match");
                }
                remote
            }
        };

        if let Some(game_id) = game_id {
            signals.push(SalesSignal {
                game_id,
                scope: list.scope,
                platform_slug: list.platform_slug.map(str::to_owned),
                sales_year: entry.sales_year,
                rank: entry.rank,
                units_sold_millions: entry.units_sold_millions,
                listed_title: entry.title,
                source_url: source_url.clone(),
            });
        }
    }

    report.stored = sales::replace_list(pool, list.title, &signals).await?;
    Ok(report)
}
