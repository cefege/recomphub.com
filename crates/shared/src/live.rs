//! Live presence and activity wire types, sent as JSON over `/api/live`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::dto::MediaRef;

/// Messages sent by the browser.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    /// `watching` is the game whose page is open. `visible` lists the game ids
    /// rendered as cards on the page (at most 200).
    Hello {
        watching: Option<i64>,
        visible: Vec<i64>,
    },
    Ping,
}

/// A game with live viewers, shown in the "Live now" rail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveGame {
    pub game_id: i64,
    pub slug: String,
    pub name: String,
    pub cover: Option<MediaRef>,
    pub viewers: u32,
    pub port_count: i32,
}

/// One row of the public activity feed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivityItem {
    pub id: i64,
    /// `port_added`, `release`, `starred`, `wanted` or `game_added`.
    pub kind: String,
    pub created_at: DateTime<Utc>,
    pub game_id: i64,
    pub game_slug: String,
    pub game_name: String,
    pub game_cover: Option<MediaRef>,
    pub port_id: Option<i64>,
    pub port_slug: Option<String>,
    pub port_name: Option<String>,
    /// Release version for `release` items.
    pub version: Option<String>,
}

/// Messages sent by the server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    Presence {
        online: u32,
        here: u32,
        counts: Vec<(i64, u32)>,
        top: Vec<LiveGame>,
    },
    Activity(Box<ActivityItem>),
    RankingUpdated,
}

/// Point-in-time presence used during server rendering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresenceSnapshotDto {
    pub online: u32,
    pub top: Vec<LiveGame>,
}
