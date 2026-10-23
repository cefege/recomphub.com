use super::*;

#[test]
fn stale_process_observation_cannot_clear_replacement_identity_or_membership() {
    let root = tempfile::tempdir().unwrap();
    let db = Mutex::new(rusqlite::Connection::open_in_memory().unwrap());
    db.lock().unwrap().execute_batch(
        "CREATE TABLE running_processes(port_id INTEGER PRIMARY KEY,pid INTEGER NOT NULL,started_at INTEGER NOT NULL,executable_path TEXT NOT NULL);",
    ).unwrap();
    db.lock().unwrap().execute(
        "INSERT INTO running_processes(port_id,pid,started_at,executable_path) VALUES(7,202,9002,'/owned/new-game')",
        [],
    ).unwrap();
    let tracked = Mutex::new(HashSet::from([7]));
    let stale = ProcessIdentity {
        port_id: 7,
        pid: 101,
        started_at: 9001,
        executable: "/owned/old-game".into(),
    };

    clear_process_record(
        root.path(),
        &tracked,
        &db,
        &stale,
        Some("stale observation"),
    )
    .unwrap();

    let conn = db.lock().unwrap();
    let replacement: (i64, i64, String) = conn
        .query_row(
            "SELECT pid,started_at,executable_path FROM running_processes WHERE port_id=7",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(replacement, (202, 9002, "/owned/new-game".into()));
    assert!(tracked.lock().unwrap().contains(&7));
}

#[test]
fn uncertain_launch_recovery_does_not_reconcile_unrelated_port_work() {
    let root = tempfile::tempdir().unwrap();
    let manager = LibraryManager::open(root.path().join("library")).unwrap();
    let part = manager.root.join("downloads/unrelated.part");
    fs::write(&part, b"in-progress package").unwrap();
    manager.db.lock().unwrap().execute(
        "INSERT INTO installs(port_id,game_id,game_name,game_slug,port_name,port_slug,manifest_json,active_path,status,error,offer_json) VALUES(11,1,'Other Game','other-game','Other Port','other-port','{}','games/11/versions/v1','ready',NULL,'{}')",
        [],
    ).unwrap();
    manager.db.lock().unwrap().execute(
        "INSERT INTO operations(operation_id,port_id,kind,phase,candidate_offer_json,final_path,created_at) VALUES('uncertain-launch',11,'launch','launching',NULL,'games/11/versions/v1','2026-01-01T00:00:00Z')",
        [],
    ).unwrap();
    manager.db.lock().unwrap().execute(
        "INSERT INTO operations(operation_id,port_id,kind,phase,part_path,created_at) VALUES('other-port',22,'install','downloading','downloads/unrelated.part','2026-01-01T00:00:00Z')",
        [],
    ).unwrap();

    assert!(
        !manager
            .recover_launch_operation("uncertain-launch", 11)
            .unwrap()
    );

    assert_eq!(fs::read(&part).unwrap(), b"in-progress package");
    let phase: String = manager
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT phase FROM operations WHERE operation_id='other-port'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(phase, "downloading");
}

#[test]
fn post_spawn_uncertain_failure_blocks_a_duplicate_port_operation() {
    let root = tempfile::tempdir().unwrap();
    let manager = LibraryManager::open(root.path().join("library")).unwrap();
    manager.db.lock().unwrap().execute(
        "INSERT INTO installs(port_id,game_id,game_name,game_slug,port_name,port_slug,manifest_json,active_path,status,error,offer_json) VALUES(31,1,'Game','game','Port','port','{}','games/31/versions/v1','ready',NULL,'{}')",
        [],
    ).unwrap();
    manager.db.lock().unwrap().execute(
        "INSERT INTO operations(operation_id,port_id,kind,phase,created_at) VALUES('uncertain-launch',31,'launch','launching','2026-01-01T00:00:00Z')",
        [],
    ).unwrap();
    let spawn_error =
        DesktopError::LaunchUncertain("native helper may have spawned the process".into());
    assert!(
        !manager
            .record_launch_failure("uncertain-launch", 31, &spawn_error)
            .unwrap()
    );

    let (phase,status,error): (String,String,String) = manager.db.lock().unwrap().query_row(
        "SELECT o.phase,i.status,i.error FROM operations o JOIN installs i ON i.port_id=o.port_id WHERE o.operation_id='uncertain-launch'",
        [],|row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
    ).unwrap();
    assert_eq!(phase, "launching");
    assert_eq!(status, "failed");
    assert!(error.contains("uncertain"));
    assert!(matches!(
        manager.begin_operation(31),
        Err(DesktopError::Busy)
    ));
}

#[cfg(any(target_os = "macos", windows))]
#[test]
fn verified_reinstall_drops_damaged_path_and_corrupt_offer_snapshots() {
    let host = detect_host_platform().unwrap();
    let root = tempfile::tempdir().unwrap();
    let library_root = root.path().join("library");
    let manager = LibraryManager::open(library_root.clone()).unwrap();
    let foreign = ensure_versions_directory(manager.root(), 999)
        .unwrap()
        .join("foreign");
    fs::create_dir_all(&foreign).unwrap();
    fs::write(foreign.join("unrelated-package"), b"another port").unwrap();

    for (port_id, prior_path, prior_offer) in [
        (81, "games/999/versions/foreign", None),
        (82, "games/82/versions/old", Some("{}")),
    ] {
        let offer =
            crate::lifecycle::offer(&host, port_id, "1.0.2", port_id * 10, 1, &"0".repeat(64));
        offer.validate().unwrap();
        let manifest_json = serde_json::to_string(&offer.manifest).unwrap();
        let offer_json = prior_offer
            .map(str::to_owned)
            .unwrap_or_else(|| serde_json::to_string(&offer).unwrap());
        if prior_path == "games/82/versions/old" {
            let prior = ensure_versions_directory(manager.root(), port_id)
                .unwrap()
                .join("old");
            fs::create_dir_all(prior).unwrap();
        }
        manager.db.lock().unwrap().execute(
            "INSERT INTO installs(port_id,game_id,game_name,game_slug,port_name,port_slug,manifest_json,active_path,status,error,offer_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'ready',NULL,?9)",
            params![port_id,offer.game.id,offer.game.name,offer.game.slug,offer.port.name,offer.port.slug,manifest_json,prior_path,offer_json],
        ).unwrap();
        let operation_id = format!("verified-reinstall-{port_id}");
        manager.db.lock().unwrap().execute(
            "INSERT INTO operations(operation_id,port_id,kind,phase,created_at) VALUES(?1,?2,'install','validated','2026-01-01T00:00:00Z')",
            params![operation_id,port_id],
        ).unwrap();

        let final_dir = ensure_versions_directory(manager.root(), port_id)
            .unwrap()
            .join("replacement");
        fs::create_dir_all(&final_dir).unwrap();
        let executable = final_dir.join(&offer.manifest.recipe.launch.executable);
        fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        }
        write_manifest_file(&final_dir, &offer.manifest).unwrap();
        manager
            .commit_install(&offer, &final_dir, &operation_id)
            .unwrap();
        assert_eq!(
            fs::read(foreign.join("unrelated-package")).unwrap(),
            b"another port"
        );

        let (previous_path,previous_manifest,previous_offer): (Option<String>,Option<String>,Option<String>) =
            manager.db.lock().unwrap().query_row(
                "SELECT previous_path,previous_manifest_json,previous_offer_json FROM installs WHERE port_id=?1",
                params![port_id],|row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
            ).unwrap();
        assert!(previous_path.is_none());
        assert!(previous_manifest.is_none());
        assert!(previous_offer.is_none());
        let installed = manager.installed_one(port_id).unwrap().unwrap();
        assert_eq!(installed.manifest.as_ref(), Some(&offer.manifest));
        assert_eq!(installed.active_path, final_dir);
        assert!(installed.previous_path.is_none());
        assert_eq!(installed.status, InstallStatus::Ready);
    }
}
#[cfg(any(target_os = "macos", windows))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn startup_defers_interrupted_uninstall_for_exact_path_native_child() {
    let host = detect_host_platform().unwrap();
    let executable_name = if cfg!(windows) {
        "fixture.exe"
    } else {
        "fixture"
    };
    let offer = crate::lifecycle::offer(&host, 73, "1.0.1", 7301, 1, &"0".repeat(64));
    let root = tempfile::tempdir().unwrap();
    let library_root = root.path().join("library");
    let manager = LibraryManager::open(library_root.clone()).unwrap();
    let package = library_root.join("games/73/versions/recovery-fixture");
    fs::create_dir_all(&package).unwrap();
    let executable = package.join(executable_name);
    fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    }
    write_manifest_file(&package, &offer.manifest).unwrap();
    let manifest_json = serde_json::to_string(&offer.manifest).unwrap();
    let offer_json = serde_json::to_string(&offer).unwrap();
    let relative_path = relative(&library_root, &package).unwrap();
    manager.db.lock().unwrap().execute(
        "INSERT INTO installs(port_id,game_id,game_name,game_slug,port_name,port_slug,manifest_json,active_path,previous_path,status,error,offer_json,last_checked_at) VALUES(73,41,'Fixture Game','fixture-game','Fixture Port','fixture-port-73',?1,?2,NULL,'ready',NULL,?3,NULL)",
        params![manifest_json,relative_path,offer_json],
    ).unwrap();
    manager.db.lock().unwrap().execute(
        "INSERT INTO operations(operation_id,port_id,kind,phase,part_path,created_at) VALUES('interrupted-uninstall',73,'uninstall','uninstalling',?1,'2026-01-01T00:00:00Z')",
        params![relative_path],
    ).unwrap();

    let mut child = std::process::Command::new(&executable)
        .args([
            "--ignored",
            "--exact",
            "recovery::recovery_child_process_entry",
            "--nocapture",
        ])
        .current_dir(&package)
        .spawn()
        .unwrap();
    drop(manager);
    let manager = LibraryManager::open(library_root.clone()).unwrap();
    assert!(
        package.exists(),
        "startup must preserve the exact-path live package"
    );
    assert!(
        manager.is_running(73).unwrap(),
        "the discovered native child must remain tracked"
    );
    let phase: String = manager
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT phase FROM operations WHERE operation_id='interrupted-uninstall'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(phase, "uninstalling");

    child.wait().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            manager.reconcile().unwrap();
            if manager.installed_one(73).unwrap().is_none() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("uninstall resumes after the child exits");
    assert!(!package.exists());
}

#[cfg(any(target_os = "macos", windows))]
#[test]
#[ignore]
fn recovery_child_process_entry() {
    std::thread::sleep(std::time::Duration::from_secs(3));
}
