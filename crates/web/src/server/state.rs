//! Process-wide server state: the Postgres pool, set once at startup.

use std::sync::OnceLock;

use dioxus::prelude::ServerFnError;
use sqlx::PgPool;

static POOL: OnceLock<PgPool> = OnceLock::new();

/// Stores the pool. Called once from bootstrap before the server accepts requests.
pub fn init_pool(pool: PgPool) -> Result<(), PgPool> {
    POOL.set(pool)
}

/// The pool for server functions and routes.
pub fn pool() -> Result<&'static PgPool, ServerFnError> {
    POOL.get()
        .ok_or_else(|| ServerFnError::new("database pool is not initialised"))
}
