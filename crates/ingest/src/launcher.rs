//! Synchronizes reviewed native-install recipes with stable GitHub release assets.

use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use recomphub_db::{PgPool, launcher as builds, ports};
use recomphub_shared::{
    INSTALL_MANIFEST_SCHEMA_VERSION, InstallManifest, InstallRecipe,
    validate_github_release_download_url,
};
use semver::Version;
use std::collections::{HashMap, HashSet};

use crate::github::{GithubClient, InstallRelease, InstallReleaseFetch};

const RECIPES_JSON: &str = include_str!("launcher/recipes.json");

pub fn recipes() -> Result<Vec<InstallRecipe>> {
    let recipes: Vec<InstallRecipe> =
        serde_json::from_str(RECIPES_JSON).context("parsing checked-in launcher recipes")?;
    let mut ids = HashSet::new();
    for recipe in &recipes {
        recipe
            .validate()
            .map_err(|error| anyhow!(error.to_string()))?;
        if !ids.insert(recipe.id.as_str()) {
            return Err(anyhow!("duplicate launcher recipe ID: {}", recipe.id));
        }
    }
    Ok(recipes)
}

/// Refreshes every checked-in reviewed recipe. Each upstream repository is fetched once per run.
pub async fn sync_install_offers(pool: &PgPool, client: &GithubClient) -> Result<()> {
    let recipes = recipes()?;
    sync_reviewed(pool, &recipes, client).await
}

trait ReleaseSource {
    async fn latest(&self, repo_url: &str) -> Result<InstallReleaseFetch>;
}

impl ReleaseSource for GithubClient {
    async fn latest(&self, repo_url: &str) -> Result<InstallReleaseFetch> {
        self.latest_install_release(repo_url).await
    }
}

async fn sync_reviewed(
    pool: &PgPool,
    recipes: &[InstallRecipe],
    source: &impl ReleaseSource,
) -> Result<()> {
    let mut tx = pool.begin().await.context("beginning launcher refresh")?;
    builds::lock_refresh(&mut *tx)
        .await
        .context("locking launcher refresh")?;
    let reviewed_ids: HashSet<&str> = recipes.iter().map(|recipe| recipe.id.as_str()).collect();
    let mut releases = HashMap::new();
    let mut errors = Vec::new();

    for recipe in recipes {
        let checked_at = Utc::now();
        let port = match ports::find_by_repo_url(&mut *tx, &recipe.repo_url).await {
            Ok(Some(port)) if port.status == "published" => port,
            Ok(_) => {
                if let Err(error) = builds::disable(&mut *tx, &recipe.id, checked_at).await {
                    errors.push(format!("{}: {error:#}", recipe.id));
                }
                continue;
            }
            Err(error) => {
                errors.push(format!("{} catalog lookup: {error:#}", recipe.id));
                continue;
            }
        };
        let repository =
            crate::github_links::canonical_repo_url(&recipe.repo_url).ok_or_else(|| {
                anyhow!(
                    "invalid reviewed GitHub repository URL: {}",
                    recipe.repo_url
                )
            })?;

        if !releases.contains_key(&repository) {
            let fetched = source
                .latest(&recipe.repo_url)
                .await
                .map_err(|error| format!("{error:#}"));
            releases.insert(repository.clone(), fetched);
        }
        let result = releases.get(&repository).expect("release fetched");
        let release = match result {
            Err(error) => {
                errors.push(format!("{} release request: {error}", recipe.id));
                continue;
            }
            Ok(InstallReleaseFetch::Missing) => {
                if let Err(error) = builds::disable(&mut *tx, &recipe.id, checked_at).await {
                    errors.push(format!("{} withdrawal: {error:#}", recipe.id));
                }
                continue;
            }
            Ok(InstallReleaseFetch::Malformed(reason)) => {
                errors.push(format!("{} malformed stable release: {reason}", recipe.id));
                if let Err(error) = builds::disable(&mut *tx, &recipe.id, checked_at).await {
                    errors.push(format!("{} withdrawal: {error:#}", recipe.id));
                }
                continue;
            }
            Ok(InstallReleaseFetch::Release(release)) => release.clone(),
        };
        if release.draft || release.prerelease {
            errors.push(format!(
                "{} latest release is draft or prerelease",
                recipe.id
            ));
            if let Err(error) = builds::disable(&mut *tx, &recipe.id, checked_at).await {
                errors.push(format!("{} withdrawal: {error:#}", recipe.id));
            }
            continue;
        }
        let manifest = match manifest_for(recipe, port.id, port.game_id, release) {
            Ok(Some(manifest)) => manifest,
            Ok(None) => {
                errors.push(format!(
                    "{} latest stable release has no valid reviewed asset",
                    recipe.id
                ));
                if let Err(error) = builds::disable(&mut *tx, &recipe.id, checked_at).await {
                    errors.push(format!("{} withdrawal: {error:#}", recipe.id));
                }
                continue;
            }
            Err(error) => {
                errors.push(format!("{} malformed stable release: {error:#}", recipe.id));
                if let Err(disable_error) = builds::disable(&mut *tx, &recipe.id, checked_at).await
                {
                    errors.push(format!("{} withdrawal: {disable_error:#}", recipe.id));
                }
                continue;
            }
        };
        let previous = match builds::get(&mut *tx, &recipe.id).await {
            Ok(previous) => previous,
            Err(error) => {
                errors.push(format!("{} stored offer: {error:#}", recipe.id));
                continue;
            }
        };
        if let Some(previous) = previous {
            match should_publish(&previous.manifest, &manifest) {
                Ok(true) => {}
                Ok(false) => {
                    errors.push(format!(
                        "{} latest release regressed or changed the same-version asset",
                        recipe.id
                    ));
                    if let Err(error) = builds::disable(&mut *tx, &recipe.id, checked_at).await {
                        errors.push(format!("{} withdrawal: {error:#}", recipe.id));
                    }
                    continue;
                }
                Err(error) => {
                    errors.push(format!("{} stored version: {error:#}", recipe.id));
                    continue;
                }
            }
        }
        if let Err(error) = builds::upsert(&mut *tx, &manifest, true, checked_at).await {
            errors.push(format!("{} publish: {error:#}", recipe.id));
        }
    }

    match builds::recipe_ids(&mut *tx).await {
        Ok(existing_ids) => {
            for recipe_id in existing_ids {
                if !reviewed_ids.contains(recipe_id.as_str())
                    && let Err(error) = builds::disable(&mut *tx, &recipe_id, Utc::now()).await
                {
                    errors.push(format!("{recipe_id} removed recipe: {error:#}"));
                }
            }
        }
        Err(error) => errors.push(format!("removed-recipe reconciliation: {error:#}")),
    }

    tx.commit().await.context("committing launcher refresh")?;
    if errors.is_empty() {
        Ok(())
    } else {
        Err(anyhow!("launcher sync had errors: {}", errors.join("; ")))
    }
}

fn should_publish(previous: &InstallManifest, candidate: &InstallManifest) -> Result<bool> {
    if previous.port_id != candidate.port_id || previous.game_id != candidate.game_id {
        return Ok(false);
    }
    let previous_version = Version::parse(&previous.version)?;
    let candidate_version = Version::parse(&candidate.version)?;
    Ok(candidate_version > previous_version
        || (candidate_version == previous_version
            && previous.tag == candidate.tag
            && previous.release_id == candidate.release_id
            && previous.asset_id == candidate.asset_id
            && previous.asset_name == candidate.asset_name
            && previous.size_bytes == candidate.size_bytes
            && previous.sha256 == candidate.sha256))
}

fn manifest_for(
    recipe: &InstallRecipe,
    port_id: i64,
    game_id: i64,
    release: InstallRelease,
) -> Result<Option<InstallManifest>> {
    let version = release.tag.strip_prefix('v').unwrap_or(&release.tag);
    let parsed = match Version::parse(version) {
        Ok(version) if version.pre.is_empty() && version.build.is_empty() => version,
        _ => return Ok(None),
    };
    let expected = recipe
        .expected_asset_name(&release.tag)
        .map_err(|error| anyhow!(error.to_string()))?;
    let assets: Vec<_> = release
        .assets
        .into_iter()
        .filter(|asset| asset.name == expected)
        .collect();
    if assets.len() != 1 {
        return Ok(None);
    }
    let asset = assets.into_iter().next().expect("exactly one asset");
    let digest = match asset
        .digest
        .as_deref()
        .and_then(|value| value.strip_prefix("sha256:"))
    {
        Some(value) if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) => {
            value.to_ascii_lowercase()
        }
        _ => return Ok(None),
    };
    if asset.state != "uploaded"
        || asset.size == 0
        || asset.size > 512 * 1024 * 1024
        || validate_github_release_download_url(&recipe.repo_url, &asset.download_url).is_err()
    {
        return Ok(None);
    }
    let manifest = InstallManifest {
        schema_version: INSTALL_MANIFEST_SCHEMA_VERSION,
        recipe: recipe.clone(),
        port_id,
        game_id,
        release_id: release.id,
        asset_id: asset.id,
        tag: release.tag,
        version: parsed.to_string(),
        published_at: release.published_at,
        release_url: release.url,
        asset_name: asset.name,
        download_url: asset.download_url,
        size_bytes: asset.size,
        sha256: digest,
    };
    manifest
        .validate()
        .map_err(|error| anyhow!(error.to_string()))?;
    Ok(Some(manifest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipe() -> InstallRecipe {
        recipes()
            .unwrap()
            .into_iter()
            .find(|recipe| recipe.id == "banjo-recompiled-macos")
            .unwrap()
    }

    fn release(
        tag: &str,
        release_id: i64,
        asset_id: i64,
        digest: &str,
        size: u64,
    ) -> InstallRelease {
        InstallRelease {
            id: release_id,
            tag: tag.into(),
            published_at: Utc::now(),
            url: format!("https://github.com/BanjoRecomp/BanjoRecomp/releases/tag/{tag}"),
            draft: false,
            prerelease: false,
            assets: vec![crate::github::InstallAsset {
                id: asset_id,
                name: format!("BanjoRecompiled-{tag}-macOS.zip"),
                download_url: format!(
                    "https://github.com/BanjoRecomp/BanjoRecomp/releases/download/{tag}/BanjoRecompiled-{tag}-macOS.zip"
                ),
                size,
                state: "uploaded".into(),
                digest: Some(format!("sha256:{digest}")),
            }],
        }
    }

    #[test]
    fn manifest_for_accepts_githubs_mixed_case_asset_urls() {
        let manifest = manifest_for(
            &recipe(),
            1,
            2,
            release("v1.0.2", 3, 4, &"ab".repeat(32), 1),
        )
        .unwrap()
        .expect("reviewed asset is publishable");
        assert_eq!(
            manifest.download_url,
            "https://github.com/BanjoRecomp/BanjoRecomp/releases/download/v1.0.2/BanjoRecompiled-v1.0.2-macOS.zip"
        );
    }

    #[test]
    fn manifest_for_rejects_unstable_or_incomplete_release_metadata() {
        assert!(
            manifest_for(
                &recipe(),
                1,
                2,
                release("v1.0.2-rc.1", 3, 4, &"ab".repeat(32), 1)
            )
            .unwrap()
            .is_none()
        );
        assert!(
            manifest_for(
                &recipe(),
                1,
                2,
                release("v1.0.2", 0, 4, &"ab".repeat(32), 1)
            )
            .is_err()
        );
    }

    #[test]
    fn release_transitions_promote_reenable_and_refuse_same_version_changes() {
        let make = |tag: &str, release_id, asset_id, digest: &str, size| {
            manifest_for(
                &recipe(),
                1,
                2,
                release(tag, release_id, asset_id, digest, size),
            )
            .unwrap()
            .unwrap()
        };
        let active = make("v1.0.1", 10, 20, &"ab".repeat(32), 100);
        let promoted = make("v1.0.2", 11, 21, &"cd".repeat(32), 200);
        assert!(should_publish(&active, &promoted).unwrap());
        assert!(should_publish(&active, &active).unwrap());

        let mut recipe_revalidated = active.clone();
        recipe_revalidated.recipe.revision += 1;
        assert!(should_publish(&active, &recipe_revalidated).unwrap());
        assert!(!should_publish(&promoted, &active).unwrap());
        let mut reassigned = active.clone();
        reassigned.game_id += 1;
        assert!(!should_publish(&active, &reassigned).unwrap());
        assert!(!should_publish(&active, &make("v1.0.1", 12, 20, &"ab".repeat(32), 100)).unwrap());
        assert!(!should_publish(&active, &make("v1.0.1", 10, 21, &"ab".repeat(32), 100)).unwrap());
        assert!(!should_publish(&active, &make("v1.0.1", 10, 20, &"cd".repeat(32), 100)).unwrap());
        assert!(!should_publish(&active, &make("v1.0.1", 10, 20, &"ab".repeat(32), 101)).unwrap());
    }

    #[derive(Clone)]
    enum Observation {
        Release(InstallRelease),
        Missing,
        Malformed,
        Failure,
    }

    #[derive(Clone)]
    struct FixtureSource(std::sync::Arc<std::sync::Mutex<Observation>>);

    impl FixtureSource {
        fn new(observation: Observation) -> Self {
            Self(std::sync::Arc::new(std::sync::Mutex::new(observation)))
        }

        fn set(&self, observation: Observation) {
            *self.0.lock().expect("fixture source lock") = observation;
        }
    }

    impl ReleaseSource for FixtureSource {
        async fn latest(&self, _repo_url: &str) -> Result<InstallReleaseFetch> {
            match self.0.lock().expect("fixture source lock").clone() {
                Observation::Release(release) => Ok(InstallReleaseFetch::Release(release)),
                Observation::Missing => Ok(InstallReleaseFetch::Missing),
                Observation::Malformed => Ok(InstallReleaseFetch::Malformed(
                    "fixture malformed metadata".into(),
                )),
                Observation::Failure => Err(anyhow!("fixture transport failure")),
            }
        }
    }

    #[sqlx::test(migrations = "../db/migrations")]
    async fn database_release_transitions_preserve_history_and_transient_state(
        pool: PgPool,
    ) -> Result<()> {
        use chrono::Duration;
        use recomphub_db::{
            games::{self, IgdbGame},
            ports::{self, PortFull, PortInput},
        };

        let recipe = recipe();
        let mut conn = pool.acquire().await?;
        let game_id = games::upsert_igdb(
            &mut conn,
            &IgdbGame {
                igdb_id: 882001,
                slug: "launcher-transition-game".into(),
                name: "Launcher transition fixture".into(),
                ..Default::default()
            },
        )
        .await?;
        let port_id = ports::insert_port(
            &mut conn,
            game_id,
            &PortFull {
                input: PortInput {
                    name: "Launcher transition fixture".into(),
                    slug: Some("launcher-transition-port".into()),
                    kind: "static_recomp".into(),
                    repo_url: recipe.repo_url.clone(),
                    playability: "playable".into(),
                    ..Default::default()
                },
                authors: vec![],
            },
            &["launcher-transition-fixture".into()],
        )
        .await?;

        let source = FixtureSource::new(Observation::Release(release(
            "v1.0.1",
            101,
            201,
            &"ab".repeat(32),
            100,
        )));
        sync_reviewed(&pool, std::slice::from_ref(&recipe), &source).await?;
        let initial = builds::get(&pool, &recipe.id)
            .await?
            .expect("initial build");
        assert!(initial.enabled);
        assert_eq!(initial.manifest.version, "1.0.1");

        builds::disable(&pool, &recipe.id, Utc::now() - Duration::minutes(2)).await?;
        sync_reviewed(&pool, std::slice::from_ref(&recipe), &source).await?;
        assert!(
            builds::get(&pool, &recipe.id)
                .await?
                .expect("re-enabled build")
                .enabled
        );

        let mut revised_recipe = recipe.clone();
        revised_recipe.revision += 1;
        sync_reviewed(&pool, std::slice::from_ref(&revised_recipe), &source).await?;
        assert_eq!(
            builds::get(&pool, &recipe.id)
                .await?
                .expect("revised build")
                .manifest
                .recipe
                .revision,
            revised_recipe.revision
        );

        source.set(Observation::Release(release(
            "v1.0.2",
            102,
            202,
            &"cd".repeat(32),
            200,
        )));
        sync_reviewed(&pool, std::slice::from_ref(&revised_recipe), &source).await?;
        let promoted = builds::get(&pool, &recipe.id)
            .await?
            .expect("promoted build");
        assert!(promoted.enabled);
        assert_eq!(promoted.manifest.version, "1.0.2");

        source.set(Observation::Release(release(
            "v1.0.1",
            101,
            201,
            &"ab".repeat(32),
            100,
        )));
        assert!(
            sync_reviewed(&pool, std::slice::from_ref(&revised_recipe), &source)
                .await
                .is_err()
        );
        let lower = builds::get(&pool, &recipe.id)
            .await?
            .expect("lower release retained");
        assert!(!lower.enabled);
        assert_eq!(lower.manifest.version, "1.0.2");

        source.set(Observation::Release(release(
            "v1.0.2",
            999,
            202,
            &"cd".repeat(32),
            200,
        )));
        assert!(
            sync_reviewed(&pool, std::slice::from_ref(&revised_recipe), &source)
                .await
                .is_err()
        );
        let changed = builds::get(&pool, &recipe.id)
            .await?
            .expect("same-version row retained");
        assert!(!changed.enabled);
        assert_eq!(changed.manifest.release_id, 102);

        for altered_release in [
            release("v1.0.2", 102, 203, &"cd".repeat(32), 200),
            release("v1.0.2", 102, 202, &"ef".repeat(32), 200),
            release("v1.0.2", 102, 202, &"cd".repeat(32), 201),
        ] {
            source.set(Observation::Release(release(
                "v1.0.2",
                102,
                202,
                &"cd".repeat(32),
                200,
            )));
            sync_reviewed(&pool, std::slice::from_ref(&revised_recipe), &source).await?;
            source.set(Observation::Release(altered_release));
            assert!(
                sync_reviewed(&pool, std::slice::from_ref(&revised_recipe), &source)
                    .await
                    .is_err()
            );
            let refused = builds::get(&pool, &recipe.id)
                .await?
                .expect("changed asset row retained");
            assert!(!refused.enabled);
            assert_eq!(refused.manifest.release_id, 102);
            assert_eq!(refused.manifest.asset_id, 202);
        }

        let retained_at = Utc::now() - Duration::hours(1);
        builds::upsert(&pool, &promoted.manifest, true, retained_at).await?;
        let retained_at = builds::get(&pool, &recipe.id)
            .await?
            .expect("verification timestamp persisted")
            .checked_at;
        source.set(Observation::Failure);
        assert!(
            sync_reviewed(&pool, std::slice::from_ref(&revised_recipe), &source)
                .await
                .is_err()
        );
        let transient = builds::get(&pool, &recipe.id)
            .await?
            .expect("transient row retained");
        assert!(transient.enabled);
        assert_eq!(transient.checked_at, retained_at);
        assert_eq!(transient.manifest.version, "1.0.2");

        source.set(Observation::Missing);
        sync_reviewed(&pool, std::slice::from_ref(&revised_recipe), &source).await?;
        assert!(
            !builds::get(&pool, &recipe.id)
                .await?
                .expect("missing release row")
                .enabled
        );

        source.set(Observation::Release(release(
            "v1.0.2",
            102,
            202,
            &"cd".repeat(32),
            200,
        )));
        sync_reviewed(&pool, std::slice::from_ref(&revised_recipe), &source).await?;
        source.set(Observation::Malformed);
        assert!(
            sync_reviewed(&pool, std::slice::from_ref(&revised_recipe), &source)
                .await
                .is_err()
        );
        assert!(
            !builds::get(&pool, &recipe.id)
                .await?
                .expect("malformed release row")
                .enabled
        );
        let mut removed_recipe = revised_recipe.clone();
        removed_recipe.id = "removed-fixture-recipe".into();
        let removed_manifest = manifest_for(
            &removed_recipe,
            port_id,
            game_id,
            release("v1.0.2", 102, 202, &"cd".repeat(32), 200),
        )?
        .expect("removed fixture manifest");
        builds::upsert(&pool, &removed_manifest, true, retained_at).await?;
        sync_reviewed(&pool, &[], &source).await?;
        let removed = builds::get(&pool, "removed-fixture-recipe")
            .await?
            .expect("removed history retained");
        assert!(!removed.enabled);
        assert_eq!(removed.manifest.version, "1.0.2");

        // A waiting refresh must observe upstream only after the preceding refresh releases it.
        // Keep its own pool to one connection so holding the lock cannot exhaust that pool.
        let single_pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_with(pool.connect_options().as_ref().clone())
            .await?;
        let mut preceding_refresh = pool.begin().await?;
        builds::lock_refresh(&mut *preceding_refresh).await?;
        source.set(Observation::Release(release(
            "v1.0.3",
            103,
            203,
            &"ef".repeat(32),
            300,
        )));
        {
            let waiting_refresh =
                sync_reviewed(&single_pool, std::slice::from_ref(&revised_recipe), &source);
            tokio::pin!(waiting_refresh);
            let waiting_for_lock = async {
                loop {
                    let waiting: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE locktype = 'advisory' AND database = (SELECT oid FROM pg_database WHERE datname = current_database()) AND classid = 741921384 AND objid = 1 AND objsubid = 2 AND NOT granted)",
                    )
                    .fetch_one(&pool)
                    .await?;
                    if waiting {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
                Ok::<_, sqlx::Error>(())
            };
            tokio::select! {
                result = &mut waiting_refresh => panic!("refresh bypassed the preceding refresh: {result:?}"),
                result = tokio::time::timeout(std::time::Duration::from_secs(5), waiting_for_lock) => result??,
            }
            source.set(Observation::Release(release(
                "v1.0.4",
                104,
                204,
                &"ab".repeat(32),
                400,
            )));
            preceding_refresh.commit().await?;
            tokio::time::timeout(std::time::Duration::from_secs(5), waiting_refresh).await??;
        }
        let serialized = builds::get(&pool, &recipe.id).await?.unwrap();
        assert!(serialized.enabled);
        assert_eq!(serialized.manifest.version, "1.0.4");

        struct InterruptedSource(tokio::sync::Notify);
        impl ReleaseSource for InterruptedSource {
            async fn latest(&self, _: &str) -> Result<InstallReleaseFetch> {
                self.0.notify_one();
                std::future::pending().await
            }
        }
        let interrupted = InterruptedSource(tokio::sync::Notify::new());
        {
            let cancelled = sync_reviewed(
                &single_pool,
                std::slice::from_ref(&revised_recipe),
                &interrupted,
            );
            tokio::pin!(cancelled);
            tokio::select! {
                result = &mut cancelled => panic!("interrupted source unexpectedly finished: {result:?}"),
                _ = interrupted.0.notified() => {}
            }
        }
        source.set(Observation::Release(release(
            "v1.0.5",
            105,
            205,
            &"cd".repeat(32),
            500,
        )));
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            sync_reviewed(&single_pool, std::slice::from_ref(&revised_recipe), &source),
        )
        .await??;
        let resumed = builds::get(&pool, &recipe.id).await?.unwrap();
        assert!(resumed.enabled);
        assert_eq!(resumed.manifest.version, "1.0.5");
        single_pool.close().await;
        Ok(())
    }
}
