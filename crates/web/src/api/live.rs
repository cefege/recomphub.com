//! Live presence and activity: the websocket endpoint and the presence snapshot
//! used by server-rendered pages.

#[cfg(feature = "server")]
use dioxus::fullstack::HeaderMap;
use dioxus::fullstack::{WebSocketOptions, Websocket};
use dioxus::prelude::*;
use recomphub_shared::{ClientMsg, PresenceSnapshotDto, ServerMsg};

/// Websocket for presence and live activity. Clients send `ClientMsg` and receive `ServerMsg`.
#[get("/api/live", headers: HeaderMap)]
pub async fn live(
    options: WebSocketOptions,
) -> Result<Websocket<ClientMsg, ServerMsg>, ServerFnError> {
    let visitor = crate::server::live::visitor_key(&headers).await?;
    Ok(options.on_upgrade(move |socket| crate::server::live::serve_connection(socket, visitor)))
}

/// Site-wide online count and the "Live now" rail, from the last published snapshot.
#[get("/api/live/snapshot")]
pub async fn live_snapshot() -> Result<PresenceSnapshotDto, ServerFnError> {
    Ok(live_presence())
}

/// Synchronous read of the in-memory presence snapshot, for server-rendered pages.
/// Empty `top` and `online: 0` when the hub has not been started.
#[cfg(feature = "server")]
pub fn live_presence() -> PresenceSnapshotDto {
    let snapshot = crate::server::live::hub().current();
    PresenceSnapshotDto {
        online: snapshot.online,
        top: snapshot.top.clone(),
    }
}

/// Live viewers of one game, from the in-memory hub. 0 when there are none.
#[cfg(feature = "server")]
pub fn live_viewers(game_id: i64) -> u32 {
    crate::server::live::viewers(game_id)
}
