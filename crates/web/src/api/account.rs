//! Sign-in options and the provider accounts connected to the signed-in viewer.

use dioxus::prelude::*;
use recomphub_shared::dto::{AuthProviders, ConnectedAccount};

#[cfg(feature = "server")]
use crate::server::{
    config,
    session::{db_error, http_error, require_user},
    state::pool,
};
#[cfg(feature = "server")]
use dioxus::server::axum::http::StatusCode;
#[cfg(feature = "server")]
use recomphub_db::identities::{self, Provider};

/// Which sign-in providers are configured on this server, so the UI can hide the rest.
#[get("/api/auth/providers")]
pub async fn auth_providers() -> Result<AuthProviders, ServerFnError> {
    let cfg = config::get();
    Ok(AuthProviders {
        github: cfg.github_client_id.is_some() && cfg.github_client_secret.is_some(),
        google: cfg.google_client_id.is_some() && cfg.google_client_secret.is_some(),
    })
}

/// The sign-in providers connected to the signed-in viewer, oldest first.
#[get("/api/account/connections")]
pub async fn connected_accounts() -> Result<Vec<ConnectedAccount>, ServerFnError> {
    let user = require_user().await?;
    let rows = identities::list_for_user(pool()?, user.id)
        .await
        .map_err(db_error)?;
    Ok(rows
        .into_iter()
        .map(|row| ConnectedAccount {
            provider: row.provider,
            handle: row.login.or(row.email),
        })
        .collect())
}

/// Disconnects one provider from the signed-in viewer. The last sign-in method cannot be removed.
#[post("/api/account/unlink")]
pub async fn unlink_account(provider: String) -> Result<(), ServerFnError> {
    let user = require_user().await?;
    let Some(provider) = Provider::parse(&provider) else {
        return Err(http_error(
            StatusCode::BAD_REQUEST,
            "unknown sign-in provider",
        ));
    };
    identities::unlink(pool()?, user.id, provider)
        .await
        .map_err(db_error)
}
