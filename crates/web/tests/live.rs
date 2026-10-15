#![cfg(feature = "server")]

use std::time::{Duration, Instant};

use recomphub::server::live::{IDLE_TIMEOUT, LiveHub, MAX_VISIBLE, TOP_N, presence_for};
use recomphub_shared::{LiveGame, ServerMsg};

fn visitor(n: u8) -> Option<Vec<u8>> {
    Some(vec![n; 32])
}

fn card(game_id: i64) -> LiveGame {
    LiveGame {
        game_id,
        slug: format!("game-{game_id}"),
        name: format!("Game {game_id}"),
        cover: None,
        viewers: 0,
        port_count: 1,
    }
}

#[test]
fn same_visitor_on_two_tabs_counts_once() {
    let hub = LiveHub::new();
    let now = Instant::now();
    let a = hub.register(visitor(1), now);
    let b = hub.register(visitor(1), now);
    hub.hello(a, Some(5), vec![], now);
    hub.hello(b, Some(5), vec![], now);

    let snapshot = hub.recompute(now);
    assert_eq!(snapshot.online, 1);
    assert_eq!(snapshot.per_game.get(&5), Some(&1));
}

#[test]
fn two_visitors_on_one_game_see_each_other() {
    let hub = LiveHub::new();
    let now = Instant::now();
    let a = hub.register(visitor(1), now);
    let b = hub.register(visitor(2), now);
    hub.hello(a, Some(5), vec![], now);
    hub.hello(b, Some(5), vec![], now);

    let snapshot = hub.recompute(now);
    assert_eq!(snapshot.online, 2);
    match presence_for(&snapshot, Some(5), &[]) {
        ServerMsg::Presence { here, online, .. } => {
            assert_eq!(here, 2);
            assert_eq!(online, 2);
        }
        other => panic!("expected presence, got {other:?}"),
    }
}

#[test]
fn bot_connections_are_never_counted() {
    let hub = LiveHub::new();
    let now = Instant::now();
    let human = hub.register(visitor(1), now);
    let bot = hub.register(None, now);
    hub.hello(human, Some(5), vec![], now);
    hub.hello(bot, Some(5), vec![], now);

    let snapshot = hub.recompute(now);
    assert_eq!(snapshot.online, 1);
    assert_eq!(snapshot.per_game.get(&5), Some(&1));
}

#[test]
fn counts_only_visible_games_that_have_viewers() {
    let hub = LiveHub::new();
    let now = Instant::now();
    let a = hub.register(visitor(1), now);
    let b = hub.register(visitor(2), now);
    hub.hello(a, Some(5), vec![], now);
    hub.hello(b, Some(6), vec![], now);

    let snapshot = hub.recompute(now);
    match presence_for(&snapshot, None, &[5, 6, 7]) {
        ServerMsg::Presence { here, counts, .. } => {
            assert_eq!(here, 0);
            assert_eq!(counts, vec![(5, 1), (6, 1)]);
        }
        other => panic!("expected presence, got {other:?}"),
    }
}

#[test]
fn leaving_removes_the_viewer() {
    let hub = LiveHub::new();
    let now = Instant::now();
    let a = hub.register(visitor(1), now);
    let b = hub.register(visitor(2), now);
    hub.hello(a, Some(5), vec![], now);
    hub.hello(b, Some(5), vec![], now);
    assert_eq!(hub.recompute(now).per_game.get(&5), Some(&2));

    hub.unregister(b);
    let snapshot = hub.recompute(now);
    assert_eq!(snapshot.online, 1);
    assert_eq!(snapshot.per_game.get(&5), Some(&1));
}

#[test]
fn silent_connections_are_dropped_after_the_idle_timeout() {
    let hub = LiveHub::new();
    let start = Instant::now();
    let a = hub.register(visitor(1), start);
    hub.hello(a, Some(5), vec![], start);

    let almost = start + IDLE_TIMEOUT;
    assert!(!hub.is_idle(a, almost));

    let later = start + IDLE_TIMEOUT + Duration::from_secs(1);
    assert!(hub.is_idle(a, later));
    hub.sweep(later);
    assert_eq!(hub.recompute(later).online, 0);
}

#[test]
fn a_message_keeps_the_connection_alive() {
    let hub = LiveHub::new();
    let start = Instant::now();
    let a = hub.register(visitor(1), start);
    let ping_at = start + Duration::from_secs(60);
    hub.touch(a, ping_at);

    let later = start + IDLE_TIMEOUT + Duration::from_secs(1);
    assert!(!hub.is_idle(a, later));
}

#[test]
fn hello_dedupes_and_caps_visible_games() {
    let hub = LiveHub::new();
    let now = Instant::now();
    let a = hub.register(visitor(1), now);
    let stored = hub.hello(a, None, vec![1, 1, 2], now);
    assert_eq!(stored, vec![1, 2]);

    let many: Vec<i64> = (0..(MAX_VISIBLE as i64 + 50)).collect();
    assert_eq!(hub.hello(a, None, many, now).len(), MAX_VISIBLE);
}

#[test]
fn top_lists_the_busiest_cached_games_first() {
    let hub = LiveHub::new();
    let now = Instant::now();
    let games: Vec<i64> = (1..=(TOP_N as i64 + 1)).collect();
    hub.store_cards(games.iter().map(|g| card(*g)).collect(), now);

    // Game g gets g viewers, so game TOP_N+1 is busiest and game 1 is cut.
    let mut next_visitor = 0u8;
    for game in &games {
        for _ in 0..*game {
            next_visitor = next_visitor.wrapping_add(1);
            let conn = hub.register(Some(vec![next_visitor; 32]), now);
            hub.hello(conn, Some(*game), vec![], now);
        }
    }

    let snapshot = hub.recompute(now);
    assert_eq!(snapshot.top.len(), TOP_N);
    assert_eq!(snapshot.top[0].game_id, TOP_N as i64 + 1);
    assert_eq!(snapshot.top[0].viewers, TOP_N as u32 + 1);
    assert!(snapshot.top.iter().all(|g| g.game_id != 1));
}

#[test]
fn presence_publishes_only_when_it_changes() {
    let hub = LiveHub::new();
    let now = Instant::now();
    let mut watch = hub.subscribe_presence();

    let _ = watch.borrow_and_update();
    hub.recompute(now);
    assert!(!watch.has_changed().unwrap());

    hub.recompute(now);
    assert!(!watch.has_changed().unwrap());

    let a = hub.register(visitor(1), now);
    hub.hello(a, Some(5), vec![], now);
    hub.recompute(now);
    assert!(watch.has_changed().unwrap());
}

#[test]
fn published_events_reach_subscribers() {
    let hub = LiveHub::new();
    let mut events = hub.subscribe_events();
    hub.publish(ServerMsg::RankingUpdated);
    assert!(matches!(events.try_recv(), Ok(ServerMsg::RankingUpdated)));
}

#[test]
fn publishing_without_subscribers_is_harmless() {
    let hub = LiveHub::new();
    hub.publish(ServerMsg::RankingUpdated);
    assert_eq!(hub.viewers(5), 0);
}
