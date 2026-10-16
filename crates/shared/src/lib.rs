//! Types, enums and pure helpers shared by the server, the worker and the wasm client.
//! Keep this crate free of sqlx and other server-only dependencies.

pub mod dto;
pub mod enums;
pub mod live;
pub mod maintenance;
pub mod release;
pub mod seo;
pub mod text;

pub use dto::*;
pub use enums::*;
pub use live::{ActivityItem, ClientMsg, LiveGame, PresenceSnapshotDto, ServerMsg};
pub use maintenance::{MaintenanceInput, MaintenanceLevel, maintenance_level, maintenance_score};
pub use release::{ReleaseInput, ReleaseState, release_state};
pub use text::{normalize_title, slugify};

/// The six major platforms every game should reach. Platform parity and the creators'
/// "missing platforms" list are both measured over these.
pub const MAJOR_TARGETS: [&str; 6] = ["windows", "macos", "linux", "web", "android", "ios"];
