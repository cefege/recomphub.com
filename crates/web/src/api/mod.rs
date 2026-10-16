//! Server functions called from the UI. Each submodule owns one area; the flat
//! re-exports let pages call `api::name(...)`.

pub mod account;
pub mod catalog;
pub mod comments;
pub mod creators;
pub mod games {
    pub use super::catalog::GamesQuery;
}
pub mod live;
pub mod members;
pub mod moderation;
pub mod stats;

pub use account::*;
pub use catalog::*;
pub use comments::*;
pub use creators::*;
pub use live::*;
pub use members::*;
pub use moderation::*;
pub use stats::*;
