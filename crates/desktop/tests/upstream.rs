#![cfg(any(target_os = "macos", windows))]

use recomphub_desktop::{LibraryManager, detect_host_platform};
use recomphub_shared::InstallOffer;

/// Supply a public offer returned by the real local backend, not an invented download manifest.
/// This downloads the reviewed release; it neither launches a game nor changes original data.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an explicit real-backend offer file and upstream network access"]
async fn real_upstream_package_installs_restarts_and_uninstalls() {
    let offer_path = std::env::var_os("RECOMPHUB_UPSTREAM_OFFER_FILE")
        .expect("set RECOMPHUB_UPSTREAM_OFFER_FILE to a public real-backend offer JSON file");
    let offer: InstallOffer = serde_json::from_slice(&std::fs::read(offer_path).unwrap()).unwrap();
    offer.validate().unwrap();
    assert!(
        offer
            .manifest
            .matches_host(&detect_host_platform().unwrap())
            .unwrap()
    );
    let root = tempfile::tempdir().unwrap();
    let library_root = root.path().join("library");
    let manager = LibraryManager::open(library_root.clone()).unwrap();
    manager.install(offer.clone()).await.unwrap();
    let installed = manager.installed().unwrap();
    let game = installed
        .iter()
        .find(|game| game.port_id == offer.port.id)
        .unwrap();
    assert_eq!(game.manifest.as_ref(), Some(&offer.manifest));
    let active_path = game.active_path.clone();
    assert!(
        active_path
            .join(&offer.manifest.recipe.launch.executable)
            .is_file()
    );
    drop(manager);
    let manager = LibraryManager::open(library_root).unwrap();
    let restarted = manager.installed().unwrap();
    let game = restarted
        .iter()
        .find(|game| game.port_id == offer.port.id)
        .unwrap();
    assert_eq!(game.active_path, active_path);
    assert_eq!(game.manifest.as_ref(), Some(&offer.manifest));
    manager.uninstall(offer.port.id).await.unwrap();
    assert!(
        manager
            .installed()
            .unwrap()
            .iter()
            .all(|game| game.port_id != offer.port.id)
    );
    assert!(!active_path.exists());
}
