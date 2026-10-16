//! Platform parity for `/stats`: how much of the ported catalog runs natively on each major
//! platform, its daily history, milestones, achievements and leaderboards.
//!
//! A counted port is published and not a decompilation. A "slot" is one (ported game, major
//! platform) pair and parity is covered slots over all slots. History is keyed by the UTC day a
//! port was added to RecompHub. The daily snapshot records what it observes; the backfill
//! reconstructs earlier days from creation dates and never overwrites an existing row.

use std::collections::{BTreeMap, HashMap};

use chrono::{Duration, NaiveDate, Utc};
use recomphub_shared::{
    AchievementDto, AuthorRankDto, ContributorDto, GameGapDto, HistoryPointDto, MilestoneDto,
    MostCoveredDto, OriginDto, ParityDto, ParityTargetDto, StatsPageDto, TargetPlatform,
    TargetRefDto, UnlockDto, UnportedGameDto,
};

use crate::{DbResult, PgConnection, PgExecutor, PgPool, games};

pub use recomphub_shared::MAJOR_TARGETS;
/// Length of the "this month" window and of the trend comparison.
pub const PERIOD_DAYS: i64 = 30;
/// Original platforms need at least this many games to appear in the origin ranking.
const MIN_ORIGIN_GAMES: i64 = 10;
/// Fixed milestone steps for port and slot counts; beyond the last step, every 1,000.
const FIXED_STEPS: [i64; 7] = [10, 25, 50, 100, 250, 500, 1_000];
/// Parity percentage milestones.
const PARITY_STEPS: [i64; 5] = [10, 25, 50, 75, 100];

const TOTAL_KEY: &str = "all";
const M_PORTS: &str = "ports";
const M_PORTED_GAMES: &str = "ported_games";
const M_CATALOG_GAMES: &str = "catalog_games";
const M_TARGET_PORTS: &str = "target_ports";
const M_TARGET_GAMES: &str = "target_games";
const SNAPSHOT: &str = "snapshot";
const BACKFILL: &str = "backfill";
const INSERT_CHUNK: usize = 2_000;

pub fn is_major(key: &str) -> bool {
    MAJOR_TARGETS.contains(&key)
}

/// The smallest milestone strictly above `count`.
pub fn next_milestone(count: i64) -> i64 {
    FIXED_STEPS
        .iter()
        .copied()
        .find(|&t| t > count)
        .unwrap_or((count / 1_000 + 1) * 1_000)
}

/// Every milestone reached when a counter moves from `prev` up to `cur`.
pub fn thresholds_between(prev: i64, cur: i64) -> Vec<i64> {
    let mut out = Vec::new();
    let mut t = next_milestone(prev);
    while t <= cur {
        out.push(t);
        t = next_milestone(t);
    }
    out
}

/// The next parity percentage above `pct`, or `None` at 100.
pub fn next_parity(pct: f64) -> Option<i64> {
    PARITY_STEPS.iter().copied().find(|&t| pct < t as f64)
}

fn ratio_pct(part: i64, whole: i64) -> f64 {
    if whole <= 0 {
        0.0
    } else {
        part as f64 * 100.0 / whole as f64
    }
}

fn target_ref(key: &str) -> TargetRefDto {
    match key.parse::<TargetPlatform>() {
        Ok(t) => TargetRefDto {
            slug: t.slug().to_string(),
            label: t.label().to_string(),
        },
        Err(_) => TargetRefDto {
            slug: key.to_string(),
            label: key.to_string(),
        },
    }
}

/// One stored or computed counter value.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Metric {
    pub metric: String,
    pub key: String,
    pub value: i64,
}

fn metric(name: &str, key: &str, value: i64) -> Metric {
    Metric {
        metric: name.to_string(),
        key: key.to_string(),
        value,
    }
}

/// A counted port: the day it was added, its game and the platforms it runs on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortCoverage {
    pub game_id: i64,
    pub added: NaiveDate,
    /// Platform keys (`TargetPlatform::as_str`), distinct.
    pub targets: Vec<&'static str>,
}

/// Every counter as a function of the day, built from creation dates.
#[derive(Debug, Default, Clone)]
pub struct Timeline {
    ports: Vec<NaiveDate>,
    ported_games: Vec<NaiveDate>,
    catalog_games: Vec<NaiveDate>,
    target_ports: BTreeMap<&'static str, Vec<NaiveDate>>,
    target_games: BTreeMap<&'static str, Vec<NaiveDate>>,
}

impl Timeline {
    pub fn build(ports: &[PortCoverage], catalog: &[NaiveDate]) -> Self {
        let mut t = Self::default();
        for target in TargetPlatform::ALL {
            t.target_ports.insert(target.as_str(), Vec::new());
            t.target_games.insert(target.as_str(), Vec::new());
        }
        let mut game_first: HashMap<i64, NaiveDate> = HashMap::new();
        let mut pair_first: HashMap<(i64, &'static str), NaiveDate> = HashMap::new();
        for port in ports {
            t.ports.push(port.added);
            game_first
                .entry(port.game_id)
                .and_modify(|d| *d = (*d).min(port.added))
                .or_insert(port.added);
            let mut keys = port.targets.clone();
            keys.sort_unstable();
            keys.dedup();
            for key in keys {
                t.target_ports.entry(key).or_default().push(port.added);
                pair_first
                    .entry((port.game_id, key))
                    .and_modify(|d| *d = (*d).min(port.added))
                    .or_insert(port.added);
            }
        }
        t.ported_games = game_first.into_values().collect();
        for ((_, key), day) in pair_first {
            t.target_games.entry(key).or_default().push(day);
        }
        t.catalog_games = catalog.to_vec();
        t.ports.sort_unstable();
        t.ported_games.sort_unstable();
        t.catalog_games.sort_unstable();
        for dates in t
            .target_ports
            .values_mut()
            .chain(t.target_games.values_mut())
        {
            dates.sort_unstable();
        }
        t
    }

    /// The earliest day with anything to record.
    pub fn first_day(&self) -> Option<NaiveDate> {
        [self.ports.first(), self.catalog_games.first()]
            .into_iter()
            .flatten()
            .min()
            .copied()
    }

    /// Every counter as it stood at the end of `day`.
    pub fn values_on(&self, day: NaiveDate) -> Vec<Metric> {
        let count = |dates: &[NaiveDate]| dates.partition_point(|d| *d <= day) as i64;
        let mut out = vec![
            metric(M_PORTS, TOTAL_KEY, count(&self.ports)),
            metric(M_PORTED_GAMES, TOTAL_KEY, count(&self.ported_games)),
            metric(M_CATALOG_GAMES, TOTAL_KEY, count(&self.catalog_games)),
        ];
        for (key, dates) in &self.target_ports {
            out.push(metric(M_TARGET_PORTS, key, count(dates)));
        }
        for (key, dates) in &self.target_games {
            out.push(metric(M_TARGET_GAMES, key, count(dates)));
        }
        out
    }
}

/// Parity values for one day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetCount {
    pub key: String,
    /// Ported games with a native port on this platform.
    pub games: i64,
    pub ports: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parity {
    pub ports: i64,
    pub ported_games: i64,
    pub catalog_games: i64,
    /// Covered (game, major platform) pairs.
    pub slots_covered: i64,
    /// One entry per platform in `TargetPlatform::ALL` order.
    pub targets: Vec<TargetCount>,
}

impl Parity {
    pub fn from_metrics(values: &[Metric]) -> Self {
        let get = |name: &str, key: &str| {
            values
                .iter()
                .find(|m| m.metric == name && m.key == key)
                .map_or(0, |m| m.value)
        };
        let targets: Vec<TargetCount> = TargetPlatform::ALL
            .iter()
            .map(|t| TargetCount {
                key: t.as_str().to_string(),
                games: get(M_TARGET_GAMES, t.as_str()),
                ports: get(M_TARGET_PORTS, t.as_str()),
            })
            .collect();
        let slots_covered = targets
            .iter()
            .filter(|t| is_major(&t.key))
            .map(|t| t.games)
            .sum();
        Self {
            ports: get(M_PORTS, TOTAL_KEY),
            ported_games: get(M_PORTED_GAMES, TOTAL_KEY),
            catalog_games: get(M_CATALOG_GAMES, TOTAL_KEY),
            slots_covered,
            targets,
        }
    }

    pub fn slots_total(&self) -> i64 {
        self.ported_games * MAJOR_TARGETS.len() as i64
    }

    pub fn pct(&self) -> f64 {
        ratio_pct(self.slots_covered, self.slots_total())
    }

    /// Share of ported games with a native port on one platform.
    pub fn pct_of(&self, games: i64) -> f64 {
        ratio_pct(games, self.ported_games)
    }
}

/// Platform ranks by share of ported games, ties broken by key.
fn ranks(parity: &Parity) -> HashMap<String, i64> {
    let mut order: Vec<(&str, f64)> = parity
        .targets
        .iter()
        .map(|t| (t.key.as_str(), parity.pct_of(t.games)))
        .collect();
    order.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
    order
        .into_iter()
        .enumerate()
        .map(|(i, (key, _))| (key.to_string(), i as i64 + 1))
        .collect()
}

/// The three counters closest to their next milestone, by share of the way there.
pub fn unlocks(parity: &Parity) -> Vec<UnlockDto> {
    let mut candidates: Vec<(Option<&str>, &str, i64)> = vec![
        (None, "ports", parity.ports),
        (None, "slots", parity.slots_covered),
    ];
    for t in &parity.targets {
        candidates.push((Some(t.key.as_str()), "ports", t.ports));
    }
    let mut out: Vec<UnlockDto> = candidates
        .into_iter()
        .map(|(key, kind, current)| {
            let next = next_milestone(current);
            UnlockDto {
                target: key.map(target_ref),
                kind: kind.to_string(),
                current,
                next,
                to_go: next - current,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        let fa = a.to_go as f64 / a.next as f64;
        let fb = b.to_go as f64 / b.next as f64;
        fa.total_cmp(&fb).then(b.current.cmp(&a.current))
    });
    out.truncate(3);
    out
}

/// Milestones crossed by a series of `(day, value)` points, starting from zero.
fn crossings(points: &[(NaiveDate, i64)]) -> Vec<(NaiveDate, i64)> {
    let mut prev = 0;
    let mut out = Vec::new();
    for &(day, value) in points {
        for threshold in thresholds_between(prev, value) {
            out.push((day, threshold));
        }
        prev = prev.max(value);
    }
    out
}

/// One milestone series: (kind, optional target slug) to dated values.
type Series = BTreeMap<(&'static str, Option<String>), Vec<(NaiveDate, i64)>>;

/// Every milestone crossed in the recorded history, newest first.
pub fn milestone_events(days: &[(NaiveDate, Vec<Metric>)]) -> Vec<MilestoneDto> {
    let mut series: Series = BTreeMap::new();
    for (day, values) in days {
        let mut slots = 0;
        for m in values {
            match m.metric.as_str() {
                M_PORTS => series
                    .entry(("ports", None))
                    .or_default()
                    .push((*day, m.value)),
                M_TARGET_PORTS => series
                    .entry(("ports", Some(m.key.clone())))
                    .or_default()
                    .push((*day, m.value)),
                M_TARGET_GAMES if is_major(&m.key) => slots += m.value,
                _ => {}
            }
        }
        series
            .entry(("slots", None))
            .or_default()
            .push((*day, slots));
    }
    let mut out = Vec::new();
    for ((kind, key), points) in series {
        for (day, threshold) in crossings(&points) {
            out.push(MilestoneDto {
                day,
                target: key.as_deref().map(target_ref),
                kind: kind.to_string(),
                threshold,
            });
        }
    }
    out.sort_by(|a, b| {
        b.day
            .cmp(&a.day)
            .then(a.kind.cmp(&b.kind))
            .then(a.threshold.cmp(&b.threshold))
    });
    out
}

/// Facts about one port of an author, used for achievements.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct PortFacts {
    pub platforms: Vec<String>,
    pub github_stars: i64,
    pub ratings: i64,
    pub avg_rating: f64,
    /// Earliest counted port of its game on RecompHub.
    pub first_for_game: bool,
    /// Earliest counted port of its game that runs on Linux.
    pub first_linux_for_game: bool,
}

fn badge(key: &str, title: &str, description: &str) -> AchievementDto {
    AchievementDto {
        key: key.to_string(),
        title: title.to_string(),
        description: description.to_string(),
    }
}

/// Achievements earned from an author's counted ports. Pure: derived only from port facts.
pub fn achievements(ports: &[PortFacts]) -> Vec<AchievementDto> {
    if ports.is_empty() {
        return Vec::new();
    }
    let platform_count = |p: &PortFacts| {
        p.platforms
            .iter()
            .filter(|x| x.parse::<TargetPlatform>().is_ok())
            .count()
    };
    let covers_majors = |p: &PortFacts| {
        MAJOR_TARGETS
            .iter()
            .all(|k| p.platforms.iter().any(|x| x == k))
    };
    let mut out = vec![badge(
        "first_port",
        "First port",
        "Published a port on RecompHub.",
    )];
    if ports.len() >= 5 {
        out.push(badge(
            "prolific",
            "Prolific",
            "Published five or more ports.",
        ));
    }
    if ports.iter().any(|p| platform_count(p) >= 3) {
        out.push(badge(
            "platform_hopper",
            "Platform hopper",
            "A port runs on three or more platforms.",
        ));
    }
    if ports.iter().any(|p| p.first_linux_for_game) {
        out.push(badge(
            "penguin_pioneer",
            "Penguin pioneer",
            "First native Linux build of a game on RecompHub.",
        ));
    }
    if ports.iter().any(|p| p.first_for_game) {
        out.push(badge(
            "open_door",
            "Open door",
            "First port of a game on RecompHub.",
        ));
    }
    if ports.iter().any(covers_majors) {
        out.push(badge(
            "full_house",
            "Full house",
            "A port runs on all six major platforms: Windows, macOS, Linux, Web, Android and iOS.",
        ));
    }
    if ports
        .iter()
        .any(|p| p.github_stars >= 100 || (p.ratings >= 3 && p.avg_rating >= 4.5))
    {
        out.push(badge(
            "fan_favourite",
            "Fan favourite",
            "A port has 100+ GitHub stars, or 3+ ratings averaging 4.5 or higher.",
        ));
    }
    out
}

const PORT_COUNT_FILTER: &str = "p.status = 'published' AND p.kind <> 'decompilation'";

/// Counted ports with their added day and platform keys.
pub async fn load_coverage(ex: impl PgExecutor<'_>) -> DbResult<Vec<PortCoverage>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        game_id: i64,
        added: NaiveDate,
        platforms: Vec<String>,
    }
    let sql = format!(
        "SELECT p.game_id, (COALESCE(p.repo_created_at, p.created_at) AT TIME ZONE 'UTC')::date AS added,
                port_platforms(p) AS platforms
         FROM ports p
         WHERE {PORT_COUNT_FILTER}"
    );
    let rows = sqlx::query_as::<_, Row>(&sql).fetch_all(ex).await?;
    Ok(rows
        .into_iter()
        .map(|r| PortCoverage {
            game_id: r.game_id,
            added: r.added,
            targets: r
                .platforms
                .iter()
                .filter_map(|p| p.parse::<TargetPlatform>().ok())
                .map(|t| t.as_str())
                .collect(),
        })
        .collect())
}

async fn load_catalog_dates(ex: impl PgExecutor<'_>) -> DbResult<Vec<NaiveDate>> {
    Ok(sqlx::query_scalar::<_, NaiveDate>(
        "SELECT (created_at AT TIME ZONE 'UTC')::date FROM games",
    )
    .fetch_all(ex)
    .await?)
}

async fn store_rows(
    conn: &mut PgConnection,
    rows: &[(NaiveDate, Metric)],
    source: &str,
    overwrite: bool,
) -> DbResult<u64> {
    let conflict = if overwrite {
        "DO UPDATE SET value = EXCLUDED.value, source = EXCLUDED.source, captured_at = now()"
    } else {
        "DO NOTHING"
    };
    let sql = format!(
        "INSERT INTO stats_daily (day, metric, key, value, source)
         SELECT d, m, k, v, $5
         FROM UNNEST($1::date[], $2::text[], $3::text[], $4::bigint[]) AS t(d, m, k, v)
         ON CONFLICT (day, metric, key) {conflict}"
    );
    let mut written = 0;
    for chunk in rows.chunks(INSERT_CHUNK) {
        let days: Vec<NaiveDate> = chunk.iter().map(|(d, _)| *d).collect();
        let metrics: Vec<String> = chunk.iter().map(|(_, m)| m.metric.clone()).collect();
        let keys: Vec<String> = chunk.iter().map(|(_, m)| m.key.clone()).collect();
        let values: Vec<i64> = chunk.iter().map(|(_, m)| m.value).collect();
        written += sqlx::query(&sql)
            .bind(days)
            .bind(metrics)
            .bind(keys)
            .bind(values)
            .bind(source)
            .execute(&mut *conn)
            .await?
            .rows_affected();
    }
    Ok(written)
}

/// What one snapshot run stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotReport {
    pub day: NaiveDate,
    /// Rows written for `day` (inserted or refreshed).
    pub snapshot_rows: u64,
    /// First reconstructed day, if any past days were missing.
    pub backfill_from: Option<NaiveDate>,
    /// Past days covered by the backfill range, `backfill_from..day`.
    pub backfill_days: i64,
    /// Rows written by the backfill after clearing the previous backfill rows.
    pub backfill_rows: u64,
}

/// Records today's counters, then rebuilds every earlier day from the timeline. Snapshot rows
/// are kept; backfill rows are derived, so each run replaces them.
pub async fn run_snapshot(pool: &PgPool, today: NaiveDate) -> DbResult<SnapshotReport> {
    let coverage = load_coverage(pool).await?;
    let catalog = load_catalog_dates(pool).await?;
    let timeline = Timeline::build(&coverage, &catalog);
    let mut tx = pool.begin().await?;
    let today_rows: Vec<(NaiveDate, Metric)> = timeline
        .values_on(today)
        .into_iter()
        .map(|m| (today, m))
        .collect();
    let snapshot_rows = store_rows(&mut tx, &today_rows, SNAPSHOT, true).await?;
    let mut report = SnapshotReport {
        day: today,
        snapshot_rows,
        backfill_from: None,
        backfill_days: 0,
        backfill_rows: 0,
    };
    sqlx::query("DELETE FROM stats_daily WHERE source = $1")
        .bind(BACKFILL)
        .execute(&mut *tx)
        .await?;
    if let Some(start) = timeline.first_day().filter(|s| *s < today) {
        let rows: Vec<(NaiveDate, Metric)> = start
            .iter_days()
            .take_while(|d| *d < today)
            .flat_map(|day| timeline.values_on(day).into_iter().map(move |m| (day, m)))
            .collect();
        report.backfill_from = Some(start);
        report.backfill_days = (today - start).num_days();
        report.backfill_rows = store_rows(&mut tx, &rows, BACKFILL, false).await?;
    }
    tx.commit().await?;
    Ok(report)
}

/// `run_snapshot` for the current UTC day.
pub async fn run_snapshot_now(pool: &PgPool) -> DbResult<SnapshotReport> {
    run_snapshot(pool, Utc::now().date_naive()).await
}

async fn stored_on(ex: impl PgExecutor<'_>, day: NaiveDate) -> DbResult<Vec<Metric>> {
    Ok(
        sqlx::query_as::<_, Metric>("SELECT metric, key, value FROM stats_daily WHERE day = $1")
            .bind(day)
            .fetch_all(ex)
            .await?,
    )
}

async fn stored_history(ex: impl PgExecutor<'_>) -> DbResult<Vec<(NaiveDate, Vec<Metric>)>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        day: NaiveDate,
        metric: String,
        key: String,
        value: i64,
    }
    let rows = sqlx::query_as::<_, Row>(
        "SELECT day, metric, key, value FROM stats_daily ORDER BY day, metric, key",
    )
    .fetch_all(ex)
    .await?;
    let mut by_day: BTreeMap<NaiveDate, Vec<Metric>> = BTreeMap::new();
    for r in rows {
        by_day.entry(r.day).or_default().push(Metric {
            metric: r.metric,
            key: r.key,
            value: r.value,
        });
    }
    Ok(by_day.into_iter().collect())
}

/// Current counters, the same counters a period ago, and the stored values from then if any.
struct Built {
    today: NaiveDate,
    now: Vec<Metric>,
    then: Vec<Metric>,
    past: Option<Vec<Metric>>,
}

impl Built {
    async fn load(pool: &PgPool, today: NaiveDate) -> DbResult<Self> {
        let coverage = load_coverage(pool).await?;
        let catalog = load_catalog_dates(pool).await?;
        let timeline = Timeline::build(&coverage, &catalog);
        let past_day = today - Duration::days(PERIOD_DAYS);
        let stored = stored_on(pool, past_day).await?;
        Ok(Self {
            today,
            now: timeline.values_on(today),
            then: timeline.values_on(past_day),
            past: (!stored.is_empty()).then_some(stored),
        })
    }
}

async fn period_contributors(
    ex: impl PgExecutor<'_>,
    since: NaiveDate,
    limit: i64,
) -> DbResult<Vec<ContributorDto>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        login: String,
        name: String,
        count: i64,
    }
    let rows = sqlx::query_as::<_, Row>(
        "WITH pairs AS (
            SELECT DISTINCT ON (p.game_id, t.target)
                   p.id AS port_id, (p.created_at AT TIME ZONE 'UTC')::date AS added
            FROM ports p
            CROSS JOIN LATERAL unnest(port_platforms(p)) AS t(target)
            WHERE p.status = 'published' AND p.kind <> 'decompilation'
              AND t.target = ANY($1::text[])
            ORDER BY p.game_id, t.target, p.created_at, p.id
         )
         SELECT lower(pa.github_login) AS login,
                COALESCE(max(pa.name), lower(pa.github_login)) AS name,
                count(*)::bigint AS count
         FROM pairs pr
         JOIN port_authors pa ON pa.port_id = pr.port_id
         WHERE pr.added > $2 AND pa.github_login IS NOT NULL
         GROUP BY lower(pa.github_login)
         ORDER BY count DESC, login
         LIMIT $3",
    )
    .bind(MAJOR_TARGETS.to_vec())
    .bind(since)
    .bind(limit)
    .fetch_all(ex)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| ContributorDto {
            login: r.login,
            name: r.name,
            count: r.count,
        })
        .collect())
}

async fn parity_dto(pool: &PgPool, built: &Built) -> DbResult<ParityDto> {
    let now = Parity::from_metrics(&built.now);
    let then = Parity::from_metrics(&built.then);
    let past = built.past.as_deref().map(Parity::from_metrics);
    let rank_now = ranks(&now);
    let rank_past = past.as_ref().map(ranks);
    let mut targets: Vec<ParityTargetDto> = now
        .targets
        .iter()
        .map(|t| {
            let target = target_ref(&t.key);
            ParityTargetDto {
                slug: target.slug,
                label: target.label,
                games: t.games,
                ports: t.ports,
                pct: now.pct_of(t.games),
                rank: rank_now.get(&t.key).copied().unwrap_or_default(),
                pct_30d: past.as_ref().and_then(|p| {
                    p.targets
                        .iter()
                        .find(|x| x.key == t.key)
                        .map(|x| p.pct_of(x.games))
                }),
                rank_30d: rank_past.as_ref().and_then(|r| r.get(&t.key).copied()),
                gap_link: crate::creators::is_major_target(&t.key),
            }
        })
        .collect();
    targets.sort_by_key(|t| t.rank);
    let contributors =
        period_contributors(pool, built.today - Duration::days(PERIOD_DAYS), 5).await?;
    Ok(ParityDto {
        pct: now.pct(),
        pct_30d: past.as_ref().map(Parity::pct),
        slots_covered: now.slots_covered,
        slots_total: now.slots_total(),
        ported_games: now.ported_games,
        catalog_games: now.catalog_games,
        catalog_pct: ratio_pct(now.ported_games, now.catalog_games),
        catalog_pct_30d: past
            .as_ref()
            .map(|p| ratio_pct(p.ported_games, p.catalog_games)),
        ports: now.ports,
        period_days: PERIOD_DAYS,
        ports_in_period: (now.ports - then.ports).max(0),
        slots_in_period: (now.slots_covered - then.slots_covered).max(0),
        next_parity: next_parity(now.pct()),
        targets,
        contributors,
    })
}

/// Headline parity numbers for the home teaser and `/stats`.
pub async fn coverage_summary(pool: &PgPool, today: NaiveDate) -> DbResult<ParityDto> {
    let built = Built::load(pool, today).await?;
    parity_dto(pool, &built).await
}

async fn author_leaders(
    ex: impl PgExecutor<'_>,
) -> DbResult<(Vec<AuthorRankDto>, Vec<AuthorRankDto>)> {
    #[derive(sqlx::FromRow)]
    struct Row {
        login: String,
        name: String,
        ports: i64,
        platforms: i64,
    }
    let rows = sqlx::query_as::<_, Row>(
        "SELECT lower(pa.github_login) AS login,
                COALESCE(max(pa.name), lower(pa.github_login)) AS name,
                count(DISTINCT p.id)::bigint AS ports,
                count(DISTINCT t.target)::bigint AS platforms
         FROM port_authors pa
         JOIN ports p ON p.id = pa.port_id AND p.status = 'published' AND p.kind <> 'decompilation'
         LEFT JOIN LATERAL (SELECT u.t AS target FROM unnest(port_platforms(p)) AS u(t)) AS t ON true
         WHERE pa.github_login IS NOT NULL
         GROUP BY lower(pa.github_login)",
    )
    .fetch_all(ex)
    .await?;
    let to_dto = |r: &Row| AuthorRankDto {
        login: r.login.clone(),
        name: r.name.clone(),
        ports: r.ports,
        platforms: r.platforms,
    };
    let mut by_ports: Vec<AuthorRankDto> = rows.iter().map(to_dto).collect();
    by_ports.sort_by(|a, b| {
        b.ports
            .cmp(&a.ports)
            .then(b.platforms.cmp(&a.platforms))
            .then(a.login.cmp(&b.login))
    });
    by_ports.truncate(10);
    let mut by_platforms: Vec<AuthorRankDto> = rows
        .iter()
        .filter(|r| r.platforms > 0)
        .map(to_dto)
        .collect();
    by_platforms.sort_by(|a, b| {
        b.platforms
            .cmp(&a.platforms)
            .then(b.ports.cmp(&a.ports))
            .then(a.login.cmp(&b.login))
    });
    by_platforms.truncate(10);
    Ok((by_ports, by_platforms))
}

async fn game_leaders(
    pool: &PgPool,
) -> DbResult<(Vec<MostCoveredDto>, Vec<GameGapDto>, Vec<UnportedGameDto>)> {
    #[derive(sqlx::FromRow)]
    struct Row {
        slug: String,
        name: String,
        ports: i64,
        covered: Vec<String>,
    }
    let rows = sqlx::query_as::<_, Row>(
        "SELECT g.slug, g.name,
                count(DISTINCT p.id)::bigint AS ports,
                COALESCE(array_agg(DISTINCT t.target) FILTER (WHERE t.target IS NOT NULL), '{}') AS covered
         FROM games g
         JOIN ports p ON p.game_id = g.id AND p.status = 'published' AND p.kind <> 'decompilation'
         LEFT JOIN LATERAL (
            SELECT u.t AS target FROM unnest(port_platforms(p)) AS u(t) WHERE u.t = ANY($1::text[])
         ) AS t ON true
         GROUP BY g.id
         ORDER BY g.importance DESC, g.id",
    )
    .bind(MAJOR_TARGETS.to_vec())
    .fetch_all(pool)
    .await?;

    let mut most: Vec<MostCoveredDto> = rows
        .iter()
        .filter_map(|r| {
            let covered = r.covered.iter().filter(|c| is_major(c)).count() as i64;
            (covered > 0).then(|| MostCoveredDto {
                slug: r.slug.clone(),
                name: r.name.clone(),
                ports: r.ports,
                covered,
            })
        })
        .collect();
    most.sort_by(|a, b| {
        b.covered
            .cmp(&a.covered)
            .then(b.ports.cmp(&a.ports))
            .then(a.name.cmp(&b.name))
    });
    most.truncate(8);

    let gaps: Vec<GameGapDto> = rows
        .iter()
        .filter(|r| r.covered.iter().filter(|c| is_major(c)).count() < MAJOR_TARGETS.len())
        .take(10)
        .map(|r| GameGapDto {
            slug: r.slug.clone(),
            name: r.name.clone(),
            ports: r.ports,
            missing: MAJOR_TARGETS
                .iter()
                .filter(|k| !r.covered.iter().any(|c| c == *k))
                .map(|k| target_ref(k))
                .collect(),
        })
        .collect();

    let wanted: Vec<UnportedGameDto> = sqlx::query_as::<_, (String, String)>(
        "SELECT g.slug, g.name FROM games g
         WHERE NOT EXISTS (
            SELECT 1 FROM ports p
            WHERE p.game_id = g.id AND p.status = 'published' AND p.kind <> 'decompilation'
         )
         ORDER BY g.importance DESC, g.id
         LIMIT 10",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(slug, name)| UnportedGameDto { slug, name })
    .collect();

    Ok((most, gaps, wanted))
}

async fn origin_platforms(ex: impl PgExecutor<'_>) -> DbResult<Vec<OriginDto>> {
    let hubs = games::platform_hubs(ex, false).await?;
    let mut origins: Vec<OriginDto> = hubs
        .into_iter()
        .filter(|h| h.game_count >= MIN_ORIGIN_GAMES)
        .filter_map(|h| {
            let slug = h.platform.slug?;
            Some(OriginDto {
                slug,
                name: h.platform.name,
                game_count: h.game_count,
                ported_count: h.ported_count,
                pct: ratio_pct(h.ported_count, h.game_count),
            })
        })
        .collect();
    origins.sort_by(|a, b| {
        b.pct
            .total_cmp(&a.pct)
            .then(b.game_count.cmp(&a.game_count))
            .then(a.name.cmp(&b.name))
    });
    origins.truncate(10);
    Ok(origins)
}

/// Everything on `/stats`.
pub async fn stats_page(pool: &PgPool, today: NaiveDate) -> DbResult<StatsPageDto> {
    let built = Built::load(pool, today).await?;
    let parity = parity_dto(pool, &built).await?;
    let mut days = stored_history(pool).await?;
    if days.last().is_none_or(|(d, _)| *d < today) {
        days.push((today, built.now.clone()));
    }
    let history: Vec<HistoryPointDto> = days
        .iter()
        .map(|(day, values)| {
            let p = Parity::from_metrics(values);
            HistoryPointDto {
                day: *day,
                pct: p.pct(),
                slots: p.slots_covered,
                ported_games: p.ported_games,
            }
        })
        .collect();
    let mut milestones = milestone_events(&days);
    milestones.truncate(12);
    let (top_authors, top_platform_authors) = author_leaders(pool).await?;
    let (most_covered, gaps, wanted) = game_leaders(pool).await?;
    Ok(StatsPageDto {
        parity,
        history,
        milestones,
        unlocks: unlocks(&Parity::from_metrics(&built.now)),
        top_authors,
        top_platform_authors,
        most_covered,
        gaps,
        wanted,
        origins: origin_platforms(pool).await?,
    })
}

/// Achievements for an author, from facts about their counted ports.
pub async fn author_achievements(
    ex: impl PgExecutor<'_>,
    login: &str,
) -> DbResult<Vec<AchievementDto>> {
    let sql = format!(
        "WITH base AS (
            SELECT p.id, p.game_id, p.created_at, port_platforms(p) AS platforms, p.github_stars
            FROM ports p
            WHERE {PORT_COUNT_FILTER}
         ),
         first_game AS (
            SELECT DISTINCT ON (game_id) id FROM base ORDER BY game_id, created_at, id
         ),
         first_linux AS (
            SELECT DISTINCT ON (game_id) id FROM base
            WHERE 'linux' = ANY(platforms)
            ORDER BY game_id, created_at, id
         ),
         ratings AS (
            SELECT port_id, count(*) AS n, avg(stars)::float8 AS avg
            FROM port_ratings GROUP BY port_id
         )
         SELECT b.platforms, b.github_stars::bigint AS github_stars,
                COALESCE(r.n, 0)::bigint AS ratings,
                COALESCE(r.avg, 0)::float8 AS avg_rating,
                fg.id IS NOT NULL AS first_for_game,
                fl.id IS NOT NULL AS first_linux_for_game
         FROM base b
         JOIN port_authors pa ON pa.port_id = b.id
         LEFT JOIN ratings r ON r.port_id = b.id
         LEFT JOIN first_game fg ON fg.id = b.id
         LEFT JOIN first_linux fl ON fl.id = b.id
         WHERE lower(pa.github_login) = lower($1)
         ORDER BY b.id"
    );
    let ports = sqlx::query_as::<_, PortFacts>(&sql)
        .bind(login)
        .fetch_all(ex)
        .await?;
    Ok(achievements(&ports))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(n: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 1, n).unwrap()
    }

    fn port(game_id: i64, added: NaiveDate, targets: &[&'static str]) -> PortCoverage {
        PortCoverage {
            game_id,
            added,
            targets: targets.to_vec(),
        }
    }

    fn facts(platforms: &[&str]) -> PortFacts {
        PortFacts {
            platforms: platforms.iter().map(|p| p.to_string()).collect(),
            github_stars: 0,
            ratings: 0,
            avg_rating: 0.0,
            first_for_game: false,
            first_linux_for_game: false,
        }
    }

    #[test]
    fn next_milestone_is_strictly_above_count() {
        assert_eq!(next_milestone(0), 10);
        assert_eq!(next_milestone(10), 25);
        assert_eq!(next_milestone(199), 250);
        assert_eq!(next_milestone(1_000), 2_000);
        assert_eq!(next_milestone(2_500), 3_000);
    }

    #[test]
    fn thresholds_between_lists_every_crossing() {
        assert_eq!(thresholds_between(0, 9), Vec::<i64>::new());
        assert_eq!(thresholds_between(9, 25), vec![10, 25]);
        assert_eq!(thresholds_between(24, 24), Vec::<i64>::new());
        assert_eq!(
            thresholds_between(0, 1_000),
            vec![10, 25, 50, 100, 250, 500, 1_000]
        );
    }

    #[test]
    fn next_parity_stops_at_one_hundred() {
        assert_eq!(next_parity(0.0), Some(10));
        assert_eq!(next_parity(9.99), Some(10));
        assert_eq!(next_parity(10.0), Some(25));
        assert_eq!(next_parity(99.9), Some(100));
        assert_eq!(next_parity(100.0), None);
    }

    #[test]
    fn timeline_counts_first_coverage_per_game_and_platform() {
        let ports = [
            port(1, day(1), &["windows", "linux"]),
            port(1, day(3), &["linux", "web"]),
            port(2, day(5), &["windows"]),
        ];
        let catalog = [day(1), day(1), day(2)];
        let t = Timeline::build(&ports, &catalog);
        assert_eq!(t.first_day(), Some(day(1)));

        let on = |d| Parity::from_metrics(&t.values_on(d));
        let p = on(day(3));
        assert_eq!(p.ports, 2);
        assert_eq!(p.ported_games, 1);
        assert_eq!(p.catalog_games, 3);
        // Game 1: windows (day 1), linux (day 1), web (day 3). Game 2 is not yet ported.
        assert_eq!(p.slots_covered, 3);
        assert_eq!(p.slots_total(), 6);

        let end = on(day(5));
        assert_eq!(end.ported_games, 2);
        assert_eq!(end.slots_covered, 4);
        assert_eq!(end.slots_total(), 12);
        let linux = end.targets.iter().find(|t| t.key == "linux").unwrap();
        assert_eq!((linux.games, linux.ports), (1, 2));
    }

    #[test]
    fn duplicate_platforms_on_one_port_count_once() {
        let ports = [port(1, day(1), &["linux", "linux"])];
        let t = Timeline::build(&ports, &[]);
        let p = Parity::from_metrics(&t.values_on(day(1)));
        let linux = p.targets.iter().find(|t| t.key == "linux").unwrap();
        assert_eq!((linux.games, linux.ports), (1, 1));
    }

    #[test]
    fn crossings_do_not_repeat_after_a_dip() {
        let points = [(day(1), 12), (day(2), 9), (day(3), 26)];
        assert_eq!(crossings(&points), vec![(day(1), 10), (day(3), 25)]);
    }

    #[test]
    fn milestone_events_are_newest_first_and_labelled() {
        let days = vec![
            (
                day(1),
                vec![
                    metric(M_PORTS, TOTAL_KEY, 9),
                    metric(M_TARGET_PORTS, "linux", 9),
                ],
            ),
            (
                day(2),
                vec![
                    metric(M_PORTS, TOTAL_KEY, 10),
                    metric(M_TARGET_PORTS, "linux", 10),
                ],
            ),
        ];
        let events = milestone_events(&days);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].day, day(2));
        assert_eq!(events[0].kind, "ports");
        let linux: Vec<_> = events.iter().filter(|e| e.target.is_some()).collect();
        assert_eq!(linux.len(), 1);
        assert_eq!(linux[0].target.as_ref().unwrap().slug, "linux");
    }

    #[test]
    fn unlocks_sort_by_share_of_the_way_there() {
        let mut p = Parity {
            ports: 195,
            ported_games: 0,
            catalog_games: 0,
            slots_covered: 0,
            targets: TargetPlatform::ALL
                .iter()
                .map(|t| TargetCount {
                    key: t.as_str().to_string(),
                    games: 0,
                    ports: 0,
                })
                .collect(),
        };
        let linux = p.targets.iter_mut().find(|t| t.key == "linux").unwrap();
        linux.ports = 240;
        let out = unlocks(&p);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].target.as_ref().unwrap().slug, "linux");
        assert_eq!(out[0].to_go, 10);
        assert_eq!(out[0].next, 250);
    }

    #[test]
    fn achievements_follow_the_port_facts() {
        assert!(achievements(&[]).is_empty());

        let solo = [facts(&["windows"])];
        let keys: Vec<_> = achievements(&solo).into_iter().map(|a| a.key).collect();
        assert_eq!(keys, vec!["first_port"]);

        let hopper = [facts(&["windows", "linux", "web"])];
        let keys: Vec<_> = achievements(&hopper).into_iter().map(|a| a.key).collect();
        assert_eq!(keys, vec!["first_port", "platform_hopper"]);

        let mut full = facts(&["windows", "macos", "linux", "web", "android", "ios"]);
        full.first_linux_for_game = true;
        full.first_for_game = true;
        let keys: Vec<_> = achievements(&[full]).into_iter().map(|a| a.key).collect();
        assert_eq!(
            keys,
            vec![
                "first_port",
                "platform_hopper",
                "penguin_pioneer",
                "open_door",
                "full_house"
            ]
        );

        let prolific: Vec<PortFacts> = (0..5).map(|_| facts(&["windows"])).collect();
        assert!(achievements(&prolific).iter().any(|a| a.key == "prolific"));
    }

    #[test]
    fn fan_favourite_needs_stars_or_rated_quality() {
        let mut starred = facts(&["windows"]);
        starred.github_stars = 100;
        assert!(
            achievements(&[starred])
                .iter()
                .any(|a| a.key == "fan_favourite")
        );

        let mut rated = facts(&["windows"]);
        rated.ratings = 3;
        rated.avg_rating = 4.5;
        assert!(
            achievements(&[rated])
                .iter()
                .any(|a| a.key == "fan_favourite")
        );

        let mut thin = facts(&["windows"]);
        thin.ratings = 2;
        thin.avg_rating = 5.0;
        assert!(
            !achievements(&[thin])
                .iter()
                .any(|a| a.key == "fan_favourite")
        );
    }
}
