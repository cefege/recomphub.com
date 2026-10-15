//! Server functions called from the UI. Each submodule owns one area; the flat
//! re-exports let pages call `api::name(...)`.

pub mod catalog;
pub mod games {
    pub use super::catalog::GamesQuery;
}
pub mod live;
pub mod members;
pub mod moderation;

pub use catalog::*;
pub use live::*;
pub use members::*;
pub use moderation::*;
