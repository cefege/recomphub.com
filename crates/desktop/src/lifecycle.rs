use super::*;
use std::{
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
};

#[cfg(any(target_os = "macos", windows))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_install_update_rollback_repair_and_uninstall_lifecycle() {
    let host = detect_host_platform().expect("supported native test host");
    let package = package_with_host_binary();
    let package_digest = hex(&Sha256::digest(&package));
    let package_size = package.len() as u64;
    let mut server = PackageServer::start(package);
    let isolated = tempfile::tempdir().unwrap();
    let library_root = isolated.path().join("library");
    let external_data = isolated.path().join("upstream");
    fs::create_dir_all(external_data.join("saves")).unwrap();
    fs::write(external_data.join("original-data-marker"), b"user data").unwrap();
    fs::write(external_data.join("saves/save-marker"), b"user save").unwrap();

    let mut manager = LibraryManager::open(library_root.clone()).unwrap();
    manager.test_download_endpoint = Some(server.url.clone());
    let manager = Arc::new(manager);
    let first_offer = offer(&host, 1, "1.0.1", 101, package_size, &package_digest);
    let first_install_manager = Arc::clone(&manager);
    let cancelled_offer = first_offer.clone();
    let first_install =
        tokio::spawn(async move { first_install_manager.install(cancelled_offer).await });
    server.wait_for_first_request().await;

    let queued_offer = offer(&host, 2, "1.0.1", 201, package_size, &package_digest);
    let queued_manager = Arc::clone(&manager);
    let queued_install = tokio::spawn(async move { queued_manager.install(queued_offer).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if manager.queue.lock().unwrap().contains(&2) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("second operation enters FIFO queue");
    assert!(manager.cancel(2).unwrap());
    assert!(manager.cancel(1).unwrap());
    server.release_first_request();
    assert!(matches!(
        first_install.await.unwrap(),
        Err(DesktopError::Cancelled)
    ));
    assert!(matches!(
        queued_install.await.unwrap(),
        Err(DesktopError::Cancelled)
    ));
    assert!(manager.installed_one(1).unwrap().is_none());
    assert!(manager.installed_one(2).unwrap().is_none());
    let terminal_operations = manager.operations().unwrap();
    assert!(
        terminal_operations
            .iter()
            .any(|operation| operation.port_id == Some(1) && operation.stage == "cancelled")
    );
    assert!(
        terminal_operations
            .iter()
            .any(|operation| operation.port_id == Some(2) && operation.stage == "cancelled")
    );
    assert_eq!(
        fs::read_dir(library_root.join("downloads"))
            .unwrap()
            .count(),
        0
    );
    assert_eq!(
        fs::read_dir(library_root.join("staging")).unwrap().count(),
        0
    );
    let expected_first_offer = first_offer.clone();
    manager.install(first_offer).await.unwrap();

    let original = manager.installed_one(1).unwrap().unwrap();
    assert_eq!(original.manifest.as_ref().unwrap().version, "1.0.1");
    let original_path = original.active_path.clone();
    drop(manager);

    let mut manager = LibraryManager::open(library_root.clone()).unwrap();
    manager.test_download_endpoint = Some(server.url.clone());
    let restarted = manager.installed_one(1).unwrap().unwrap();
    assert_eq!(restarted.active_path, original_path);
    assert_eq!(restarted.manifest.as_ref().unwrap().version, "1.0.1");

    let bad_digest = if let Some(suffix) = package_digest.strip_prefix('f') {
        format!("e{suffix}")
    } else {
        format!("f{}", &package_digest[1..])
    };
    let bad_update = offer(&host, 1, "1.0.2", 102, package_size, &bad_digest);
    assert!(manager.update(1, bad_update).await.is_err());
    let after_failed_update = manager.installed_one(1).unwrap().unwrap();
    assert_eq!(after_failed_update.active_path, original_path);
    assert_eq!(
        after_failed_update.manifest.as_ref().unwrap().version,
        "1.0.1"
    );
    assert!(
        manager
            .operations()
            .unwrap()
            .iter()
            .any(|operation| operation.port_id == Some(1) && operation.stage == "failed")
    );
    assert_eq!(
        fs::read_dir(library_root.join("downloads"))
            .unwrap()
            .count(),
        0
    );
    assert_eq!(
        fs::read_dir(library_root.join("staging")).unwrap().count(),
        0
    );

    let second_offer = offer(&host, 1, "1.0.2", 102, package_size, &package_digest);
    let expected_second_offer = second_offer.clone();
    manager.update(1, second_offer).await.unwrap();
    let updated = manager.installed_one(1).unwrap().unwrap();
    assert_eq!(updated.manifest.as_ref().unwrap().version, "1.0.2");
    assert_eq!(updated.previous_path.as_ref(), Some(&original_path));

    manager.rollback(1).await.unwrap();
    let rolled_back = manager.installed_one(1).unwrap().unwrap();
    let rolled_back_manifest = rolled_back.manifest.as_ref().unwrap();
    assert_eq!(rolled_back_manifest.version, "1.0.1");
    let active_offer_json: String = manager
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT offer_json FROM installs WHERE port_id=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let active_offer: InstallOffer = serde_json::from_str(&active_offer_json).unwrap();
    assert_eq!(active_offer, expected_first_offer);
    let previous_offer_json: String = manager
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT previous_offer_json FROM installs WHERE port_id=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let previous_offer: InstallOffer = serde_json::from_str(&previous_offer_json).unwrap();
    assert_eq!(previous_offer, expected_second_offer);

    fs::remove_file(
        rolled_back
            .active_path
            .join(&rolled_back_manifest.recipe.launch.executable),
    )
    .unwrap();
    assert_eq!(
        manager.installed_one(1).unwrap().unwrap().status,
        InstallStatus::RepairNeeded
    );
    manager.repair(1).await.unwrap();
    let repaired = manager.installed_one(1).unwrap().unwrap();
    assert_eq!(repaired.manifest.as_ref().unwrap().version, "1.0.1");
    assert_eq!(repaired.manifest.as_ref().unwrap().release_id, 101);
    assert_eq!(
        repaired.manifest.as_ref().unwrap(),
        &expected_first_offer.manifest
    );
    assert!(
        repaired
            .active_path
            .join(&repaired.manifest.as_ref().unwrap().recipe.launch.executable)
            .is_file()
    );
    let repaired_path = repaired.active_path.clone();

    let failed_fresh = offer(&host, 3, "1.0.1", 301, package_size, &bad_digest);
    assert!(manager.install(failed_fresh).await.is_err());
    assert!(manager.installed_one(3).unwrap().is_none());
    assert!(
        manager
            .operations()
            .unwrap()
            .iter()
            .any(|operation| operation.port_id == Some(3)
                && operation.stage == "failed"
                && operation.message.is_some())
    );

    manager.uninstall(1).await.unwrap();
    assert!(manager.installed_one(1).unwrap().is_none());
    assert!(external_data.join("original-data-marker").is_file());
    assert_eq!(
        fs::read(external_data.join("saves/save-marker")).unwrap(),
        b"user save"
    );
    assert!(!repaired_path.exists());
    assert!(
        !rolled_back.active_path.exists(),
        "uninstall left the replaced damaged package behind"
    );
    server.stop();
}

#[cfg(any(target_os = "macos", windows))]
fn package_with_host_binary() -> Vec<u8> {
    let executable = std::env::current_exe().unwrap();
    let mut bytes = Vec::new();
    let mut archive = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
    let options = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
    #[cfg(windows)]
    let executable_name = "fixture.exe";
    #[cfg(target_os = "macos")]
    let executable_name = "fixture";
    archive.start_file(executable_name, options).unwrap();
    io::copy(&mut File::open(executable).unwrap(), &mut archive).unwrap();
    archive.finish().unwrap();
    bytes
}

pub(crate) fn offer(
    host: &HostPlatform,
    port_id: i64,
    version: &str,
    release_id: i64,
    size_bytes: u64,
    sha256: &str,
) -> InstallOffer {
    let tag = format!("v{version}");
    let asset_name = format!("game-{tag}.zip");
    let recipe = recomphub_shared::InstallRecipe {
        id: "fixture-native".into(),
        revision: 1,
        repo_url: "https://github.com/example/game".into(),
        os: host.os,
        architectures: vec![host.arch],
        minimum_os_version: None,
        asset_name_template: "game-{tag}.zip".into(),
        package_format: recomphub_shared::PackageFormat::Zip,
        launch: recomphub_shared::LaunchSpec {
            #[cfg(windows)]
            executable: "fixture.exe".into(),
            #[cfg(target_os = "macos")]
            executable: "fixture".into(),
            working_directory: ".".into(),
            arguments: Vec::new(),
        },
        data_adapter: None,
        requirements: Vec::new(),
    };
    let manifest = InstallManifest {
        schema_version: 1,
        recipe,
        port_id,
        game_id: 41,
        release_id,
        asset_id: release_id * 10 + 1,
        tag: tag.clone(),
        version: version.into(),
        published_at: Utc::now(),
        release_url: format!("https://github.com/example/game/releases/tag/{tag}"),
        asset_name: asset_name.clone(),
        download_url: format!(
            "https://github.com/example/game/releases/download/{tag}/{asset_name}"
        ),
        size_bytes,
        sha256: sha256.into(),
    };
    let game = recomphub_shared::GameCard {
        id: 41,
        slug: "fixture-game".into(),
        name: "Fixture Game".into(),
        release_year: None,
        cover: None,
        port_count: 1,
        want_count: 0,
        targets: Vec::new(),
        best_playability: None,
        popularity: 0,
    };
    let port = recomphub_shared::PortCard {
        id: port_id,
        slug: format!("fixture-port-{port_id}"),
        game_id: game.id,
        game_slug: game.slug.clone(),
        game_name: game.name.clone(),
        name: "Fixture Port".into(),
        kind: "native".into(),
        kind_label: "Native".into(),
        targets: Vec::new(),
        authors: Vec::new(),
        ai_usage: "unknown".into(),
        ai_tools: Vec::new(),
        ai_evidence: Vec::new(),
        playability: "unknown".into(),
        latest_version: Some(version.into()),
        latest_release_url: Some(manifest.release_url.clone()),
        latest_release_at: Some(manifest.published_at),
        release_state: "released".into(),
        release_state_label: "Released".into(),
        github_stars: 0,
        star_count: 0,
        repo_url: manifest.recipe.repo_url.clone(),
        cover: None,
        maintenance: None,
    };
    InstallOffer {
        game,
        port,
        manifest,
        checked_at: Utc::now(),
    }
}

struct PackageServer {
    url: url::Url,
    first_request: Option<Receiver<()>>,
    release_first: SyncSender<()>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl PackageServer {
    fn start(bytes: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let (request_tx, request_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            let mut first = true;
            while !thread_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(5)))
                            .unwrap();
                        stream
                            .set_write_timeout(Some(Duration::from_secs(10)))
                            .unwrap();
                        let mut request = [0u8; 2048];
                        let _ = stream.read(&mut request);
                        if first {
                            first = false;
                            let _ = request_tx.send(());
                            if release_rx.recv().is_err() {
                                break;
                            }
                        }
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/zip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            bytes.len()
                        );
                        if stream.write_all(response.as_bytes()).is_ok() {
                            let _ = stream.write_all(&bytes);
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(std::time::Duration::from_millis(5))
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            url: url::Url::parse(&format!("http://{address}/asset.zip")).unwrap(),
            first_request: Some(request_rx),
            release_first: release_tx,
            stop,
            thread: Some(thread),
        }
    }

    async fn wait_for_first_request(&mut self) {
        let request = self.first_request.take().expect("first request receiver");
        tokio::task::spawn_blocking(move || request.recv())
            .await
            .unwrap()
            .unwrap();
    }

    fn release_first_request(&self) {
        let _ = self.release_first.send(());
    }

    fn stop(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

impl Drop for PackageServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.release_first.try_send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
