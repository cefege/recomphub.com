//! Server-only database layer for RecompHub: pool migrations, queries, and
//! the submission apply/revert logic.

pub mod activity;
pub mod ai_providers;
pub mod author_funding;
pub mod comments;
pub mod creators;
pub mod game_faqs;
pub mod games;
pub mod hits;
pub mod identities;
pub mod live;
pub mod media;
pub mod port_descriptions;
pub mod port_media;
pub mod ports;
pub mod sales;
pub mod secrets;
pub mod settings;
pub mod stars;
pub mod stats;
pub mod submissions;
pub mod users_sessions;
pub mod video_discovery;
pub mod videos;
pub use sqlx::{PgConnection, PgExecutor, PgPool};

use recomphub_shared::UnknownValue;

/// Errors returned by every database function in this crate.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("database error: {0}")]
    Sql(#[from] sqlx::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid state: {0}")]
    InvalidState(String),
    /// A user-facing refusal, such as an account that is already connected elsewhere.
    #[error("{0}")]
    Conflict(String),
    #[error("invalid payload: {0}")]
    InvalidPayload(String),
    #[error("account is banned")]
    Banned,
    #[error("daily submission limit reached; retry in {0} seconds")]
    RateLimited(i64),
    #[error("unknown {} value: {}", .0.kind, .0.value)]
    Enum(UnknownValue),
}

impl From<UnknownValue> for DbError {
    fn from(value: UnknownValue) -> Self {
        DbError::Enum(value)
    }
}

pub type DbResult<T> = Result<T, DbError>;

/// Applies the embedded migrations. Safe to call from several processes at once.
pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await
}
