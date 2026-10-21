//! Private account-backed game library server functions.

use dioxus::prelude::*;
use recomphub_shared::dto::{GameFavoriteMerge, GameFavoriteState};

#[cfg(feature = "server")]
use crate::server::session::{db_error, http_error, require_user};
#[cfg(feature = "server")]
use crate::server::state::pool;
#[cfg(feature = "server")]
use dioxus::server::axum::http::StatusCode;

#[cfg(feature = "server")]
const MERGE_LIMIT: usize = 200;

/// Current user's private favorite ids, newest saved first.
#[get("/api/v1/library/favorites")]
pub async fn favorite_game_ids() -> Result<Vec<i64>, ServerFnError> {
    let user = require_user().await?;
    recomphub_db::game_favorites::list(pool()?, user.id)
        .await
        .map_err(db_error)
}

/// Sets one private catalog game favorite to an explicit state.
#[post("/api/v1/library/favorites/set")]
pub async fn set_game_favorite(
    game_id: i64,
    favorite: bool,
) -> Result<GameFavoriteState, ServerFnError> {
    validate_positive(game_id)?;
    let user = require_user().await?;
    recomphub_db::game_favorites::set(pool()?, user.id, game_id, favorite)
        .await
        .map_err(db_error)?;
    Ok(GameFavoriteState { game_id, favorite })
}

/// Additively imports device favorites into the current account.
#[post("/api/v1/library/favorites/merge")]
pub async fn merge_game_favorites(game_ids: Vec<i64>) -> Result<GameFavoriteMerge, ServerFnError> {
    let game_ids = validated_ids(game_ids, MERGE_LIMIT)?;
    let user = require_user().await?;
    let result = recomphub_db::game_favorites::merge(pool()?, user.id, &game_ids)
        .await
        .map_err(db_error)?;
    Ok(GameFavoriteMerge {
        accepted_game_ids: result.accepted_game_ids,
        missing_game_ids: result.missing_game_ids,
    })
}

#[cfg(feature = "server")]
fn validate_positive(game_id: i64) -> Result<(), ServerFnError> {
    if game_id > 0 {
        Ok(())
    } else {
        Err(http_error(
            StatusCode::BAD_REQUEST,
            "game id must be positive",
        ))
    }
}

#[cfg(feature = "server")]
fn validated_ids(game_ids: Vec<i64>, limit: usize) -> Result<Vec<i64>, ServerFnError> {
    let mut ids = Vec::with_capacity(game_ids.len().min(limit));
    for game_id in game_ids {
        validate_positive(game_id)?;
        if !ids.contains(&game_id) {
            ids.push(game_id);
            if ids.len() > limit {
                return Err(http_error(
                    StatusCode::BAD_REQUEST,
                    format!("at most {limit} game ids are allowed"),
                ));
            }
        }
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::validated_ids;

    #[test]
    fn ids_are_positive_deduplicated_and_bounded() {
        assert_eq!(validated_ids(vec![3, 1, 3, 2], 3).unwrap(), [3, 1, 2]);
        assert!(validated_ids(vec![0], 3).is_err());
        assert!(validated_ids(vec![1, 2, 3, 4], 3).is_err());
    }
}
