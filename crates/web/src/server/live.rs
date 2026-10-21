//! Live presence: which visitors are on which page right now, the per-connection
//! websocket loop, and the background tasks that publish presence, flush viewer
//! minutes and forward new activity rows.
//!
//! All presence lives in memory. It is correct only with a single web replica.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use chrono::Utc;
use dioxus::fullstack::{HeaderMap, TypedWebsocket};
use dioxus::prelude::ServerFnError;
use recomphub_db::DbResult;
use recomphub_db::activity::{self, ActivityRow};
use recomphub_db::hits;
use recomphub_db::live::flush_viewer_minutes;
use recomphub_db::media::{self, MediaRow};
use recomphub_shared::{ActivityItem, ClientMsg, LiveGame, MediaRef, ServerMsg};
use sqlx::PgPool;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, watch};

use super::session::db_error;
use super::state;

/// A connection with no message for longer than this is dropped.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(70);
/// How long a game's "Live now" card is reused before reloading it from the database.
pub const CARD_TTL: Duration = Duration::from_secs(600);
/// Games in the "Live now" rail.
pub const TOP_N: usize = 12;
/// Most games one connection may ask counts for.
pub const MAX_VISIBLE: usize = 200;
const EVENT_CAPACITY: usize = 256;
const ACTIVITY_BATCH: i64 = 50;

pub type ConnId = u64;

/// The site-wide presence state, published whenever it changes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PresenceSnapshot {
    /// Distinct counted visitors across all connections.
    pub online: u32,
    /// Distinct counted visitors per watched game. Only non-zero entries.
    pub per_game: HashMap<i64, u32>,
    /// Up to [`TOP_N`] games by viewers, using cached cards.
    pub top: Vec<LiveGame>,
}

struct Conn {
    /// `None` for bots: they receive updates but are never counted.
    visitor: Option<Vec<u8>>,
    watching: Option<i64>,
    last_seen: Instant,
}

struct CachedCard {
    card: LiveGame,
    loaded: Instant,
}

/// In-memory state shared by every websocket connection.
pub struct LiveHub {
    conns: Mutex<HashMap<ConnId, Conn>>,
    next_id: AtomicU64,
    events: broadcast::Sender<ServerMsg>,
    presence: watch::Sender<Arc<PresenceSnapshot>>,
    cards: Mutex<HashMap<i64, CachedCard>>,
}

impl Default for LiveHub {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveHub {
    pub fn new() -> Self {
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let (presence, _) = watch::channel(Arc::new(PresenceSnapshot::default()));
        Self {
            conns: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            events,
            presence,
            cards: Mutex::new(HashMap::new()),
        }
    }

    /// Adds a connection. `visitor` is `None` for bot user agents.
    pub fn register(&self, visitor: Option<Vec<u8>>, now: Instant) -> ConnId {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        lock(&self.conns).insert(
            id,
            Conn {
                visitor,
                watching: None,
                last_seen: now,
            },
        );
        id
    }

    pub fn unregister(&self, id: ConnId) {
        lock(&self.conns).remove(&id);
    }

    /// Records a client's `Hello`. Returns the visible list as stored (deduplicated, capped).
    pub fn hello(
        &self,
        id: ConnId,
        watching: Option<i64>,
        visible: Vec<i64>,
        now: Instant,
    ) -> Vec<i64> {
        let visible = normalise_visible(visible);
        let mut conns = lock(&self.conns);
        if let Some(conn) = conns.get_mut(&id) {
            conn.watching = watching;
            conn.last_seen = now;
        }
        visible
    }

    /// Marks any message from the client as proof the connection is alive.
    pub fn touch(&self, id: ConnId, now: Instant) {
        if let Some(conn) = lock(&self.conns).get_mut(&id) {
            conn.last_seen = now;
        }
    }

    /// True when the connection is gone or has been silent longer than [`IDLE_TIMEOUT`].
    pub fn is_idle(&self, id: ConnId, now: Instant) -> bool {
        match lock(&self.conns).get(&id) {
            None => true,
            Some(conn) => now.saturating_duration_since(conn.last_seen) > IDLE_TIMEOUT,
        }
    }

    /// Drops every connection that has been silent longer than [`IDLE_TIMEOUT`].
    pub fn sweep(&self, now: Instant) {
        lock(&self.conns)
            .retain(|_, conn| now.saturating_duration_since(conn.last_seen) <= IDLE_TIMEOUT);
    }

    /// Distinct counted visitors (site-wide) and per watched game.
    pub fn counts(&self) -> (u32, HashMap<i64, u32>) {
        let conns = lock(&self.conns);
        let mut online: HashSet<&[u8]> = HashSet::new();
        let mut per_game: HashMap<i64, HashSet<&[u8]>> = HashMap::new();
        for conn in conns.values() {
            let Some(visitor) = conn.visitor.as_deref() else {
                continue;
            };
            online.insert(visitor);
            if let Some(game) = conn.watching {
                per_game.entry(game).or_default().insert(visitor);
            }
        }
        let per_game = per_game
            .into_iter()
            .map(|(game, visitors)| (game, visitors.len() as u32))
            .collect();
        (online.len() as u32, per_game)
    }

    /// Game ids whose cards are missing or older than [`CARD_TTL`].
    pub fn stale_cards(&self, games: &[i64], now: Instant) -> Vec<i64> {
        let cards = lock(&self.cards);
        games
            .iter()
            .copied()
            .filter(|game| match cards.get(game) {
                None => true,
                Some(cached) => now.saturating_duration_since(cached.loaded) > CARD_TTL,
            })
            .collect()
    }

    pub fn store_cards(&self, cards: Vec<LiveGame>, now: Instant) {
        let mut cache = lock(&self.cards);
        for card in cards {
            cache.insert(card.game_id, CachedCard { card, loaded: now });
        }
    }

    /// Recomputes presence from the live connections and publishes it if it changed.
    pub fn recompute(&self, now: Instant) -> Arc<PresenceSnapshot> {
        self.sweep(now);
        let (online, per_game) = self.counts();
        let top = self.top(&per_game);
        let snapshot = Arc::new(PresenceSnapshot {
            online,
            per_game,
            top,
        });
        self.presence.send_if_modified(|current| {
            if **current == *snapshot {
                false
            } else {
                *current = snapshot.clone();
                true
            }
        });
        snapshot
    }

    fn top(&self, per_game: &HashMap<i64, u32>) -> Vec<LiveGame> {
        let cards = lock(&self.cards);
        let mut ranked: Vec<(i64, u32)> = per_game
            .iter()
            .map(|(game, count)| (*game, *count))
            .filter(|(game, _)| cards.contains_key(game))
            .collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        ranked
            .into_iter()
            .take(TOP_N)
            .filter_map(|(game, viewers)| {
                cards.get(&game).map(|cached| LiveGame {
                    viewers,
                    ..cached.card.clone()
                })
            })
            .collect()
    }

    /// Sends an event to every subscribed connection. Fine with no subscribers.
    pub fn publish(&self, msg: ServerMsg) {
        let _ = self.events.send(msg);
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<ServerMsg> {
        self.events.subscribe()
    }

    pub fn subscribe_presence(&self) -> watch::Receiver<Arc<PresenceSnapshot>> {
        self.presence.subscribe()
    }

    /// The last published presence snapshot.
    pub fn current(&self) -> Arc<PresenceSnapshot> {
        self.presence.borrow().clone()
    }

    /// Live viewers of one game, from the last published snapshot.
    pub fn viewers(&self, game_id: i64) -> u32 {
        self.presence
            .borrow()
            .per_game
            .get(&game_id)
            .copied()
            .unwrap_or(0)
    }

    /// Reloads missing or stale cards from the database, then recomputes presence.
    pub async fn refresh(&self, pool: &PgPool) -> DbResult<()> {
        let now = Instant::now();
        self.sweep(now);
        let (_, per_game) = self.counts();
        let watched: Vec<i64> = per_game.keys().copied().collect();
        let missing = self.stale_cards(&watched, now);
        if !missing.is_empty() {
            let cards = load_cards(pool, &missing).await?;
            self.store_cards(cards, now);
        }
        self.recompute(now);
        Ok(())
    }
}

static HUB: OnceLock<LiveHub> = OnceLock::new();
static STARTED: OnceLock<()> = OnceLock::new();

/// The process-wide hub. Created on first use, so publishing and reading work before [`start`].
pub fn hub() -> &'static LiveHub {
    HUB.get_or_init(LiveHub::new)
}

/// Starts the presence broadcaster (3 s), the viewer-minute flusher (60 s) and the
/// activity poller (3 s). Call once from inside the tokio runtime; later calls do nothing.
pub fn start(pool: &'static PgPool) {
    if STARTED.set(()).is_err() {
        return;
    }
    tokio::spawn(presence_loop(pool));
    tokio::spawn(flush_loop(pool));
    tokio::spawn(activity_loop(pool));
}

/// Sends an event to every live connection, for example `ServerMsg::RankingUpdated`.
pub fn broadcast(msg: ServerMsg) {
    hub().publish(msg);
}

/// Live viewers of one game, read from the in-memory hub. 0 when there are none.
pub fn viewers(game_id: i64) -> u32 {
    hub().viewers(game_id)
}

/// Identity for presence and hit counting: `visitor_hash(daily_salt, ip, user_agent)`.
/// Returns `None` for bot user agents, which must never be counted.
///
/// The IP comes from `CF-Connecting-IP`, then the first `X-Forwarded-For` entry, then "".
pub async fn visitor_key(headers: &HeaderMap) -> Result<Option<Vec<u8>>, ServerFnError> {
    let user_agent = header_text(headers, "user-agent");
    if hits::is_bot_user_agent(user_agent) {
        return Ok(None);
    }
    let ip = client_ip(headers);
    let pool = state::pool()?;
    let salt = hits::daily_salt(pool, Utc::now().date_naive())
        .await
        .map_err(db_error)?;
    Ok(Some(hits::visitor_hash(&salt, &ip, user_agent)))
}

/// The message one connection receives: its own watched count and visible counts,
/// plus the site-wide numbers.
pub fn presence_for(
    snapshot: &PresenceSnapshot,
    watching: Option<i64>,
    visible: &[i64],
) -> ServerMsg {
    let here = watching
        .and_then(|game| snapshot.per_game.get(&game).copied())
        .unwrap_or(0);
    let counts = visible
        .iter()
        .filter_map(|game| {
            snapshot
                .per_game
                .get(game)
                .filter(|count| **count > 0)
                .map(|count| (*game, *count))
        })
        .collect();
    ServerMsg::Presence {
        online: snapshot.online,
        here,
        counts,
        top: snapshot.top.clone(),
    }
}

/// Runs one websocket connection until the client leaves, errors, or goes idle.
pub async fn serve_connection(
    mut socket: TypedWebsocket<ClientMsg, ServerMsg>,
    visitor: Option<Vec<u8>>,
) {
    let hub = hub();
    let id = hub.register(visitor, Instant::now());
    let mut events = hub.subscribe_events();
    let mut presence = hub.subscribe_presence();
    let mut watching: Option<i64> = None;
    let mut visible: Vec<i64> = Vec::new();
    let mut idle_check = tokio::time::interval(Duration::from_secs(5));

    let initial = hub.recompute(Instant::now());
    if socket
        .send(presence_for(&initial, watching, &visible))
        .await
        .is_err()
    {
        hub.unregister(id);
        return;
    }

    loop {
        tokio::select! {
            incoming = socket.recv() => {
                let Ok(message) = incoming else { break };
                let now = Instant::now();
                hub.touch(id, now);
                if let ClientMsg::Hello { watching: next_watching, visible: next_visible } = message {
                    watching = next_watching;
                    visible = hub.hello(id, next_watching, next_visible, now);
                    let snapshot = hub.recompute(now);
                    if socket.send(presence_for(&snapshot, watching, &visible)).await.is_err() {
                        break;
                    }
                }
            }
            event = events.recv() => match event {
                Ok(msg) => {
                    if socket.send(msg).await.is_err() {
                        break;
                    }
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            },
            changed = presence.changed() => {
                if changed.is_err() {
                    break;
                }
                let snapshot = presence.borrow_and_update().clone();
                if socket.send(presence_for(&snapshot, watching, &visible)).await.is_err() {
                    break;
                }
            }
            _ = idle_check.tick() => {
                if hub.is_idle(id, Instant::now()) {
                    break;
                }
            }
        }
    }
    hub.unregister(id);
}

async fn presence_loop(pool: &'static PgPool) {
    let mut tick = tokio::time::interval(Duration::from_secs(3));
    loop {
        tick.tick().await;
        if let Err(err) = hub().refresh(pool).await {
            tracing::warn!(error = ?err, "live presence refresh failed");
        }
    }
}

async fn flush_loop(pool: &'static PgPool) {
    let mut tick = tokio::time::interval(Duration::from_secs(60));
    loop {
        tick.tick().await;
        let (online, per_game) = hub().counts();
        let per_game: Vec<(i64, i32)> = per_game
            .into_iter()
            .map(|(game, count)| (game, count as i32))
            .collect();
        let day = Utc::now().date_naive();
        if let Err(err) = flush_viewer_minutes(pool, day, &per_game, online as i32).await {
            tracing::warn!(error = ?err, "live viewer flush failed");
        }
    }
}

async fn activity_loop(pool: &'static PgPool) {
    // Start after the newest existing row so restarts do not replay history.
    let mut last = loop {
        match activity::latest_id(pool).await {
            Ok(id) => break id,
            Err(err) => {
                tracing::warn!(error = ?err, "activity cursor unavailable; retrying");
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    };
    let mut tick = tokio::time::interval(Duration::from_secs(3));
    loop {
        tick.tick().await;
        match forward_activity(pool, last).await {
            Ok(next) => last = next,
            Err(err) => tracing::warn!(error = ?err, "activity poll failed"),
        }
    }
}

/// Publishes every activity row after `after`, in id order. Returns the new cursor.
async fn forward_activity(pool: &PgPool, after: i64) -> DbResult<i64> {
    let mut last = after;
    loop {
        let rows = activity::since(pool, last, ACTIVITY_BATCH).await?;
        if rows.is_empty() {
            return Ok(last);
        }
        let full = rows.len() as i64 == ACTIVITY_BATCH;
        let cover_ids: Vec<i64> = rows
            .iter()
            .filter_map(|row| row.game_cover_media_id)
            .collect();
        let covers = load_media(pool, cover_ids).await?;
        for row in rows {
            last = row.id;
            hub().publish(ServerMsg::Activity(Box::new(activity_item(row, &covers))));
        }
        if !full {
            return Ok(last);
        }
    }
}

fn activity_item(row: ActivityRow, covers: &HashMap<i64, MediaRef>) -> ActivityItem {
    ActivityItem {
        id: row.id,
        kind: row.kind,
        created_at: row.created_at,
        game_id: row.game_id,
        game_slug: row.game_slug,
        game_name: row.game_name,
        game_cover: row
            .game_cover_media_id
            .and_then(|id| covers.get(&id).cloned()),
        port_id: row.port_id,
        port_slug: row.port_slug,
        port_name: row.port_name,
        version: row
            .data
            .get("version")
            .and_then(|value| value.as_str())
            .map(str::to_owned),
    }
}

#[derive(sqlx::FromRow)]
struct CardRow {
    id: i64,
    slug: String,
    name: String,
    cover_media_id: Option<i64>,
    port_count: i32,
}

async fn load_cards(pool: &PgPool, games: &[i64]) -> DbResult<Vec<LiveGame>> {
    let rows: Vec<CardRow> = sqlx::query_as(
        "SELECT id, slug, name, cover_media_id, port_count FROM games WHERE id = ANY($1)",
    )
    .bind(games.to_vec())
    .fetch_all(pool)
    .await?;
    let cover_ids: Vec<i64> = rows.iter().filter_map(|row| row.cover_media_id).collect();
    let covers = load_media(pool, cover_ids).await?;
    Ok(rows
        .into_iter()
        .map(|row| LiveGame {
            game_id: row.id,
            slug: row.slug,
            name: row.name,
            cover: row.cover_media_id.and_then(|id| covers.get(&id).cloned()),
            viewers: 0,
            port_count: row.port_count,
        })
        .collect())
}

async fn load_media(pool: &PgPool, ids: Vec<i64>) -> DbResult<HashMap<i64, MediaRef>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<MediaRow> = media::get_many(pool, ids).await?;
    Ok(rows.iter().map(|row| (row.id, media_ref(row))).collect())
}

fn media_ref(row: &MediaRow) -> MediaRef {
    let filename = row.public_filename.clone().filter(|filename| !filename.is_empty());
    let ready = row.status == "ready"
        && row.render_version >= recomphub_ingest::media::MEDIA_RENDER_VERSION
        && !row.widths.is_empty()
        && filename.is_some();
    MediaRef {
        id: row.id,
        kind: row.kind.clone(),
        filename,
        widths: row.widths.clone(),
        width: row.width,
        height: row.height,
        ready,
    }
}

/// Removes duplicates (keeping the first occurrence) and caps the list at [`MAX_VISIBLE`].
fn normalise_visible(visible: Vec<i64>) -> Vec<i64> {
    let mut seen = HashSet::new();
    visible
        .into_iter()
        .filter(|game| seen.insert(*game))
        .take(MAX_VISIBLE)
        .collect()
}

fn header_text<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
}

fn client_ip(headers: &HeaderMap) -> String {
    let cf = header_text(headers, "cf-connecting-ip").trim();
    if !cf.is_empty() {
        return cf.to_owned();
    }
    header_text(headers, "x-forwarded-for")
        .split(',')
        .next()
        .unwrap_or("")
        .trim()
        .to_owned()
}

/// Locks, recovering from poisoning so one panicked connection cannot stop presence.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
