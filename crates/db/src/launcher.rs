//! Reviewed, server-side install manifests. Local installation paths never enter this table.

use chrono::{DateTime, Utc};
use recomphub_shared::InstallManifest;
use sqlx::PgExecutor;

use crate::{DbError, DbResult};

#[derive(Debug, Clone, sqlx::FromRow)]
struct RawBuild {
    recipe_id: String,
    port_id: i64,
    manifest: serde_json::Value,
    enabled: bool,
    checked_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct LauncherBuild {
    pub recipe_id: String,
    pub port_id: i64,
    pub manifest: InstallManifest,
    pub enabled: bool,
    pub checked_at: DateTime<Utc>,
}

impl TryFrom<RawBuild> for LauncherBuild {
    type Error = DbError;

    fn try_from(value: RawBuild) -> Result<Self, Self::Error> {
        let manifest: InstallManifest = serde_json::from_value(value.manifest)?;
        manifest
            .validate()
            .map_err(|error| DbError::InvalidPayload(error.to_string()))?;
        if manifest.recipe.id != value.recipe_id || manifest.port_id != value.port_id {
            return Err(DbError::InvalidPayload(
                "launcher build row disagrees with its manifest".into(),
            ));
        }
        Ok(Self {
            recipe_id: value.recipe_id,
            port_id: value.port_id,
            manifest,
            enabled: value.enabled,
            checked_at: value.checked_at,
        })
    }
}

pub async fn enabled(ex: impl PgExecutor<'_>) -> DbResult<Vec<LauncherBuild>> {
    let rows = sqlx::query_as::<_, RawBuild>("SELECT recipe_id, port_id, manifest, enabled, checked_at FROM launcher_builds WHERE enabled ORDER BY recipe_id")
        .fetch_all(ex)
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| LauncherBuild::try_from(row).ok())
        .collect())
}

pub async fn get(ex: impl PgExecutor<'_>, recipe_id: &str) -> DbResult<Option<LauncherBuild>> {
    let row = sqlx::query_as::<_, RawBuild>("SELECT recipe_id, port_id, manifest, enabled, checked_at FROM launcher_builds WHERE recipe_id = $1")
        .bind(recipe_id)
        .fetch_optional(ex)
        .await?;
    row.map(LauncherBuild::try_from).transpose()
}

/// All historical recipe IDs, including disabled rows, for removed-recipe reconciliation.
pub async fn recipe_ids(ex: impl PgExecutor<'_>) -> DbResult<Vec<String>> {
    Ok(
        sqlx::query_scalar("SELECT recipe_id FROM launcher_builds ORDER BY recipe_id")
            .fetch_all(ex)
            .await?,
    )
}

/// Serializes launcher refreshes until the containing transaction ends.
pub async fn lock_refresh(ex: impl PgExecutor<'_>) -> DbResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(741921384, 1)")
        .execute(ex)
        .await?;
    Ok(())
}

pub async fn upsert(
    ex: impl PgExecutor<'_>,
    manifest: &InstallManifest,
    enabled: bool,
    checked_at: DateTime<Utc>,
) -> DbResult<()> {
    manifest
        .validate()
        .map_err(|error| DbError::InvalidPayload(error.to_string()))?;
    sqlx::query("INSERT INTO launcher_builds(recipe_id, port_id, manifest, enabled, checked_at) VALUES ($1, $2, $3, $4, $5) ON CONFLICT(recipe_id) DO UPDATE SET port_id = excluded.port_id, manifest = excluded.manifest, enabled = excluded.enabled, checked_at = excluded.checked_at")
        .bind(&manifest.recipe.id)
        .bind(manifest.port_id)
        .bind(serde_json::to_value(manifest)?)
        .bind(enabled)
        .bind(checked_at)
        .execute(ex)
        .await?;
    Ok(())
}

/// Disabling deliberately retains the last validated manifest for installed clients and audit.
pub async fn disable(
    ex: impl PgExecutor<'_>,
    recipe_id: &str,
    checked_at: DateTime<Utc>,
) -> DbResult<()> {
    sqlx::query("UPDATE launcher_builds SET enabled = false, checked_at = $2 WHERE recipe_id = $1")
        .bind(recipe_id)
        .bind(checked_at)
        .execute(ex)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use recomphub_shared::{
        CpuArch, HostOs, InstallManifest, InstallRecipe, LaunchSpec, PackageFormat,
    };

    use super::{DbError, upsert};

    #[sqlx::test(migrations = "./migrations")]
    async fn upsert_rejects_malformed_manifest_url(pool: crate::PgPool) {
        let manifest = InstallManifest {
            schema_version: 1,
            recipe: InstallRecipe {
                id: "reviewed-build".into(),
                revision: 1,
                repo_url: "https://github.com/example/game".into(),
                os: HostOs::Windows,
                architectures: vec![CpuArch::X86_64],
                minimum_os_version: None,
                asset_name_template: "game-{tag}.zip".into(),
                package_format: PackageFormat::Zip,
                launch: LaunchSpec {
                    executable: "game.exe".into(),
                    working_directory: ".".into(),
                    arguments: vec![],
                },
                data_adapter: None,
                requirements: vec![],
            },
            port_id: 1,
            game_id: 1,
            release_id: 1,
            asset_id: 1,
            tag: "v1.0.0".into(),
            version: "1.0.0".into(),
            published_at: Utc::now(),
            release_url: "https://github.com/example/game/releases/tag/v1.0.0".into(),
            asset_name: "game-v1.0.0.zip".into(),
            download_url:
                "https://github.com/example/game-extra/releases/download/v1.0.0/game-v1.0.0.zip"
                    .into(),
            size_bytes: 1,
            sha256: "ab".repeat(32),
        };

        assert!(matches!(
            upsert(&pool, &manifest, true, Utc::now()).await,
            Err(DbError::InvalidPayload(_))
        ));
    }
}
