//! Native, device-local library management for RecompHub desktop.
//!
//! This crate owns package bytes, installed game state, original-data validation, and process
//! spawning. It intentionally knows nothing about Dioxus, Postgres, browser auth, or server
//! startup so the UI stays an adapter around this engine.

pub mod credentials;
pub mod data;
pub mod package;
pub mod platform;
pub mod store;
pub use platform::detect_host_platform;

pub fn run_shell_launch_helper_if_requested() -> Result<bool, DesktopError> {
    #[cfg(windows)]
    {
        platform::run_shell_launch_helper_if_requested()
    }
    #[cfg(not(windows))]
    {
        Ok(false)
    }
}

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use fs2::FileExt;
use futures_util::StreamExt;
use recomphub_shared::{
    CpuArch, HostOs, HostPlatform, InstallManifest, InstallOffer,
    validate_github_release_download_url,
};
use reqwest::Client;
use rusqlite::{Connection, OptionalExtension, params};
use semver::Version;
use sha2::{Digest, Sha256};
use tokio::sync::watch;
use uuid::Uuid;

const PACKAGE_CAP: u64 = 512 * 1024 * 1024;
const EXTRACT_CAP: u64 = 2 * 1024 * 1024 * 1024;
const ENTRY_CAP: usize = 50_000;
const ROM_CAP: u64 = 64 * 1024 * 1024;
const DEVICE_FAVORITE_CAP: usize = 2_000;
const BANJO_ROM_NAME: &str = "bk.n64.us.1.0.z64";

#[derive(Debug, thiserror::Error)]
pub enum DesktopError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("network error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("archive error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("invalid install offer: {0}")]
    Offer(String),
    #[error("unsafe package: {0}")]
    Package(String),
    #[error("game launch tracking is uncertain: {0}")]
    LaunchUncertain(String),
    #[error("operation is already in progress for this port")]
    Busy,
    #[error("operation cancelled")]
    Cancelled,
    #[error("game is running")]
    Running,
    #[error("game is not installed")]
    NotInstalled,
    #[error("game needs its original data file")]
    NeedsData,
    #[error("original data conflict: {0}")]
    DataConflict(String),
    #[error("unsupported original-data adapter: {0}")]
    UnsupportedDataAdapter(String),
    #[error("unsupported host: {0}")]
    UnsupportedHost(String),
    #[error("internal synchronization failed")]
    Poisoned,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallStatus {
    NotInstalled,
    Downloading,
    Verifying,
    Extracting,
    InstalledNeedsData,
    Ready,
    Running,
    Updating,
    Uninstalling,
    Failed,
    RepairNeeded,
}

impl InstallStatus {
    fn as_str(&self) -> &'static str {
        match self {
            Self::NotInstalled => "not_installed",
            Self::Downloading => "downloading",
            Self::Verifying => "verifying",
            Self::Extracting => "extracting",
            Self::InstalledNeedsData => "installed_needs_data",
            Self::Ready => "ready",
            Self::Running => "running",
            Self::Updating => "updating",
            Self::Uninstalling => "uninstalling",
            Self::Failed => "failed",
            Self::RepairNeeded => "repair_needed",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "downloading" => Self::Downloading,
            "verifying" => Self::Verifying,
            "extracting" => Self::Extracting,
            "installed_needs_data" => Self::InstalledNeedsData,
            "ready" => Self::Ready,
            "running" => Self::Running,
            "updating" => Self::Updating,
            "uninstalling" => Self::Uninstalling,
            "failed" => Self::Failed,
            "repair_needed" => Self::RepairNeeded,
            _ => Self::NotInstalled,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataStatus {
    NotRequired,
    Missing,
    Ready,
    Conflict,
    Unsupported,
    Error,
}

#[derive(Debug, Clone)]
pub struct InstalledGame {
    pub port_id: i64,
    pub game_id: i64,
    pub game_name: String,
    pub game_slug: String,
    pub port_name: String,
    pub port_slug: String,
    pub manifest: Option<InstallManifest>,
    pub active_path: PathBuf,
    pub previous_path: Option<PathBuf>,
    pub status: InstallStatus,
    pub running: bool,
    pub data_ready: bool,
    pub data_status: DataStatus,
    pub error: Option<String>,
    pub update_available: Option<InstallManifest>,
    pub update_suppressed: bool,
    pub last_checked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default)]
pub struct OperationProgress {
    pub operation_id: Option<String>,
    pub port_id: Option<i64>,
    pub queued_port_ids: Vec<i64>,
    pub stage: String,
    pub received_bytes: u64,
    pub total_bytes: Option<u64>,
    pub message: Option<String>,
    pub cancellable: bool,
}

pub struct LibraryManager {
    root: PathBuf,
    db: Arc<Mutex<Connection>>,
    _lock: Arc<File>,
    progress: watch::Sender<OperationProgress>,
    running: Arc<Mutex<HashMap<i64, RunningProcess>>>,
    operating: Arc<Mutex<HashSet<i64>>>,
    downloads: Client,
    #[cfg(test)]
    test_download_endpoint: Option<url::Url>,
    cancellations: Arc<Mutex<HashSet<i64>>>,
    queue: Mutex<Vec<i64>>,
    downloads_active: Mutex<bool>,
    tracked_processes: Arc<Mutex<HashSet<i64>>>,
    process_monitor_stop: Arc<AtomicBool>,
    process_monitor: Option<std::thread::JoinHandle<()>>,
}

impl LibraryManager {
    /// Opens one fixed managed library root. A process lock prevents concurrent mutation.
    pub fn open(root: PathBuf) -> Result<Self, DesktopError> {
        fs::create_dir_all(&root)?;
        let root = root.canonicalize()?;
        for directory in ["downloads", "games", "staging"] {
            ensure_root_directory(&root, directory)?;
        }
        let lock_path = root.join("library.lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)?;
        lock.try_lock_exclusive().map_err(|error| {
            DesktopError::UnsupportedHost(format!("library is already in use: {error}"))
        })?;
        let db = Connection::open(root.join("library.sqlite3"))?;
        initialize_database(&db)?;
        let (progress, _) = watch::channel(OperationProgress::default());
        let downloads = download_client()?;
        let manager = Self {
            root: root.clone(),
            db: Arc::new(Mutex::new(db)),
            _lock: Arc::new(lock),
            progress,
            running: Arc::new(Mutex::new(HashMap::new())),
            operating: Arc::new(Mutex::new(HashSet::new())),
            downloads,
            #[cfg(test)]
            test_download_endpoint: None,
            cancellations: Arc::new(Mutex::new(HashSet::new())),
            queue: Mutex::new(Vec::new()),
            downloads_active: Mutex::new(false),
            tracked_processes: Arc::new(Mutex::new(HashSet::new())),
            process_monitor_stop: Arc::new(AtomicBool::new(false)),
            process_monitor: None,
        };
        manager.reconcile()?;
        let mut manager = manager;
        let process_monitor = manager.start_process_monitor(root)?;
        manager.process_monitor = Some(process_monitor);
        Ok(manager)
    }

    pub fn default_root() -> Result<PathBuf, DesktopError> {
        #[cfg(target_os = "macos")]
        {
            return directories::BaseDirs::new()
                .map(|dirs| {
                    dirs.home_dir()
                        .join("Library/Application Support/RecompHub")
                })
                .ok_or_else(|| {
                    DesktopError::UnsupportedHost(
                        "could not resolve the application support folder".into(),
                    )
                });
        }
        #[cfg(windows)]
        {
            return std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .map(|path| path.join("RecompHub"))
                .ok_or_else(|| {
                    DesktopError::UnsupportedHost("LOCALAPPDATA is unavailable".into())
                });
        }
        #[allow(unreachable_code)]
        Err(DesktopError::UnsupportedHost(
            "this operating system is not supported".into(),
        ))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn favorite_game_ids(&self) -> Result<Vec<i64>, DesktopError> {
        let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
        read_favorite_ids(&db)
    }

    /// Changes one device-local saved game without requiring an account session.
    pub fn set_favorite_game(
        &self,
        game_id: i64,
        favorite: bool,
    ) -> Result<Vec<i64>, DesktopError> {
        if game_id <= 0 {
            return Err(DesktopError::Package(
                "favorite game ID must be positive".into(),
            ));
        }
        let mut db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
        let tx = db.transaction()?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM device_favorites WHERE game_id = ?1)",
            params![game_id],
            |row| row.get(0),
        )?;
        if favorite && !exists {
            let count: i64 = tx.query_row("SELECT COUNT(*) FROM device_favorites", [], |row| {
                row.get(0)
            })?;
            if count >= DEVICE_FAVORITE_CAP as i64 {
                return Err(DesktopError::Package(
                    "device saved games reached the 2,000-game limit".into(),
                ));
            }
            tx.execute(
                "INSERT INTO device_favorites(game_id, position) SELECT ?1, COALESCE(MAX(position), -1) + 1 FROM device_favorites",
                params![game_id],
            )?;
        } else if !favorite && exists {
            tx.execute(
                "DELETE FROM device_favorites WHERE game_id = ?1",
                params![game_id],
            )?;
        }
        let ids = read_favorite_ids(&tx)?;
        tx.commit()?;
        Ok(ids)
    }

    /// Removes only IDs acknowledged by the server, retaining every unconfirmed device ID.
    pub fn acknowledge_favorite_games(
        &self,
        acknowledged: &[i64],
    ) -> Result<Vec<i64>, DesktopError> {
        if acknowledged.len() > DEVICE_FAVORITE_CAP || acknowledged.iter().any(|id| *id <= 0) {
            return Err(DesktopError::Package(
                "invalid device favorite acknowledgement".into(),
            ));
        }
        let mut db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
        let tx = db.transaction()?;
        {
            let mut statement = tx.prepare("DELETE FROM device_favorites WHERE game_id = ?1")?;
            for id in acknowledged {
                statement.execute(params![id])?;
            }
        }
        let ids = read_favorite_ids(&tx)?;
        tx.commit()?;
        Ok(ids)
    }
    /// Returns the documented original-data destination for this installed adapter.
    pub fn data_destination(&self, port_id: i64) -> Result<Option<PathBuf>, DesktopError> {
        let game = self
            .installed_one(port_id)?
            .ok_or(DesktopError::NotInstalled)?;
        let manifest = game
            .manifest
            .as_ref()
            .ok_or_else(|| DesktopError::Package("installed metadata is damaged".into()))?;
        match manifest.recipe.data_adapter.as_deref() {
            None => Ok(None),
            Some("banjo_n64_us_10") => Ok(Some(banjo_data_directory()?.join(BANJO_ROM_NAME))),
            Some(adapter) => Err(DesktopError::UnsupportedDataAdapter(adapter.into())),
        }
    }

    /// Returns the upstream BanjoRecompiled saves path without creating it.
    pub fn saves_directory(&self, port_id: i64) -> Result<PathBuf, DesktopError> {
        let game = self
            .installed_one(port_id)?
            .ok_or(DesktopError::NotInstalled)?;
        let manifest = game
            .manifest
            .as_ref()
            .ok_or_else(|| DesktopError::Package("installed metadata is damaged".into()))?;
        match manifest.recipe.data_adapter.as_deref() {
            Some("banjo_n64_us_10") => Ok(banjo_data_directory()?.join("saves")),
            _ => Err(DesktopError::UnsupportedDataAdapter(
                "this game has no documented saves folder".into(),
            )),
        }
    }

    pub fn subscribe_progress(&self) -> watch::Receiver<OperationProgress> {
        self.progress.subscribe()
    }

    pub fn operations(&self) -> Result<Vec<OperationProgress>, DesktopError> {
        let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
        let mut statement = db.prepare(
            "SELECT operation_id, port_id, phase, error FROM operations o
             WHERE o.rowid = (
               SELECT newer.rowid FROM operations newer WHERE newer.port_id = o.port_id
               ORDER BY newer.created_at DESC, newer.rowid DESC, newer.operation_id DESC LIMIT 1
             ) AND o.phase <> 'committed'
             ORDER BY o.created_at, o.rowid, o.operation_id",
        )?;
        let rows = statement.query_map([], |row| {
            let stage: String = row.get(2)?;
            Ok(OperationProgress {
                operation_id: Some(row.get(0)?),
                port_id: Some(row.get(1)?),
                stage: stage.clone(),
                message: row.get(3)?,
                cancellable: matches!(
                    stage.as_str(),
                    "queued" | "downloading" | "verifying" | "extracting"
                ),
                ..OperationProgress::default()
            })
        })?;
        let mut operations = rows.collect::<Result<Vec<_>, _>>()?;
        let queued = self
            .queue
            .lock()
            .map_err(|_| DesktopError::Poisoned)?
            .clone();
        for operation in &mut operations {
            operation.queued_port_ids = queued.clone();
        }
        Ok(operations)
    }

    pub fn cancel(&self, port_id: i64) -> Result<bool, DesktopError> {
        let mut cancellations = self
            .cancellations
            .lock()
            .map_err(|_| DesktopError::Poisoned)?;
        let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
        let phase: Option<String> = db.query_row(
            "SELECT phase FROM operations WHERE port_id = ?1 AND phase NOT IN ('committed','failed','cancelled')",
            params![port_id], |row| row.get(0)
        ).optional()?;
        match phase.as_deref() {
            Some("queued" | "downloading" | "verifying" | "extracting") => {
                cancellations.insert(port_id);
                Ok(true)
            }
            _ => Ok(false),
        }
    }
    pub async fn install(&self, offer: InstallOffer) -> Result<(), DesktopError> {
        self.install_offer(offer, false).await
    }

    pub async fn update(&self, port_id: i64, candidate: InstallOffer) -> Result<(), DesktopError> {
        candidate
            .validate()
            .map_err(|error| DesktopError::Offer(error.to_string()))?;
        if candidate.manifest.port_id != port_id {
            return Err(DesktopError::Offer(
                "update offer does not match installed port".into(),
            ));
        }
        let current = self
            .installed_one(port_id)?
            .ok_or(DesktopError::NotInstalled)?;
        if current.running {
            return Err(DesktopError::Running);
        }
        let installed = current
            .manifest
            .as_ref()
            .ok_or_else(|| DesktopError::Package("installed metadata is damaged".into()))?;
        let current_repo =
            recomphub_shared::parse_github_repository_url(&installed.recipe.repo_url)
                .map_err(|error| DesktopError::Offer(error.to_string()))?;
        let candidate_repo =
            recomphub_shared::parse_github_repository_url(&candidate.manifest.recipe.repo_url)
                .map_err(|error| DesktopError::Offer(error.to_string()))?;
        let host = detect_host_platform()?;
        if candidate.manifest.recipe.id != installed.recipe.id
            || !candidate_repo.matches(&current_repo)
            || candidate.manifest.recipe.os != installed.recipe.os
            || candidate.manifest.recipe.revision < installed.recipe.revision
            || !candidate.manifest.recipe.architectures.contains(&host.arch)
        {
            return Err(DesktopError::Offer(
                "update does not match the installed reviewed recipe".into(),
            ));
        }
        let candidate_version = Version::parse(&candidate.manifest.version)
            .map_err(|_| DesktopError::Offer("candidate version is invalid".into()))?;
        let installed_version = Version::parse(&installed.version)
            .map_err(|_| DesktopError::Offer("installed version is invalid".into()))?;
        if candidate_version == installed_version
            && (candidate.manifest.release_id != installed.release_id
                || candidate.manifest.asset_id != installed.asset_id
                || candidate.manifest.size_bytes != installed.size_bytes
                || !candidate
                    .manifest
                    .sha256
                    .eq_ignore_ascii_case(&installed.sha256))
        {
            return Err(DesktopError::Offer(
                "Upstream asset changed; review required".into(),
            ));
        }
        if candidate_version <= installed_version {
            return Err(DesktopError::Offer(
                "candidate is not newer than the installed version".into(),
            ));
        }
        self.install_offer(candidate, true).await
    }

    pub async fn repair(&self, port_id: i64) -> Result<(), DesktopError> {
        let installed = self
            .installed_one(port_id)?
            .ok_or(DesktopError::NotInstalled)?;
        if installed.running {
            return Err(DesktopError::Running);
        }
        if installed.manifest.is_none() {
            return Err(DesktopError::Package(
                "Metadata damaged; select a verified online Reinstall offer".into(),
            ));
        }
        let offer = self.load_offer(port_id)?;
        offer
            .validate()
            .map_err(|error| DesktopError::Offer(error.to_string()))?;
        self.install_offer(offer, true).await
    }

    pub async fn rollback(&self, port_id: i64) -> Result<(), DesktopError> {
        let _operation = self.begin_operation(port_id)?;
        let game = self
            .installed_one(port_id)?
            .ok_or(DesktopError::NotInstalled)?;
        if game.running {
            return Err(DesktopError::Running);
        }
        let previous = game
            .previous_path
            .clone()
            .ok_or_else(|| DesktopError::Offer("no previous package is available".into()))?;
        if !previous.exists() {
            return Err(DesktopError::Package(
                "previous package files are missing".into(),
            ));
        }
        let current_manifest = game.manifest.as_ref().ok_or_else(|| {
            DesktopError::Package("active metadata is damaged; online reinstall required".into())
        })?;
        let _owned_previous =
            owned_version_path(&self.root, port_id, &relative(&self.root, &previous)?)?;
        let previous_offer_json: Option<String> = {
            let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
            db.query_row(
                "SELECT previous_offer_json FROM installs WHERE port_id=?1",
                params![port_id],
                |row| row.get(0),
            )?
        };
        let previous_offer_json = previous_offer_json
            .ok_or_else(|| DesktopError::Offer("previous offer metadata is unavailable".into()))?;
        let previous_offer: InstallOffer =
            serde_json::from_str(&previous_offer_json).map_err(json_error)?;
        previous_offer
            .validate()
            .map_err(|error| DesktopError::Offer(error.to_string()))?;
        let previous_manifest = &previous_offer.manifest;
        if previous_manifest.port_id != port_id
            || previous_manifest.game_id != game.game_id
            || previous_manifest.recipe.id != current_manifest.recipe.id
            || !previous_manifest
                .recipe
                .repo_url
                .eq_ignore_ascii_case(&current_manifest.recipe.repo_url)
        {
            return Err(DesktopError::Package(
                "previous offer does not belong to this installed game".into(),
            ));
        }
        let host = detect_host_platform()?;
        validate_install_directory(&previous, previous_manifest, &host)?;
        let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
        let tx = db.unchecked_transaction()?;
        let old_offer: String = tx.query_row(
            "SELECT offer_json FROM installs WHERE port_id=?1",
            params![port_id],
            |row| row.get(0),
        )?;
        let old_manifest: String = tx.query_row(
            "SELECT manifest_json FROM installs WHERE port_id=?1",
            params![port_id],
            |row| row.get(0),
        )?;
        let status = if self.data_is_ready(previous_manifest)? {
            InstallStatus::Ready
        } else {
            InstallStatus::InstalledNeedsData
        };
        tx.execute(
            "UPDATE installs SET active_path=?1,previous_path=?2,manifest_json=?3,previous_manifest_json=?4,offer_json=?5,previous_offer_json=?6,status=?7,error=NULL,
             suppressed_recipe_id=?8,suppressed_version=?9,suppressed_asset_id=?10,suppressed_sha256=?11 WHERE port_id=?12",
            params![relative(&self.root,&previous)?,relative(&self.root,&game.active_path)?,
                serde_json::to_string(previous_manifest).map_err(json_error)?,Some(old_manifest),previous_offer_json,old_offer,status.as_str(),
                current_manifest.recipe.id.as_str(),
                current_manifest.version.as_str(),
                current_manifest.asset_id,
                current_manifest.sha256.as_str(),port_id],
        )?;
        tx.commit()?;
        self.emit(port_id, "rolled_back", None, None, false);
        Ok(())
    }

    pub async fn uninstall(&self, port_id: i64) -> Result<(), DesktopError> {
        let _operation = self.begin_operation(port_id)?;
        let game = self
            .installed_one(port_id)?
            .ok_or(DesktopError::NotInstalled)?;
        if game.running {
            return Err(DesktopError::Running);
        }
        let operation_id = Uuid::new_v4().to_string();
        let active = relative(&self.root, &game.active_path)?;
        let previous = game
            .previous_path
            .as_ref()
            .map(|path| relative(&self.root, path))
            .transpose()?;
        owned_version_path(&self.root, port_id, &active)?;
        if let Some(path) = previous.as_deref() {
            owned_version_path(&self.root, port_id, path)?;
        }
        {
            let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
            db.execute("INSERT INTO operations(operation_id,port_id,kind,phase,part_path,staging_path,error,created_at) VALUES(?1,?2,'uninstall','uninstalling',?3,?4,NULL,?5)",
                params![operation_id,port_id,active,previous,Utc::now().to_rfc3339()])?;
        }
        self.emit_for(&operation_id, port_id, "uninstalling", None, None, false);
        for relative_path in [Some(active), previous].into_iter().flatten() {
            let path = owned_version_path(&self.root, port_id, &relative_path)?;
            if path.exists() {
                fs::remove_dir_all(&path)?;
            }
        }
        let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
        let tx = db.unchecked_transaction()?;
        tx.execute("DELETE FROM installs WHERE port_id=?1", params![port_id])?;
        tx.execute("DELETE FROM updates WHERE port_id=?1", params![port_id])?;
        tx.execute(
            "UPDATE operations SET phase='committed' WHERE operation_id=?1",
            params![operation_id],
        )?;
        tx.commit()?;
        self.emit(port_id, "complete", None, None, false);
        Ok(())
    }
    pub async fn launch(&self, port_id: i64) -> Result<(), DesktopError> {
        self.launch_inner(port_id, false).await
    }

    pub async fn launch_setup(&self, port_id: i64) -> Result<(), DesktopError> {
        self.launch_inner(port_id, true).await
    }

    pub async fn provide_original_data(
        &self,
        port_id: i64,
        selected_file: PathBuf,
    ) -> Result<(), DesktopError> {
        let _operation = self.begin_operation(port_id)?;
        let game = self
            .installed_one(port_id)?
            .ok_or(DesktopError::NotInstalled)?;
        if game.running {
            return Err(DesktopError::Running);
        }
        let manifest = game
            .manifest
            .as_ref()
            .ok_or_else(|| DesktopError::Package("installed metadata is damaged".into()))?;
        let adapter = manifest.recipe.data_adapter.as_deref().ok_or_else(|| {
            DesktopError::UnsupportedDataAdapter(
                "this game has no managed original-data setup".into(),
            )
        })?;
        if adapter != "banjo_n64_us_10" {
            return Err(DesktopError::UnsupportedDataAdapter(adapter.into()));
        }
        let destination = banjo_data_directory()?.join(BANJO_ROM_NAME);
        let _data_lock = data::lock_destination(&destination)?;
        match self.data_status(manifest)? {
            DataStatus::Ready => return self.set_data_state(port_id, true),
            DataStatus::Conflict => {
                return Err(DesktopError::DataConflict(format!(
                    "{} already contains invalid data; explicitly back up and replace it",
                    destination.display()
                )));
            }
            DataStatus::Missing => {}
            DataStatus::Unsupported => {
                return Err(DesktopError::UnsupportedDataAdapter(adapter.into()));
            }
            DataStatus::Error | DataStatus::NotRequired => {
                return Err(DesktopError::DataConflict(
                    "could not validate the original-data destination".into(),
                ));
            }
        }
        let normalized = data::normalize_banjo_file(&selected_file)?;
        data::replace_with_durable_backup(&destination, &normalized)?;
        self.set_data_state(port_id, true)
    }

    /// Replaces only the expected Banjo data file after the caller has made an explicit choice.
    pub async fn replace_original_data(
        &self,
        port_id: i64,
        selected_file: PathBuf,
    ) -> Result<PathBuf, DesktopError> {
        let _operation = self.begin_operation(port_id)?;
        let game = self
            .installed_one(port_id)?
            .ok_or(DesktopError::NotInstalled)?;
        if game.running {
            return Err(DesktopError::Running);
        }
        let manifest = game
            .manifest
            .as_ref()
            .ok_or_else(|| DesktopError::Package("installed metadata is damaged".into()))?;
        if manifest.recipe.data_adapter.as_deref() != Some("banjo_n64_us_10") {
            return Err(DesktopError::UnsupportedDataAdapter(
                "this game does not use Banjo original data".into(),
            ));
        }
        let normalized = data::normalize_banjo_file(&selected_file)?;
        let destination = banjo_data_directory()?.join(BANJO_ROM_NAME);
        let _data_lock = data::lock_destination(&destination)?;
        let backup = data::replace_with_durable_backup(&destination, &normalized)?;
        self.set_data_state(port_id, true)?;
        Ok(backup.unwrap_or(destination))
    }

    pub async fn check_updates(&self, offers: Vec<InstallOffer>) -> Result<(), DesktopError> {
        for offer in &offers {
            offer
                .validate()
                .map_err(|error| DesktopError::Offer(error.to_string()))?;
        }
        let installed = self.installed()?;
        let _guards = installed
            .iter()
            .map(|game| self.begin_operation(game.port_id))
            .collect::<Result<Vec<_>, _>>()?;
        let host = detect_host_platform()?;
        let now = Utc::now().to_rfc3339();
        let mut decisions = Vec::new();
        for game in installed {
            let Some(current_manifest) = game.manifest.as_ref() else {
                continue;
            };
            let current_version = Version::parse(&current_manifest.version)
                .map_err(|_| DesktopError::Offer("installed version is invalid".into()))?;
            let current_repo =
                recomphub_shared::parse_github_repository_url(&current_manifest.recipe.repo_url)
                    .map_err(|error| DesktopError::Offer(error.to_string()))?;
            let candidates: Vec<&InstallOffer> = offers
                .iter()
                .filter(|offer| {
                    if offer.manifest.port_id != game.port_id {
                        return false;
                    }
                    let Ok(repo) = recomphub_shared::parse_github_repository_url(
                        &offer.manifest.recipe.repo_url,
                    ) else {
                        return false;
                    };
                    offer.manifest.recipe.id == current_manifest.recipe.id
                        && repo.matches(&current_repo)
                        && offer.manifest.recipe.os == current_manifest.recipe.os
                        && offer.manifest.recipe.revision >= current_manifest.recipe.revision
                        && offer.manifest.recipe.architectures.contains(&host.arch)
                        && offer.manifest.matches_host(&host).unwrap_or(false)
                })
                .collect();
            let candidate = candidates.iter().copied().find(|offer| {
                offer.manifest.version == current_manifest.version
                    && (offer.manifest.release_id != current_manifest.release_id
                        || offer.manifest.tag != current_manifest.tag
                        || offer.manifest.asset_id != current_manifest.asset_id
                        || offer.manifest.size_bytes != current_manifest.size_bytes
                        || !offer
                            .manifest
                            .sha256
                            .eq_ignore_ascii_case(&current_manifest.sha256))
            });
            if candidate.is_some() {
                return Err(DesktopError::Offer(
                    "Upstream asset changed; review required".into(),
                ));
            }
            let candidate = candidates
                .into_iter()
                .filter(|offer| {
                    Version::parse(&offer.manifest.version)
                        .is_ok_and(|version| version > current_version)
                })
                .max_by_key(|offer| {
                    Version::parse(&offer.manifest.version).expect("validated stable version")
                });
            decisions.push((
                game.port_id,
                candidate
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(json_error)?,
            ));
        }
        let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
        let tx = db.unchecked_transaction()?;
        for (port_id, candidate) in decisions {
            tx.execute("DELETE FROM updates WHERE port_id=?1", params![port_id])?;
            if let Some(offer_json) = candidate {
                tx.execute(
                    "INSERT INTO updates(port_id,offer_json,checked_at) VALUES(?1,?2,?3)",
                    params![port_id, offer_json, now],
                )?;
            }
            tx.execute(
                "UPDATE installs SET last_checked_at=?1 WHERE port_id=?2",
                params![now, port_id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn installed(&self) -> Result<Vec<InstalledGame>, DesktopError> {
        self.reap_children()?;
        let rows = {
            let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
            let mut statement = db.prepare("SELECT port_id, game_id, game_name, game_slug, port_name, port_slug, manifest_json, active_path, previous_path, status, error, offer_json, last_checked_at FROM installs ORDER BY game_name, port_name")?;
            statement
                .query_map([], install_row)?
                .collect::<Result<Vec<_>, _>>()?
        };
        rows.into_iter()
            .map(|row| self.row_to_installed(row))
            .collect()
    }

    fn installed_one(&self, port_id: i64) -> Result<Option<InstalledGame>, DesktopError> {
        self.reap_children()?;
        let row = {
            let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
            db.query_row(
                "SELECT port_id, game_id, game_name, game_slug, port_name, port_slug, manifest_json, active_path, previous_path, status, error, offer_json, last_checked_at FROM installs WHERE port_id = ?1",
                params![port_id],
                install_row,
            )
            .optional()?
        };
        row.map(|row| self.row_to_installed(row)).transpose()
    }

    async fn install_offer(&self, offer: InstallOffer, updating: bool) -> Result<(), DesktopError> {
        offer
            .validate()
            .map_err(|error| DesktopError::Offer(error.to_string()))?;
        let host = detect_host_platform()?;
        if !offer
            .manifest
            .matches_host(&host)
            .map_err(|error| DesktopError::Offer(error.to_string()))?
        {
            return Err(DesktopError::UnsupportedHost(
                "the selected build does not match this machine".into(),
            ));
        }
        check_runtime_requirements(&host, &offer.manifest)?;
        let port_id = offer.manifest.port_id;
        let _operation = self.begin_operation(port_id)?;
        let current = self.installed_one(port_id)?;
        if current.as_ref().is_some_and(|game| game.running) {
            return Err(DesktopError::Running);
        }
        if updating {
            if let Some(game) = current.as_ref() {
                let installed = game
                    .manifest
                    .as_ref()
                    .ok_or_else(|| DesktopError::Package("installed metadata is damaged".into()))?;
                let old_repo =
                    recomphub_shared::parse_github_repository_url(&installed.recipe.repo_url)
                        .map_err(|error| DesktopError::Offer(error.to_string()))?;
                let new_repo =
                    recomphub_shared::parse_github_repository_url(&offer.manifest.recipe.repo_url)
                        .map_err(|error| DesktopError::Offer(error.to_string()))?;
                if installed.recipe.id != offer.manifest.recipe.id
                    || !old_repo.matches(&new_repo)
                    || installed.recipe.os != offer.manifest.recipe.os
                    || offer.manifest.recipe.revision < installed.recipe.revision
                {
                    return Err(DesktopError::Offer(
                        "offer does not match the installed reviewed recipe".into(),
                    ));
                }
                let old_version = Version::parse(&installed.version)
                    .map_err(|_| DesktopError::Offer("installed version is invalid".into()))?;
                let new_version = Version::parse(&offer.manifest.version)
                    .map_err(|_| DesktopError::Offer("candidate version is invalid".into()))?;
                if new_version < old_version
                    || (new_version == old_version
                        && (offer.manifest.release_id != installed.release_id
                            || offer.manifest.tag != installed.tag
                            || offer.manifest.asset_id != installed.asset_id
                            || offer.manifest.size_bytes != installed.size_bytes
                            || !offer
                                .manifest
                                .sha256
                                .eq_ignore_ascii_case(&installed.sha256)))
                {
                    return Err(DesktopError::Offer(
                        "Upstream asset changed; review required".into(),
                    ));
                }
            } else {
                return Err(DesktopError::NotInstalled);
            }
        }
        let operation_id = Uuid::new_v4().to_string();
        let package = self
            .root
            .join("downloads")
            .join(format!("{operation_id}.part"));
        let staging = self.root.join("staging").join(&operation_id);
        let final_dir = self.version_directory(&offer.manifest, &operation_id);
        managed_path(&self.root, &relative(&self.root, &package)?)?;
        managed_path(&self.root, &relative(&self.root, &staging)?)?;
        self.create_operation(
            &operation_id,
            if updating { "update" } else { "install" },
            &offer,
            &package,
            &staging,
            &final_dir,
        )?;
        self.emit_for(&operation_id, port_id, "queued", None, None, true);
        let install_result: Result<(), DesktopError> = async {
            let _download_slot = self.acquire_download_slot(port_id).await?;
            self.set_operation_phase(&operation_id, "downloading", None)?;
            self.download_package(&offer, &package, &operation_id)
                .await?;
            managed_path(&self.root, &relative(&self.root, &package)?)?;
            platform::mark_download_source(&package, &offer.manifest.download_url)?;
            self.set_operation_phase(&operation_id, "verifying", None)?;
            self.emit_for(&operation_id, port_id, "verifying", None, None, true);
            check_cancelled(&self.cancellations, port_id)?;
            self.set_operation_phase(&operation_id, "extracting", None)?;
            self.emit_for(&operation_id, port_id, "extracting", None, None, true);
            managed_path(&self.root, &relative(&self.root, &staging)?)?;
            extract_package(
                &package,
                &staging,
                &offer.manifest,
                Some((&self.cancellations, port_id)),
            )?;
            check_cancelled(&self.cancellations, port_id)?;
            reject_portable_marker(&staging, &offer.manifest)?;
            write_manifest_file(&staging, &offer.manifest)?;
            validate_install_directory(&staging, &offer.manifest, &host)?;
            let extracted_executable = staging.join(&offer.manifest.recipe.launch.executable);
            platform::mark_download_source(&extracted_executable, &offer.manifest.download_url)?;
            #[cfg(target_os = "macos")]
            if let Some(app_bundle) = extracted_executable.ancestors().find(|path| {
                path.extension().is_some_and(|extension| {
                    extension.to_string_lossy().eq_ignore_ascii_case("app")
                })
            }) {
                platform::mark_download_source(app_bundle, &offer.manifest.download_url)?;
            }
            sync_tree(&staging)?;
            managed_path(&self.root, &relative(&self.root, &staging)?)?;
            ensure_versions_directory(&self.root, port_id)?;
            owned_version_path(&self.root, port_id, &relative(&self.root, &final_dir)?)?;
            self.enter_activation(&operation_id, port_id)?;
            self.set_operation_phase(&operation_id, "renamed", None)?;
            fs::rename(&staging, &final_dir)?;
            sync_parent(final_dir.parent().expect("version directory parent"))?;
            self.commit_install(&offer, &final_dir, &operation_id)?;
            Ok(())
        }
        .await;
        if let Ok(path) = managed_path(&self.root, &relative(&self.root, &package)?)
            && path.exists()
        {
            let _ = fs::remove_file(path);
        }
        if let Ok(path) = managed_path(&self.root, &relative(&self.root, &staging)?)
            && path.exists()
        {
            let _ = fs::remove_dir_all(path);
        }
        self.cancellations
            .lock()
            .map_err(|_| DesktopError::Poisoned)?
            .remove(&port_id);
        if let Err(error) = install_result {
            let committed = self.is_operation_committed(&operation_id)?;
            if !committed {
                if let Ok(path) =
                    owned_version_path(&self.root, port_id, &relative(&self.root, &final_dir)?)
                    && path.exists()
                {
                    fs::remove_dir_all(path)?;
                }
                let phase = if matches!(&error, DesktopError::Cancelled) {
                    "cancelled"
                } else {
                    "failed"
                };
                self.set_operation_phase(&operation_id, phase, Some(&error.to_string()))?;
                self.emit_for(&operation_id, port_id, phase, None, None, false);
            }
            return Err(error);
        }
        self.emit_for(&operation_id, port_id, "committed", None, None, false);
        Ok(())
    }

    async fn download_package(
        &self,
        offer: &InstallOffer,
        target: &Path,
        operation_id: &str,
    ) -> Result<(), DesktopError> {
        validate_download_url(
            &offer.manifest.download_url,
            &offer.manifest.recipe.repo_url,
        )?;
        #[cfg(test)]
        let download_url = self
            .test_download_endpoint
            .as_ref()
            .map(url::Url::as_str)
            .unwrap_or(&offer.manifest.download_url);
        #[cfg(not(test))]
        let download_url = &offer.manifest.download_url;
        let response = self.downloads.get(download_url).send().await?;
        if !response.status().is_success() {
            return Err(DesktopError::Package(format!(
                "package server returned HTTP {}",
                response.status()
            )));
        }
        if response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.to_ascii_lowercase().starts_with("text/html"))
        {
            return Err(DesktopError::Package(
                "package server returned HTML instead of an archive".into(),
            ));
        }
        let declared = response.content_length();
        if declared.is_some_and(|size| size != offer.manifest.size_bytes || size > PACKAGE_CAP) {
            return Err(DesktopError::Package(
                "package response has an unexpected size".into(),
            ));
        }
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)
            .await?;
        let mut stream = response.bytes_stream();
        let mut hash = Sha256::new();
        let mut received = 0_u64;
        while let Some(chunk) = tokio::time::timeout(Duration::from_secs(45), stream.next())
            .await
            .map_err(|_| DesktopError::Package("package download stalled".into()))?
        {
            check_cancelled(&self.cancellations, offer.manifest.port_id)?;
            let chunk = chunk?;
            received = received
                .checked_add(chunk.len() as u64)
                .ok_or_else(|| DesktopError::Package("package is too large".into()))?;
            if received > PACKAGE_CAP || received > offer.manifest.size_bytes {
                return Err(DesktopError::Package(
                    "package exceeds its declared size".into(),
                ));
            }
            hash.update(&chunk);
            tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
            self.emit_for(
                operation_id,
                offer.manifest.port_id,
                "downloading",
                Some(received),
                Some(offer.manifest.size_bytes),
                true,
            );
        }
        tokio::io::AsyncWriteExt::flush(&mut file).await?;
        if received != offer.manifest.size_bytes {
            return Err(DesktopError::Package(
                "downloaded package size does not match manifest".into(),
            ));
        }
        if !hex(&hash.finalize()).eq_ignore_ascii_case(&offer.manifest.sha256) {
            return Err(DesktopError::Package(
                "package SHA-256 does not match manifest".into(),
            ));
        }
        Ok(())
    }
    async fn acquire_download_slot(
        &self,
        port_id: i64,
    ) -> Result<DownloadPermit<'_>, DesktopError> {
        {
            let mut queue = self.queue.lock().map_err(|_| DesktopError::Poisoned)?;
            if !queue.contains(&port_id) {
                queue.push(port_id);
            }
        }
        loop {
            if let Err(error) = check_cancelled(&self.cancellations, port_id) {
                if let Ok(mut queue) = self.queue.lock() {
                    queue.retain(|queued| *queued != port_id);
                }
                return Err(error);
            }
            let acquired = {
                let mut active = self
                    .downloads_active
                    .lock()
                    .map_err(|_| DesktopError::Poisoned)?;
                let mut queue = self.queue.lock().map_err(|_| DesktopError::Poisoned)?;
                if !*active && queue.first() == Some(&port_id) {
                    queue.remove(0);
                    *active = true;
                    true
                } else {
                    false
                }
            };
            if acquired {
                return Ok(DownloadPermit {
                    active: &self.downloads_active,
                });
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn launch_inner(&self, port_id: i64, setup: bool) -> Result<(), DesktopError> {
        let _operation = self.begin_operation(port_id)?;
        let game = self
            .installed_one(port_id)?
            .ok_or(DesktopError::NotInstalled)?;
        if game.running {
            return Err(DesktopError::Running);
        }
        if !game.active_path.exists() {
            self.set_status(
                port_id,
                InstallStatus::RepairNeeded,
                Some("active package files are missing".into()),
            )?;
            return Err(DesktopError::Package(
                "active package files are missing".into(),
            ));
        }
        let manifest = game.manifest.as_ref().ok_or_else(|| {
            DesktopError::Package("installed metadata is damaged; online reinstall required".into())
        })?;
        let host = detect_host_platform()?;
        if !manifest
            .matches_host(&host)
            .map_err(|error| DesktopError::Offer(error.to_string()))?
        {
            return Err(DesktopError::UnsupportedHost(
                "the installed build no longer matches this machine".into(),
            ));
        }
        let _owned_active = owned_version_path(
            &self.root,
            port_id,
            &relative(&self.root, &game.active_path)?,
        )?;
        if let Err(error) = validate_install_directory(&game.active_path, manifest, &host) {
            self.set_status(
                port_id,
                InstallStatus::RepairNeeded,
                Some(error.to_string()),
            )?;
            return Err(error);
        }
        let status = self.data_status(manifest)?;
        match status {
            DataStatus::Missing if !setup => {
                self.set_status(port_id, InstallStatus::InstalledNeedsData, None)?;
                return Err(DesktopError::NeedsData);
            }
            DataStatus::Conflict => {
                return Err(DesktopError::DataConflict(
                    "the original-data destination contains an invalid file".into(),
                ));
            }
            DataStatus::Unsupported => {
                return Err(DesktopError::UnsupportedDataAdapter(
                    manifest.recipe.data_adapter.clone().unwrap_or_default(),
                ));
            }
            DataStatus::Error => {
                return Err(DesktopError::DataConflict(
                    "could not validate the original-data destination".into(),
                ));
            }
            _ => {}
        }
        let data_present = status == DataStatus::Ready || status == DataStatus::NotRequired;
        if !setup && !data_present {
            return Err(DesktopError::NeedsData);
        }
        if setup && status != DataStatus::Missing {
            return Err(DesktopError::DataConflict(
                "original data is already present; use Play".into(),
            ));
        }
        reject_portable_marker(&game.active_path, manifest)?;
        let offer = self.load_offer(port_id)?;
        let operation_id = Uuid::new_v4().to_string();
        let executable = game
            .active_path
            .join(&manifest.recipe.launch.executable)
            .canonicalize()?;
        #[cfg(not(windows))]
        let working_directory = game
            .active_path
            .join(&manifest.recipe.launch.working_directory)
            .canonicalize()?;
        self.create_operation(
            &operation_id,
            "launch",
            &offer,
            &self
                .root
                .join("downloads")
                .join(format!("{operation_id}.part")),
            &self.root.join("staging").join(&operation_id),
            &game.active_path,
        )?;
        self.set_operation_phase(&operation_id, "launching", None)?;
        #[cfg(windows)]
        let launch_result =
            platform::launch_process_verified(&self.root, &game.active_path, manifest);
        #[cfg(not(windows))]
        let launch_result = platform::launch_process(
            &executable,
            &working_directory,
            &manifest.recipe.launch.arguments,
        );
        let mut child = match launch_result {
            Ok(child) => child,
            Err(error) => {
                if self.record_launch_failure(&operation_id, port_id, &error)? {
                    return Ok(());
                }
                return Err(error);
            }
        };
        let pid = child.id();
        #[cfg(windows)]
        let start_time = child.started_at;
        #[cfg(not(windows))]
        let start_time = match platform::process_start_time(pid, &executable) {
            Ok(value) => value,
            Err(error) => {
                if let Err(stop_error) = child.terminate_and_wait() {
                    let message = format!("Launch outcome is uncertain: {stop_error}");
                    self.mark_launch_uncertain(&operation_id, port_id, &message)?;
                    self.tracked_processes
                        .lock()
                        .map_err(|_| DesktopError::Poisoned)?
                        .insert(port_id);
                    return Err(stop_error);
                }
                self.set_operation_phase(&operation_id, "failed", Some(&error.to_string()))?;
                return Err(error);
            }
        };
        let persistence =
            self.persist_running_process(port_id, pid, start_time, &executable, &operation_id);
        if let Err(error) = persistence {
            if let Err(stop_error) = child.terminate_and_wait() {
                let message = format!("Launch outcome is uncertain: {stop_error}");
                self.mark_launch_uncertain(&operation_id, port_id, &message)?;
                self.tracked_processes
                    .lock()
                    .map_err(|_| DesktopError::Poisoned)?
                    .insert(port_id);
                return Err(stop_error);
            }
            self.set_operation_phase(&operation_id, "failed", Some(&error.to_string()))?;
            return Err(error);
        }
        let identity = ProcessIdentity {
            port_id,
            pid: pid as i64,
            started_at: start_time as i64,
            executable: executable.to_string_lossy().into_owned(),
        };
        self.running
            .lock()
            .map_err(|_| DesktopError::Poisoned)?
            .insert(port_id, RunningProcess { child, identity });
        self.set_status(port_id, InstallStatus::Running, None)?;
        self.emit_for(&operation_id, port_id, "running", None, None, false);
        Ok(())
    }
    fn persist_running_process(
        &self,
        port_id: i64,
        pid: u32,
        started_at: u64,
        executable: &Path,
        operation_id: &str,
    ) -> Result<(), DesktopError> {
        let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
        let tx = db.unchecked_transaction()?;
        tx.execute("INSERT OR REPLACE INTO running_processes(port_id,pid,started_at,executable_path) VALUES(?1,?2,?3,?4)",
            params![port_id,pid as i64,started_at as i64,executable.to_string_lossy()])?;
        tx.execute(
            "UPDATE installs SET status='running',error=NULL WHERE port_id=?1",
            params![port_id],
        )?;
        tx.execute(
            "UPDATE operations SET phase='committed',error=NULL WHERE operation_id=?1",
            params![operation_id],
        )?;
        tx.commit()?;
        self.tracked_processes
            .lock()
            .map_err(|_| DesktopError::Poisoned)?
            .insert(port_id);
        Ok(())
    }
    fn recover_launch_operation(
        &self,
        operation_id: &str,
        port_id: i64,
    ) -> Result<bool, DesktopError> {
        if recover_launch_operation(
            &self.root,
            &self.tracked_processes,
            &self.db,
            operation_id,
            port_id,
        )? {
            return Ok(true);
        }
        let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
        Ok(db.query_row(
            "SELECT EXISTS(SELECT 1 FROM operations o WHERE o.operation_id=?1 AND o.port_id=?2 AND o.kind='launch' AND o.phase='committed')",
            params![operation_id,port_id],|row| row.get(0),
        )?)
    }
    fn create_operation(
        &self,
        operation_id: &str,
        kind: &str,
        offer: &InstallOffer,
        part: &Path,
        staging: &Path,
        final_path: &Path,
    ) -> Result<(), DesktopError> {
        let port_id = offer.manifest.port_id;
        let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
        let prior: Option<String> = db
            .query_row(
                "SELECT offer_json FROM installs WHERE port_id=?1",
                params![port_id],
                |row| row.get(0),
            )
            .optional()?;
        db.execute(
            "INSERT INTO operations(operation_id,port_id,kind,phase,prior_offer_json,candidate_offer_json,part_path,staging_path,final_path,error,created_at)
             VALUES(?1,?2,?3,'queued',?4,?5,?6,?7,?8,NULL,?9)",
            params![operation_id, port_id, kind, prior, serde_json::to_string(offer).map_err(json_error)?,
                relative(&self.root, part)?, relative(&self.root, staging)?, relative(&self.root, final_path)?, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    fn set_operation_phase(
        &self,
        operation_id: &str,
        phase: &str,
        error: Option<&str>,
    ) -> Result<(), DesktopError> {
        let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
        db.execute(
            "UPDATE operations SET phase=?1,error=?2 WHERE operation_id=?3",
            params![phase, error, operation_id],
        )?;
        Ok(())
    }
    fn enter_activation(&self, operation_id: &str, port_id: i64) -> Result<(), DesktopError> {
        let cancellations = self
            .cancellations
            .lock()
            .map_err(|_| DesktopError::Poisoned)?;
        if cancellations.contains(&port_id) {
            return Err(DesktopError::Cancelled);
        }
        let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
        db.execute(
            "UPDATE operations SET phase='validated',error=NULL WHERE operation_id=?1",
            params![operation_id],
        )?;
        Ok(())
    }
    fn is_operation_committed(&self, operation_id: &str) -> Result<bool, DesktopError> {
        let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
        let phase: String = db.query_row(
            "SELECT phase FROM operations WHERE operation_id=?1",
            params![operation_id],
            |row| row.get(0),
        )?;
        Ok(phase == "committed")
    }

    fn version_directory(&self, manifest: &InstallManifest, operation_id: &str) -> PathBuf {
        self.root
            .join("games")
            .join(manifest.port_id.to_string())
            .join("versions")
            .join(format!("{}-{operation_id}", manifest.version))
    }

    fn commit_install(
        &self,
        offer: &InstallOffer,
        final_dir: &Path,
        operation_id: &str,
    ) -> Result<(), DesktopError> {
        owned_version_path(
            &self.root,
            offer.manifest.port_id,
            &relative(&self.root, final_dir)?,
        )?;
        let existing = self.installed_one(offer.manifest.port_id)?;
        let known_active = existing.as_ref().and_then(|game| {
            let manifest = game.manifest.as_ref()?;
            let path = relative(&self.root, &game.active_path).ok()?;
            let package = owned_version_path(&self.root, offer.manifest.port_id, &path).ok()?;
            if read_manifest_file(&package).ok().as_ref() != Some(manifest) {
                return None;
            }
            let old_offer = self.load_offer(offer.manifest.port_id).ok()?;
            if old_offer.validate().is_err()
                || old_offer.manifest != *manifest
                || old_offer.game.id != game.game_id
                || old_offer.game.name != game.game_name
                || old_offer.game.slug != game.game_slug
                || old_offer.port.id != game.port_id
                || old_offer.port.name != game.port_name
                || old_offer.port.slug != game.port_slug
            {
                return None;
            }
            Some((path, package, manifest, old_offer))
        });
        let previous_snapshot =
            known_active
                .as_ref()
                .and_then(|(path, package, manifest, old_offer)| {
                    let host = detect_host_platform().ok()?;
                    if validate_install_directory(package, manifest, &host).is_err()
                        || self.is_package_running_path(package).ok()?
                    {
                        return None;
                    }
                    Some((
                        path.as_str(),
                        serde_json::to_string(manifest).ok()?,
                        serde_json::to_string(old_offer).ok()?,
                    ))
                });
        // A repaired package can be unhealthy without losing its ownership proof.
        // Remove that superseded package after activation, but never guess at damaged metadata.
        let obsolete_active = known_active
            .as_ref()
            .filter(|_| previous_snapshot.is_none())
            .map(|(_, package, _, _)| package.as_path());
        let previous = previous_snapshot.as_ref().map(|(path, _, _)| *path);
        let previous_manifest = previous_snapshot
            .as_ref()
            .map(|(_, manifest, _)| manifest.as_str());
        let previous_offer = previous_snapshot
            .as_ref()
            .map(|(_, _, offer)| offer.as_str());
        let old_previous = existing
            .as_ref()
            .and_then(|game| game.previous_path.as_deref());
        let status = if self.data_is_ready(&offer.manifest)? {
            InstallStatus::Ready
        } else {
            InstallStatus::InstalledNeedsData
        };
        let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
        let tx = db.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO installs(port_id,game_id,game_name,game_slug,port_name,port_slug,manifest_json,active_path,previous_path,status,error,offer_json,last_checked_at,previous_manifest_json,previous_offer_json)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,NULL,?11,NULL,?12,?13)
             ON CONFLICT(port_id) DO UPDATE SET game_id=excluded.game_id,game_name=excluded.game_name,game_slug=excluded.game_slug,port_name=excluded.port_name,port_slug=excluded.port_slug,
               previous_manifest_json=excluded.previous_manifest_json, previous_offer_json=excluded.previous_offer_json,
               manifest_json=excluded.manifest_json,active_path=excluded.active_path,previous_path=excluded.previous_path,status=excluded.status,error=NULL,offer_json=excluded.offer_json,
               suppressed_recipe_id=NULL,suppressed_version=NULL,suppressed_asset_id=NULL,suppressed_sha256=NULL",
            params![offer.manifest.port_id,offer.manifest.game_id,offer.game.name,offer.game.slug,offer.port.name,offer.port.slug,
                serde_json::to_string(&offer.manifest).map_err(json_error)?,relative(&self.root,final_dir)?,previous,status.as_str(),
                serde_json::to_string(offer).map_err(json_error)?,previous_manifest,previous_offer],
        )?;
        // Ensure the journal and active pointer become visible in one SQLite commit.
        tx.execute(
            "UPDATE operations SET phase='committed',error=NULL WHERE operation_id=?1",
            params![operation_id],
        )?;
        tx.execute(
            "DELETE FROM updates WHERE port_id=?1",
            params![offer.manifest.port_id],
        )?;
        tx.commit()?;
        drop(db);
        for old in [old_previous, obsolete_active].into_iter().flatten() {
            let old_relative = relative(&self.root, old)?;
            if old == final_dir || previous == Some(old_relative.as_str()) {
                continue;
            }
            if let Ok(old) = owned_version_path(&self.root, offer.manifest.port_id, &old_relative)
                && old.exists()
                && !self.is_package_running_path(&old)?
            {
                let _ = fs::remove_dir_all(old);
            }
        }
        Ok(())
    }

    fn row_to_installed(&self, row: InstallRow) -> Result<InstalledGame, DesktopError> {
        let mut manifest = serde_json::from_str::<InstallManifest>(&row.manifest_json)
            .ok()
            .filter(|manifest| manifest.validate().is_ok());
        let active_offer = serde_json::from_str::<InstallOffer>(&row.offer_json)
            .ok()
            .filter(|offer| offer.validate().is_ok())
            .filter(|offer| {
                manifest
                    .as_ref()
                    .is_some_and(|manifest| &offer.manifest == manifest)
                    && offer.game.id == row.game_id
                    && offer.game.name == row.game_name
                    && offer.game.slug == row.game_slug
                    && offer.port.id == row.port_id
                    && offer.port.name == row.port_name
                    && offer.port.slug == row.port_slug
            });
        if active_offer.is_none() {
            manifest = None;
        }
        let active_result = owned_version_path(&self.root, row.port_id, &row.active_path);
        let previous_result = row
            .previous_path
            .as_deref()
            .map(|path| owned_version_path(&self.root, row.port_id, path))
            .transpose();
        let path_damaged = active_result.is_err() || previous_result.is_err();
        if path_damaged {
            manifest = None;
        }
        let metadata_damaged = manifest.is_none();
        let active_path = active_result.unwrap_or_else(|_| {
            self.root
                .join("damaged-entry")
                .join(row.port_id.to_string())
        });
        let previous_path = previous_result.ok().flatten();
        let running = self.is_running(row.port_id)?;
        let (data_status, data_ready) = match manifest.as_ref() {
            None => (DataStatus::Error, false),
            Some(manifest) => match self.data_status(manifest) {
                Ok(status) => (
                    status,
                    status == DataStatus::Ready || status == DataStatus::NotRequired,
                ),
                Err(_) => (DataStatus::Error, false),
            },
        };
        let launch_healthy = manifest.as_ref().is_some_and(|manifest| {
            detect_host_platform()
                .is_ok_and(|host| validate_install_directory(&active_path, manifest, &host).is_ok())
        });
        let status = if running {
            InstallStatus::Running
        } else if metadata_damaged || !launch_healthy {
            InstallStatus::RepairNeeded
        } else if row.status == "running"
            || row.status == "ready"
            || row.status == "installed_needs_data"
        {
            if data_ready {
                InstallStatus::Ready
            } else {
                InstallStatus::InstalledNeedsData
            }
        } else {
            InstallStatus::parse(&row.status)
        };
        let update_available = self.update_for(row.port_id)?;
        let update_suppressed = update_available
            .as_ref()
            .map(|candidate| self.update_is_suppressed(row.port_id, candidate))
            .transpose()?
            .unwrap_or(false);
        Ok(InstalledGame {
            port_id: row.port_id,
            game_id: row.game_id,
            game_name: row.game_name,
            game_slug: row.game_slug,
            port_name: row.port_name,
            port_slug: row.port_slug,
            manifest,
            active_path,
            previous_path,
            status,
            running,
            data_ready,
            data_status,
            error: row.error.or_else(|| metadata_damaged.then(|| "Metadata damaged".into()))
                .or_else(|| (!launch_healthy && !metadata_damaged).then(|| "Repair needed: configured executable or working directory is unavailable".into())),
            update_available,
            update_suppressed,
            last_checked_at: row.last_checked_at.as_deref()
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.with_timezone(&Utc)),
        })
    }

    fn update_for(&self, port_id: i64) -> Result<Option<InstallManifest>, DesktopError> {
        let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
        let offer = db
            .query_row(
                "SELECT offer_json FROM updates WHERE port_id=?1",
                params![port_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(offer
            .and_then(|json| serde_json::from_str::<InstallOffer>(&json).ok())
            .filter(|offer| offer.validate().is_ok())
            .map(|offer| offer.manifest))
    }
    fn update_is_suppressed(
        &self,
        port_id: i64,
        candidate: &InstallManifest,
    ) -> Result<bool, DesktopError> {
        let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
        let suppressed: (Option<String>, Option<String>, Option<i64>, Option<String>) = db.query_row(
            "SELECT suppressed_recipe_id,suppressed_version,suppressed_asset_id,suppressed_sha256 FROM installs WHERE port_id=?1",
            params![port_id],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
        )?;
        Ok(
            suppressed.0.as_deref() == Some(candidate.recipe.id.as_str())
                && suppressed.1.as_deref() == Some(candidate.version.as_str())
                && suppressed.2 == Some(candidate.asset_id)
                && suppressed
                    .3
                    .as_deref()
                    .is_some_and(|digest| digest.eq_ignore_ascii_case(&candidate.sha256)),
        )
    }

    fn load_offer(&self, port_id: i64) -> Result<InstallOffer, DesktopError> {
        let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
        let json: String = db.query_row(
            "SELECT offer_json FROM installs WHERE port_id = ?1",
            params![port_id],
            |row| row.get(0),
        )?;
        serde_json::from_str(&json).map_err(json_error)
    }

    fn set_status(
        &self,
        port_id: i64,
        status: InstallStatus,
        error: Option<String>,
    ) -> Result<(), DesktopError> {
        let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
        db.execute(
            "UPDATE installs SET status = ?1, error = ?2 WHERE port_id = ?3",
            params![status.as_str(), error, port_id],
        )?;
        Ok(())
    }
    fn record_launch_failure(
        &self,
        operation_id: &str,
        port_id: i64,
        error: &DesktopError,
    ) -> Result<bool, DesktopError> {
        match error {
            DesktopError::LaunchUncertain(_) => {
                match self.recover_launch_operation(operation_id, port_id) {
                    Ok(true) => Ok(true),
                    Ok(false) => {
                        self.mark_launch_uncertain(operation_id,port_id,"Launch outcome is uncertain; verified process identity was not recovered")?;
                        Ok(false)
                    }
                    Err(recovery_error) => {
                        self.mark_launch_uncertain(
                            operation_id,
                            port_id,
                            &format!("Launch outcome is uncertain: {recovery_error}"),
                        )?;
                        Ok(false)
                    }
                }
            }
            _ => {
                self.set_operation_phase(operation_id, "failed", Some(&error.to_string()))?;
                Ok(false)
            }
        }
    }
    fn mark_launch_uncertain(
        &self,
        operation_id: &str,
        port_id: i64,
        message: &str,
    ) -> Result<(), DesktopError> {
        self.set_operation_phase(operation_id, "launching", Some(message))?;
        self.set_status(port_id, InstallStatus::Failed, Some(message.to_owned()))
    }

    fn set_data_state(&self, port_id: i64, ready: bool) -> Result<(), DesktopError> {
        self.set_status(
            port_id,
            if ready {
                InstallStatus::Ready
            } else {
                InstallStatus::InstalledNeedsData
            },
            None,
        )
    }

    fn data_status(&self, manifest: &InstallManifest) -> Result<DataStatus, DesktopError> {
        match manifest.recipe.data_adapter.as_deref() {
            None => Ok(DataStatus::NotRequired),
            Some("banjo_n64_us_10") => {
                let path = banjo_data_directory()?.join(BANJO_ROM_NAME);
                data::stored_banjo_status(&path)
            }
            Some(_) => Ok(DataStatus::Unsupported),
        }
    }

    fn data_is_ready(&self, manifest: &InstallManifest) -> Result<bool, DesktopError> {
        Ok(matches!(
            self.data_status(manifest)?,
            DataStatus::NotRequired | DataStatus::Ready
        ))
    }

    fn is_running(&self, port_id: i64) -> Result<bool, DesktopError> {
        Ok(self
            .running
            .lock()
            .map_err(|_| DesktopError::Poisoned)?
            .contains_key(&port_id)
            || self
                .tracked_processes
                .lock()
                .map_err(|_| DesktopError::Poisoned)?
                .contains(&port_id))
    }

    fn is_package_running_path(&self, path: &Path) -> Result<bool, DesktopError> {
        let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
        let mut statement = db.prepare("SELECT executable_path FROM running_processes")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows {
            if Path::new(&row?).starts_with(path) {
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn start_process_monitor(
        &self,
        root: PathBuf,
    ) -> Result<std::thread::JoinHandle<()>, DesktopError> {
        let running = Arc::downgrade(&self.running);
        let tracked = Arc::downgrade(&self.tracked_processes);
        let db = Arc::downgrade(&self.db);
        let stop = Arc::clone(&self.process_monitor_stop);
        let lock_guard = Arc::clone(&self._lock);
        let handle = std::thread::Builder::new()
            .name("recomphub-process-monitor".into())
            .spawn(move || {
                let _lock_guard = lock_guard;
                loop {
                    if stop.load(Ordering::Acquire) { break; }
                    let (Some(running), Some(tracked), Some(db)) = (running.upgrade(), tracked.upgrade(), db.upgrade()) else { break; };
                    if let Err(error) = recover_pending_launches(&root, &tracked, &db)
                        && let Ok(database) = db.lock() {
                            let message = format!("Launch outcome is uncertain: {error}");
                            let _ = database.execute(
                                "UPDATE operations SET error=?1 WHERE kind='launch' AND phase='launching'",
                                params![message],
                            );
                            let _ = database.execute(
                                "UPDATE installs SET status='failed',error='Launch outcome is uncertain' WHERE port_id IN (SELECT port_id FROM operations WHERE kind='launch' AND phase='launching')",
                                [],
                            );
                        }
                    if let Err(error) = monitor_processes(&root, &running, &tracked, &db)
                        && let Ok(database) = db.lock() {
                            let _ = database.execute(
                                "UPDATE installs SET error=?1 WHERE port_id IN (SELECT port_id FROM running_processes)",
                                params![format!("Game process monitoring failed: {error}")],
                            );
                        }
                    drop((running, tracked, db));
                    if stop.load(Ordering::Acquire) { break; }
                    std::thread::sleep(Duration::from_secs(1));
                }
            })
            .map_err(DesktopError::Io)?;
        Ok(handle)
    }

    fn reap_children(&self) -> Result<(), DesktopError> {
        let records = self.process_records()?;
        let completed = {
            let mut running = self.running.lock().map_err(|_| DesktopError::Poisoned)?;
            let mut completed = Vec::new();
            for process in running.values_mut() {
                if let Some(status) = process.child.try_wait()? {
                    completed.push((process.identity.clone(), status.code()));
                }
            }
            for (identity, _) in &completed {
                if running
                    .get(&identity.port_id)
                    .is_some_and(|process| process.identity == *identity)
                {
                    running.remove(&identity.port_id);
                }
            }
            completed
        };
        for (identity, exit_code) in completed {
            if records.iter().any(|record| record == &identity) {
                let notice = exit_code
                    .filter(|code| *code != 0)
                    .map(|code| format!("Game exited with code {code}"));
                self.clear_process_record(&identity, notice.as_deref())?;
            }
        }
        for identity in self.process_records()? {
            let pid = u32::try_from(identity.pid)
                .map_err(|_| DesktopError::Package("stored process ID is invalid".into()))?;
            if platform::process_identity_matches(
                pid,
                identity.started_at as u64,
                Path::new(&identity.executable),
            )? {
                self.tracked_processes
                    .lock()
                    .map_err(|_| DesktopError::Poisoned)?
                    .insert(identity.port_id);
            } else {
                self.clear_process_record(
                    &identity,
                    Some("Game exited; exit code unavailable after manager restart"),
                )?;
            }
        }
        Ok(())
    }
    fn process_records(&self) -> Result<Vec<ProcessIdentity>, DesktopError> {
        let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
        let mut statement =
            db.prepare("SELECT port_id,pid,started_at,executable_path FROM running_processes")?;
        Ok(statement
            .query_map([], |row| {
                Ok(ProcessIdentity {
                    port_id: row.get(0)?,
                    pid: row.get(1)?,
                    started_at: row.get(2)?,
                    executable: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }
    fn clear_process_record(
        &self,
        identity: &ProcessIdentity,
        notice: Option<&str>,
    ) -> Result<(), DesktopError> {
        clear_process_record(
            &self.root,
            &self.tracked_processes,
            &self.db,
            identity,
            notice,
        )
    }
    fn resume_uninstalls(&self) -> Result<(), DesktopError> {
        let pending = {
            let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
            let mut statement = db.prepare("SELECT operation_id,port_id,part_path,staging_path FROM operations WHERE kind='uninstall' AND phase='uninstalling'")?;
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        for (id, port_id, active, previous) in pending {
            let has_install: bool = {
                let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
                db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM installs WHERE port_id=?1)",
                    params![port_id],
                    |row| row.get(0),
                )?
            };
            if !has_install {
                let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
                db.execute("UPDATE operations SET error='Cannot safely resume uninstall: installed package ownership metadata is missing' WHERE operation_id=?1 AND phase='uninstalling'",params![id])?;
                continue;
            }
            if self.is_running(port_id)? {
                let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
                db.execute("UPDATE operations SET error='Uninstall is deferred because process identity or package ownership cannot be confirmed safe' WHERE operation_id=?1 AND phase='uninstalling'",params![id])?;
                continue;
            }
            for relative_path in [active, previous].into_iter().flatten() {
                let path = owned_version_path(&self.root, port_id, &relative_path)?;
                if path.exists() {
                    fs::remove_dir_all(&path)?;
                }
            }
            let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
            let tx = db.unchecked_transaction()?;
            tx.execute("DELETE FROM installs WHERE port_id=?1", params![port_id])?;
            tx.execute("DELETE FROM updates WHERE port_id=?1", params![port_id])?;
            tx.execute(
                "UPDATE operations SET phase='committed',error=NULL WHERE operation_id=?1",
                params![id],
            )?;
            tx.commit()?;
        }
        Ok(())
    }
    fn begin_operation(&self, port_id: i64) -> Result<PortOperation, DesktopError> {
        let mut operating = self.operating.lock().map_err(|_| DesktopError::Poisoned)?;
        if !operating.insert(port_id) {
            return Err(DesktopError::Busy);
        }
        let pending = {
            let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
            db.query_row("SELECT EXISTS(SELECT 1 FROM operations WHERE port_id=?1 AND phase NOT IN ('committed','failed','cancelled'))", params![port_id], |row| row.get::<_,bool>(0))?
        };
        if pending || self.is_running(port_id)? {
            operating.remove(&port_id);
            return Err(DesktopError::Busy);
        }
        Ok(PortOperation {
            port_id,
            operating: Arc::clone(&self.operating),
        })
    }
    fn emit(
        &self,
        port_id: i64,
        stage: &str,
        received: Option<u64>,
        total: Option<u64>,
        cancellable: bool,
    ) {
        self.progress.send_replace(OperationProgress {
            port_id: Some(port_id),
            queued_port_ids: self
                .queue
                .lock()
                .map(|queue| queue.clone())
                .unwrap_or_default(),
            stage: stage.into(),
            received_bytes: received.unwrap_or_default(),
            total_bytes: total,
            message: None,
            cancellable,
            ..OperationProgress::default()
        });
    }

    fn emit_for(
        &self,
        operation_id: &str,
        port_id: i64,
        stage: &str,
        received: Option<u64>,
        total: Option<u64>,
        cancellable: bool,
    ) {
        self.progress.send_replace(OperationProgress {
            operation_id: Some(operation_id.into()),
            port_id: Some(port_id),
            queued_port_ids: self
                .queue
                .lock()
                .map(|queue| queue.clone())
                .unwrap_or_default(),
            stage: stage.into(),
            received_bytes: received.unwrap_or_default(),
            total_bytes: total,
            message: None,
            cancellable,
        });
    }

    fn discover_installed_processes(&self) -> Result<(), DesktopError> {
        let (packages, uninstalling) = {
            let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
            let mut statement = db.prepare("SELECT port_id,manifest_json,active_path,previous_manifest_json,previous_path FROM installs")?;
            let packages = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let mut statement = db.prepare(
                "SELECT port_id FROM operations WHERE kind='uninstall' AND phase='uninstalling'",
            )?;
            let uninstalling = statement
                .query_map([], |row| row.get::<_, i64>(0))?
                .collect::<Result<HashSet<_>, _>>()?;
            (packages, uninstalling)
        };
        let mut system = sysinfo::System::new_all();
        system.refresh_all();
        let existing = self.process_records()?;
        let mut discovered: HashMap<i64, HashSet<(u32, PathBuf)>> = HashMap::new();
        let defer_uninstall = |port_id: i64| -> Result<(), DesktopError> {
            if uninstalling.contains(&port_id)
                && existing.iter().any(|record| record.port_id == port_id)
            {
                self.tracked_processes
                    .lock()
                    .map_err(|_| DesktopError::Poisoned)?
                    .insert(port_id);
            }
            Ok(())
        };
        for (port_id, active_json, active_path, previous_json, previous_path) in packages {
            let mut paths = vec![(active_json, active_path)];
            if let (Some(json), Some(path)) = (previous_json, previous_path) {
                paths.push((json, path));
            }
            for (manifest_json, relative_path) in paths {
                let Ok(manifest) = serde_json::from_str::<InstallManifest>(&manifest_json) else {
                    defer_uninstall(port_id)?;
                    continue;
                };
                if manifest.validate().is_err() || manifest.port_id != port_id {
                    defer_uninstall(port_id)?;
                    continue;
                }
                let Ok(package) = owned_version_path(&self.root, port_id, &relative_path) else {
                    defer_uninstall(port_id)?;
                    continue;
                };
                let Ok(host) = detect_host_platform() else {
                    defer_uninstall(port_id)?;
                    continue;
                };
                let stored_manifest = read_manifest_file(&package);
                if validate_install_directory(&package, &manifest, &host).is_err()
                    || !stored_manifest
                        .as_ref()
                        .is_ok_and(|stored| stored == &manifest)
                {
                    defer_uninstall(port_id)?;
                    continue;
                }
                let Ok(executable) = package
                    .join(&manifest.recipe.launch.executable)
                    .canonicalize()
                else {
                    defer_uninstall(port_id)?;
                    continue;
                };
                if !executable.starts_with(package.canonicalize()?) {
                    defer_uninstall(port_id)?;
                    continue;
                }
                for (pid, process) in system.processes() {
                    if process
                        .exe()
                        .and_then(|image| image.canonicalize().ok())
                        .as_ref()
                        == Some(&executable)
                    {
                        discovered
                            .entry(port_id)
                            .or_default()
                            .insert((pid.as_u32(), executable.clone()));
                    }
                }
            }
        }
        for (port_id, matches) in discovered {
            if matches.len() != 1 {
                mark_process_identity_uncertain(
                    &self.db,
                    &self.tracked_processes,
                    port_id,
                    "Multiple exact-path game processes prevent safe identity recovery",
                )?;
                continue;
            }
            let (pid, executable) = matches.into_iter().next().expect("one discovered process");
            let started_at = match platform::process_start_time(pid, &executable) {
                Ok(value) => value,
                Err(_) => {
                    mark_process_identity_uncertain(
                        &self.db,
                        &self.tracked_processes,
                        port_id,
                        "Exact-path game process identity cannot be confirmed",
                    )?;
                    continue;
                }
            };
            if existing.iter().any(|record| {
                record.port_id == port_id
                    && record.pid == pid as i64
                    && record.started_at == started_at as i64
                    && record.executable.as_str() == executable.to_string_lossy().as_ref()
            }) {
                continue;
            }
            let operation_id: String = {
                let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
                db.query_row("SELECT COALESCE((SELECT operation_id FROM operations WHERE port_id=?1 AND kind='launch' AND phase='launching' LIMIT 1),'')",
                    params![port_id],|row| row.get(0))?
            };
            self.persist_running_process(port_id, pid, started_at, &executable, &operation_id)?;
        }
        Ok(())
    }
    fn reconcile(&self) -> Result<(), DesktopError> {
        self.discover_installed_processes()?;
        self.reap_children()?;
        self.resume_uninstalls()?;
        let pending = {
            let db = self.db.lock().map_err(|_| DesktopError::Poisoned)?;
            let mut statement = db.prepare("SELECT operation_id,port_id,kind,phase,part_path,staging_path,final_path FROM operations WHERE phase NOT IN ('committed','failed','cancelled')")?;
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        for (id, port_id, kind, phase, part, staging, final_path) in pending {
            if kind == "launch" {
                if phase == "launching" {
                    self.set_operation_phase(&id,"launching",Some("Launch outcome is still uncertain; verified process identity was not recovered"))?;
                    self.set_status(port_id,InstallStatus::Failed,Some("Launch outcome is uncertain; destructive actions are blocked until process identity is confirmed".into()))?;
                } else {
                    self.set_operation_phase(
                        &id,
                        "failed",
                        Some("Launch was interrupted before process spawn"),
                    )?;
                }
                continue;
            }
            if self.is_running(port_id)? {
                continue;
            }
            if kind == "uninstall" {
                continue;
            }
            for relative_path in [part, staging].into_iter().flatten() {
                let path = managed_path(&self.root, &relative_path)?;
                if path.is_dir() {
                    fs::remove_dir_all(path)?;
                } else if path.exists() {
                    fs::remove_file(path)?;
                }
            }
            if phase == "renamed"
                && let Some(relative_path) = final_path
            {
                let path = owned_version_path(&self.root, port_id, &relative_path)?;
                if path.exists() {
                    fs::remove_dir_all(path)?;
                }
            }
            self.set_operation_phase(
                &id,
                "failed",
                Some("Interrupted operation was safely rolled back during startup"),
            )?;
        }
        Ok(())
    }
}

impl Drop for LibraryManager {
    fn drop(&mut self) {
        self.process_monitor_stop.store(true, Ordering::Release);
        if let Some(monitor) = self.process_monitor.take() {
            let _ = monitor.join();
        }
    }
}
struct PortOperation {
    port_id: i64,
    operating: Arc<Mutex<HashSet<i64>>>,
}
impl Drop for PortOperation {
    fn drop(&mut self) {
        if let Ok(mut operating) = self.operating.lock() {
            operating.remove(&self.port_id);
        }
    }
}
struct DownloadPermit<'a> {
    active: &'a Mutex<bool>,
}

impl Drop for DownloadPermit<'_> {
    fn drop(&mut self) {
        if let Ok(mut active) = self.active.lock() {
            *active = false;
        }
    }
}

fn check_cancelled(cancellations: &Mutex<HashSet<i64>>, port_id: i64) -> Result<(), DesktopError> {
    if cancellations
        .lock()
        .map_err(|_| DesktopError::Poisoned)?
        .contains(&port_id)
    {
        Err(DesktopError::Cancelled)
    } else {
        Ok(())
    }
}

fn check_extraction_cancelled(
    cancellation: Option<(&Mutex<HashSet<i64>>, i64)>,
) -> Result<(), DesktopError> {
    if let Some((cancellations, port_id)) = cancellation {
        check_cancelled(cancellations, port_id)
    } else {
        Ok(())
    }
}

fn managed_path(root: &Path, relative: &str) -> Result<PathBuf, DesktopError> {
    let rel = Path::new(relative);
    if rel.as_os_str().is_empty()
        || rel.is_absolute()
        || rel
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err(DesktopError::Package(
            "stored managed path is invalid".into(),
        ));
    }
    let path = root.join(rel);
    let mut cursor = root.to_path_buf();
    for component in rel.components() {
        let std::path::Component::Normal(name) = component else {
            unreachable!()
        };
        cursor.push(name);
        match fs::symlink_metadata(&cursor) {
            Ok(metadata) if is_link_or_reparse(&metadata) => {
                return Err(DesktopError::Package(
                    "stored managed path contains a symlink or reparse point".into(),
                ));
            }
            Ok(_) => {
                if !cursor.canonicalize()?.starts_with(root) {
                    return Err(DesktopError::Package(
                        "stored path resolves outside the managed library".into(),
                    ));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(path)
}

#[cfg(unix)]
pub(crate) fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}
#[cfg(windows)]
pub(crate) fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
}
#[cfg(not(any(unix, windows)))]
pub(crate) fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn sync_tree(root: &Path) -> Result<(), DesktopError> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            sync_tree(&entry.path())?;
        } else if kind.is_file() {
            // Windows FlushFileBuffers requires a handle opened with write access.
            #[cfg(windows)]
            let file = File::options().write(true).open(entry.path())?;
            #[cfg(not(windows))]
            let file = File::open(entry.path())?;
            file.sync_all()?;
        }
    }
    sync_parent(root)
}
pub(crate) fn owned_version_path(
    root: &Path,
    port_id: i64,
    relative: &str,
) -> Result<PathBuf, DesktopError> {
    let path = managed_path(root, relative)?;
    let versions = root
        .join("games")
        .join(port_id.to_string())
        .join("versions");
    let suffix = path.strip_prefix(&versions).map_err(|_| {
        DesktopError::Package("stored package path is not an owned version directory".into())
    })?;
    if suffix.components().count() != 1
        || suffix
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err(DesktopError::Package(
            "stored package path is not an owned version directory".into(),
        ));
    }
    if path.exists() {
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_dir() || is_link_or_reparse(&metadata) {
            return Err(DesktopError::Package(
                "owned package path is not a regular directory".into(),
            ));
        }
        let canonical_versions = versions.canonicalize()?;
        let canonical_path = path.canonicalize()?;
        if canonical_path.parent() != Some(canonical_versions.as_path()) {
            return Err(DesktopError::Package(
                "stored package path escapes its owned versions directory".into(),
            ));
        }
    }
    Ok(path)
}
fn ensure_versions_directory(root: &Path, port_id: i64) -> Result<PathBuf, DesktopError> {
    let games = root.join("games");
    let game = games.join(port_id.to_string());
    let versions = game.join("versions");
    for directory in [&games, &game, &versions] {
        match fs::symlink_metadata(directory) {
            Ok(metadata) if is_link_or_reparse(&metadata) || !metadata.is_dir() => {
                return Err(DesktopError::Package(
                    "managed versions path contains a symlink, reparse point, or non-directory"
                        .into(),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(directory)?,
            Err(error) => return Err(error.into()),
        }
        if !directory.canonicalize()?.starts_with(root) {
            return Err(DesktopError::Package(
                "managed versions directory escapes the library root".into(),
            ));
        }
    }
    let canonical_game = game.canonicalize()?;
    if canonical_game.parent() != Some(games.canonicalize()?.as_path()) {
        return Err(DesktopError::Package(
            "managed game directory is not owned by the expected port".into(),
        ));
    }
    Ok(versions)
}
fn ensure_root_directory(root: &Path, name: &str) -> Result<(), DesktopError> {
    let path = root.join(name);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if is_link_or_reparse(&metadata) || !metadata.is_dir() => {
            return Err(DesktopError::Package(format!(
                "managed {name} directory is a symlink, reparse point, or non-directory"
            )));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(&path)?,
        Err(error) => return Err(error.into()),
    }
    if !path.canonicalize()?.starts_with(root) {
        return Err(DesktopError::Package(format!(
            "managed {name} directory escapes the library root"
        )));
    }
    Ok(())
}

fn sync_parent(path: &Path) -> Result<(), DesktopError> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(windows)]
    {
        let _ = path;
    }
    Ok(())
}
#[derive(Clone, PartialEq, Eq)]
struct ProcessIdentity {
    port_id: i64,
    pid: i64,
    started_at: i64,
    executable: String,
}
struct RunningProcess {
    child: platform::TrackedChild,
    identity: ProcessIdentity,
}

fn recover_pending_launches(
    root: &Path,
    tracked: &Mutex<HashSet<i64>>,
    db: &Mutex<Connection>,
) -> Result<(), DesktopError> {
    let pending = {
        let database = db.lock().map_err(|_| DesktopError::Poisoned)?;
        let mut statement = database.prepare(
            "SELECT operation_id,port_id FROM operations WHERE kind='launch' AND phase='launching'",
        )?;
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    for (operation_id, port_id) in pending {
        match recover_launch_operation(root, tracked, db, &operation_id, port_id) {
            Ok(_) => {}
            Err(error) => {
                let database = db.lock().map_err(|_| DesktopError::Poisoned)?;
                database.execute(
                    "UPDATE operations SET error=?1 WHERE operation_id=?2 AND port_id=?3 AND phase='launching'",
                    params![format!("Launch outcome is uncertain: {error}"),operation_id,port_id],
                )?;
                database.execute(
                    "UPDATE installs SET status='failed',error=?1 WHERE port_id=?2",
                    params![format!("Launch outcome is uncertain: {error}"), port_id],
                )?;
            }
        }
    }
    Ok(())
}

fn mark_process_identity_uncertain(
    db: &Mutex<Connection>,
    tracked: &Mutex<HashSet<i64>>,
    port_id: i64,
    message: &str,
) -> Result<(), DesktopError> {
    let database = db.lock().map_err(|_| DesktopError::Poisoned)?;
    let tx = database.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM running_processes WHERE port_id=?1",
        params![port_id],
    )?;
    tx.execute(
        "UPDATE installs SET status='failed',error=?1 WHERE port_id=?2",
        params![message, port_id],
    )?;
    {
        let mut tracked = tracked.lock().map_err(|_| DesktopError::Poisoned)?;
        tracked.insert(port_id);
    }
    tx.commit()?;
    Ok(())
}

fn recover_launch_operation(
    root: &Path,
    tracked: &Mutex<HashSet<i64>>,
    db: &Mutex<Connection>,
    operation_id: &str,
    port_id: i64,
) -> Result<bool, DesktopError> {
    let record = {
        let database = db.lock().map_err(|_| DesktopError::Poisoned)?;
        database.query_row(
            "SELECT o.candidate_offer_json,o.final_path,i.manifest_json,i.active_path FROM operations o JOIN installs i ON i.port_id=o.port_id WHERE o.operation_id=?1 AND o.port_id=?2 AND o.kind='launch' AND o.phase='launching'",
            params![operation_id,port_id],
            |row| Ok((row.get::<_,Option<String>>(0)?,row.get::<_,Option<String>>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?)),
        ).optional()?
    };
    let Some((Some(json), Some(final_path), installed_json, active_path)) = record else {
        return Ok(false);
    };
    if active_path != final_path {
        return Ok(false);
    }
    let offer: InstallOffer = serde_json::from_str(&json).map_err(json_error)?;
    offer
        .validate()
        .map_err(|error| DesktopError::Offer(error.to_string()))?;
    let installed: InstallManifest = serde_json::from_str(&installed_json).map_err(json_error)?;
    installed
        .validate()
        .map_err(|error| DesktopError::Offer(error.to_string()))?;
    if offer.manifest.port_id != port_id || offer.manifest != installed {
        return Ok(false);
    }
    let package = owned_version_path(root, port_id, &final_path)?;
    if read_manifest_file(&package)? != installed {
        return Ok(false);
    }
    let host = detect_host_platform()?;
    validate_install_directory(&package, &installed, &host)?;
    let executable = package
        .join(&installed.recipe.launch.executable)
        .canonicalize()?;
    if !executable.starts_with(package.canonicalize()?) {
        return Ok(false);
    }
    let mut system = sysinfo::System::new_all();
    system.refresh_all();
    let matches = system
        .processes()
        .iter()
        .filter_map(|(pid, process)| {
            process.exe().and_then(|path| {
                path.canonicalize()
                    .ok()
                    .filter(|path| path == &executable)
                    .map(|_| pid.as_u32())
            })
        })
        .collect::<Vec<_>>();
    if matches.len() > 1 {
        let database = db.lock().map_err(|_| DesktopError::Poisoned)?;
        let message = "Multiple exact-path game processes prevent safe launch identity recovery";
        database.execute(
            "UPDATE operations SET error=?1 WHERE operation_id=?2 AND port_id=?3 AND phase='launching'",
            params![message,operation_id,port_id],
        )?;
        database.execute(
            "UPDATE installs SET status='failed',error=?1 WHERE port_id=?2",
            params![message, port_id],
        )?;
        database.execute(
            "DELETE FROM running_processes WHERE port_id=?1",
            params![port_id],
        )?;
        tracked
            .lock()
            .map_err(|_| DesktopError::Poisoned)?
            .insert(port_id);
        return Ok(false);
    }
    let Some(pid) = matches.first().copied() else {
        return Ok(false);
    };
    let started_at = platform::process_start_time(pid, &executable)?;
    let database = db.lock().map_err(|_| DesktopError::Poisoned)?;
    let tx = database.unchecked_transaction()?;
    let claimed = tx.execute(
        "UPDATE operations SET phase='committed',error=NULL WHERE operation_id=?1 AND port_id=?2 AND kind='launch' AND phase='launching'",
        params![operation_id,port_id],
    )?;
    if claimed == 0 {
        let already_recovered: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM operations o WHERE o.operation_id=?1 AND o.port_id=?2 AND o.kind='launch' AND o.phase='committed')",
            params![operation_id,port_id],|row| row.get(0),
        )?;
        tx.commit()?;
        return Ok(already_recovered);
    }
    tx.execute(
        "INSERT OR REPLACE INTO running_processes(port_id,pid,started_at,executable_path) VALUES(?1,?2,?3,?4)",
        params![port_id,pid as i64,started_at as i64,executable.to_string_lossy()],
    )?;
    tx.execute(
        "UPDATE installs SET status='running',error=NULL WHERE port_id=?1",
        params![port_id],
    )?;
    tx.commit()?;
    tracked
        .lock()
        .map_err(|_| DesktopError::Poisoned)?
        .insert(port_id);
    Ok(true)
}
fn monitor_processes(
    root: &Path,
    running: &Mutex<HashMap<i64, RunningProcess>>,
    tracked: &Mutex<HashSet<i64>>,
    db: &Mutex<Connection>,
) -> Result<(), DesktopError> {
    let records = {
        let database = db.lock().map_err(|_| DesktopError::Poisoned)?;
        let mut statement = database
            .prepare("SELECT port_id,pid,started_at,executable_path FROM running_processes")?;
        statement
            .query_map([], |row| {
                Ok(ProcessIdentity {
                    port_id: row.get(0)?,
                    pid: row.get(1)?,
                    started_at: row.get(2)?,
                    executable: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    let completed = {
        let mut children = running.lock().map_err(|_| DesktopError::Poisoned)?;
        let mut completed = Vec::new();
        for process in children.values_mut() {
            if let Some(status) = process.child.try_wait()? {
                completed.push((process.identity.clone(), status.code()));
            }
        }
        for (identity, _) in &completed {
            if children
                .get(&identity.port_id)
                .is_some_and(|process| process.identity == *identity)
            {
                children.remove(&identity.port_id);
            }
        }
        completed
    };
    for (identity, code) in completed {
        if records.iter().any(|record| record == &identity) {
            let notice = code
                .filter(|code| *code != 0)
                .map(|code| format!("Game exited with code {code}"));
            clear_process_record(root, tracked, db, &identity, notice.as_deref())?;
        }
    }
    for identity in records {
        let pid = u32::try_from(identity.pid)
            .map_err(|_| DesktopError::Package("stored process ID is invalid".into()))?;
        if platform::process_identity_matches(
            pid,
            identity.started_at as u64,
            Path::new(&identity.executable),
        )? {
            tracked
                .lock()
                .map_err(|_| DesktopError::Poisoned)?
                .insert(identity.port_id);
        } else {
            clear_process_record(
                root,
                tracked,
                db,
                &identity,
                Some("Game exited; exit code unavailable after manager restart"),
            )?;
        }
    }
    Ok(())
}

fn clear_process_record(
    root: &Path,
    tracked: &Mutex<HashSet<i64>>,
    db: &Mutex<Connection>,
    identity: &ProcessIdentity,
    notice: Option<&str>,
) -> Result<(), DesktopError> {
    let database = db.lock().map_err(|_| DesktopError::Poisoned)?;
    let tx = database.unchecked_transaction()?;
    let deleted = tx.execute(
        "DELETE FROM running_processes WHERE port_id=?1 AND pid=?2 AND started_at=?3 AND executable_path=?4",
        params![identity.port_id,identity.pid,identity.started_at,identity.executable],
    )?;
    if deleted == 0 {
        tx.commit()?;
        return Ok(());
    }
    let installed: Option<(String, String)> = tx
        .query_row(
            "SELECT manifest_json,active_path FROM installs WHERE port_id=?1",
            params![identity.port_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((json, active_path)) = installed {
        let state = serde_json::from_str::<InstallManifest>(&json)
            .ok()
            .filter(|manifest| manifest.validate().is_ok())
            .map(|manifest| {
                let healthy = owned_version_path(root, identity.port_id, &active_path)
                    .ok()
                    .is_some_and(|path| {
                        detect_host_platform().is_ok_and(|host| {
                            validate_install_directory(&path, &manifest, &host).is_ok()
                        })
                    });
                (healthy, is_original_data_ready(&manifest))
            });
        let (status, error) = match state {
            None => ("repair_needed", Some("Metadata damaged".to_owned())),
            Some((false, _)) => (
                "repair_needed",
                Some(
                    "Repair needed: configured executable or working directory is unavailable"
                        .to_owned(),
                ),
            ),
            Some((true, true)) => ("ready", notice.map(str::to_owned)),
            Some((true, false)) => ("installed_needs_data", notice.map(str::to_owned)),
        };
        tx.execute(
            "UPDATE installs SET status=?1,error=?2 WHERE port_id=?3",
            params![status, error, identity.port_id],
        )?;
    }
    let mut membership = tracked.lock().map_err(|_| DesktopError::Poisoned)?;
    tx.commit()?;
    membership.remove(&identity.port_id);
    Ok(())
}
fn is_original_data_ready(manifest: &InstallManifest) -> bool {
    match manifest.recipe.data_adapter.as_deref() {
        None => true,
        Some("banjo_n64_us_10") => banjo_data_directory()
            .map(|directory| {
                matches!(
                    data::stored_banjo_status(&directory.join(BANJO_ROM_NAME)),
                    Ok(DataStatus::Ready)
                )
            })
            .unwrap_or(false),
        Some(_) => false,
    }
}

fn check_runtime_requirements(
    host: &recomphub_shared::HostPlatform,
    manifest: &InstallManifest,
) -> Result<(), DesktopError> {
    if host.arch == CpuArch::X86_64 && manifest.recipe.id.starts_with("banjo-recompiled-") {
        #[cfg(target_arch = "x86_64")]
        if !std::is_x86_feature_detected!("sse4.1") {
            return Err(DesktopError::UnsupportedHost(
                "Banjo Recompiled requires SSE4.1 on x86_64".into(),
            ));
        }
        #[cfg(not(target_arch = "x86_64"))]
        return Err(DesktopError::UnsupportedHost(
            "this process cannot verify the required SSE4.1 instruction set".into(),
        ));
    }
    Ok(())
}
struct InstallRow {
    port_id: i64,
    game_id: i64,
    game_name: String,
    game_slug: String,
    port_name: String,
    port_slug: String,
    manifest_json: String,
    active_path: String,
    previous_path: Option<String>,
    status: String,
    error: Option<String>,
    offer_json: String,
    last_checked_at: Option<String>,
}

fn install_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<InstallRow> {
    Ok(InstallRow {
        port_id: row.get(0)?,
        game_id: row.get(1)?,
        game_name: row.get(2)?,
        game_slug: row.get(3)?,
        port_name: row.get(4)?,
        port_slug: row.get(5)?,
        manifest_json: row.get(6)?,
        active_path: row.get(7)?,
        previous_path: row.get(8)?,
        status: row.get(9)?,
        error: row.get(10)?,
        offer_json: row.get(11)?,
        last_checked_at: row.get(12)?,
    })
}

fn read_favorite_ids(db: &Connection) -> Result<Vec<i64>, DesktopError> {
    let mut statement =
        db.prepare("SELECT game_id FROM device_favorites ORDER BY position, game_id LIMIT ?1")?;
    let rows = statement.query_map(params![DEVICE_FAVORITE_CAP as i64 + 1], |row| row.get(0))?;
    let ids = rows.collect::<Result<Vec<i64>, _>>()?;
    if ids.len() > DEVICE_FAVORITE_CAP {
        return Err(DesktopError::Package(
            "device saved games exceed the supported 2,000-game limit".into(),
        ));
    }
    Ok(ids)
}

fn initialize_database(db: &Connection) -> Result<(), DesktopError> {
    db.execute_batch("PRAGMA journal_mode = WAL;")?;
    store::initialize(db)
}

fn download_client() -> Result<Client, DesktopError> {
    Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(600))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let url = attempt.url();
            let valid_authority = url.scheme() == "https"
                && url.username().is_empty()
                && url.password().is_none()
                && url.port_or_known_default() == Some(443);
            let initial = attempt.previous().first();
            let initial_repo = initial.and_then(|url| {
                let mut segments = url.path_segments()?;
                Some((segments.next()?.to_owned(), segments.next()?.to_owned()))
            });
            let segments = url
                .path_segments()
                .map(|segments| segments.map(str::to_owned).collect::<Vec<_>>())
                .unwrap_or_default();
            let allowed = valid_authority
                && match url.host_str().unwrap_or_default() {
                    "github.com" => initial_repo.as_ref().is_some_and(|(owner, repo)| {
                        segments.len() == 6
                            && segments[0].eq_ignore_ascii_case(owner)
                            && segments[1].eq_ignore_ascii_case(repo)
                            && segments[2] == "releases"
                            && segments[3] == "download"
                    }),
                    "release-assets.githubusercontent.com" => true,
                    "objects.githubusercontent.com" => url.path().starts_with("/objects/"),
                    _ => false,
                };
            if !allowed || attempt.previous().len() >= 5 {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(DesktopError::Http)
}

fn validate_download_url(download: &str, repo: &str) -> Result<(), DesktopError> {
    validate_github_release_download_url(repo, download)
        .map_err(|error| DesktopError::Package(error.to_string()))
}

fn extract_package(
    package: &Path,
    destination: &Path,
    manifest: &InstallManifest,
    cancellation: Option<(&Mutex<HashSet<i64>>, i64)>,
) -> Result<(), DesktopError> {
    let mut archive = zip::ZipArchive::new(File::open(package)?)?;
    if archive.len() > ENTRY_CAP {
        return Err(DesktopError::Package("archive has too many entries".into()));
    }
    #[derive(Clone)]
    struct EntryInfo {
        path: PathBuf,
        directory: bool,
        #[cfg(unix)]
        mode: u32,
        size: u64,
        symlink: bool,
    }
    let mut entries = Vec::with_capacity(archive.len());
    let mut kinds = HashMap::new();
    let mut total = 0_u64;
    let mut links = HashMap::new();
    check_extraction_cancelled(cancellation)?;
    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        check_extraction_cancelled(cancellation)?;
        let mut name = entry.name().to_owned();
        let trailing_slash = name.ends_with('/');
        let mode = entry.unix_mode().unwrap_or(0);
        let kind = mode & 0o170000;
        let symlink = kind == 0o120000;
        if kind != 0 && kind != 0o100000 && kind != 0o040000 && !symlink {
            return Err(DesktopError::Package(
                "archive contains a special file".into(),
            ));
        }
        if trailing_slash && (kind == 0o100000 || symlink) {
            return Err(DesktopError::Package(
                "archive entry metadata contradicts its trailing slash".into(),
            ));
        }
        let directory = kind == 0o040000 || (kind == 0 && (entry.is_dir() || trailing_slash));
        if directory && trailing_slash {
            name.pop();
        }
        let path = safe_zip_path(&name)?;
        // Links are leaves regardless of their name marker or other archive attributes.
        package::insert_archive_entry(&mut kinds, &path, directory && !symlink)?;
        let size = entry.size();
        if symlink && size > 4096 {
            return Err(DesktopError::Package(
                "archive symlink payload is too large".into(),
            ));
        }
        if !directory && !symlink {
            total = total
                .checked_add(size)
                .ok_or_else(|| DesktopError::Package("archive is too large".into()))?;
            if total > EXTRACT_CAP {
                return Err(DesktopError::Package(
                    "archive exceeds decompression limit".into(),
                ));
            }
        }
        if symlink {
            let mut target = String::new();
            entry.take(4097).read_to_string(&mut target)?;
            links.insert(path.clone(), target);
        }
        entries.push(EntryInfo {
            path,
            directory,
            #[cfg(unix)]
            mode,
            size,
            symlink,
        });
    }
    if entries.iter().any(|entry| entry.symlink) && manifest.recipe.os != HostOs::Macos {
        return Err(DesktopError::Package(
            "archive symlinks are not permitted for this platform".into(),
        ));
    }
    let regular_files = entries
        .iter()
        .filter(|entry| !entry.directory && !entry.symlink)
        .map(|entry| entry.path.clone())
        .collect();
    package::validate_archive_links(&links, &regular_files, &kinds)?;
    if destination.exists() {
        return Err(DesktopError::Package(
            "staging directory already exists".into(),
        ));
    }
    fs::create_dir_all(destination)?;
    for (index, info) in entries.iter().enumerate() {
        check_extraction_cancelled(cancellation)?;
        let mut entry = archive.by_index(index)?;
        let output = destination.join(&info.path);
        if info.symlink {
            continue;
        }
        if info.directory {
            fs::create_dir_all(&output)?;
            continue;
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)?;
        let mut copied = 0_u64;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            check_extraction_cancelled(cancellation)?;
            let count = entry.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            copied = copied
                .checked_add(count as u64)
                .ok_or_else(|| DesktopError::Package("archive is too large".into()))?;
            if copied > info.size || copied > EXTRACT_CAP {
                return Err(DesktopError::Package(
                    "archive entry exceeds its declared size".into(),
                ));
            }
            file.write_all(&buffer[..count])?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let safe_mode = if info.mode == 0 {
                if info.path == Path::new(&manifest.recipe.launch.executable) {
                    0o755
                } else {
                    0o644
                }
            } else {
                info.mode & 0o777 & !0o6000
            };
            fs::set_permissions(&output, fs::Permissions::from_mode(safe_mode))?;
        }
        file.sync_all()?;
    }
    #[cfg(unix)]
    for (relative_link, target) in links {
        check_extraction_cancelled(cancellation)?;
        let link = destination.join(&relative_link);
        if let Some(parent) = link.parent() {
            fs::create_dir_all(parent)?;
        }
        std::os::unix::fs::symlink(Path::new(&target), link)?;
    }
    #[cfg(not(unix))]
    if !links.is_empty() {
        return Err(DesktopError::Package(
            "archive symlinks are unsupported on this host".into(),
        ));
    }
    Ok(())
}

fn safe_zip_path(value: &str) -> Result<PathBuf, DesktopError> {
    package::safe_zip_path(value)
}

fn configured_launch_paths(
    directory: &Path,
    manifest: &InstallManifest,
) -> Result<(PathBuf, PathBuf), DesktopError> {
    let root = directory.canonicalize()?;
    let executable = checked_package_path(&root, &manifest.recipe.launch.executable)?;
    let working = checked_package_path(&root, &manifest.recipe.launch.working_directory)?;
    let exe_metadata = fs::symlink_metadata(&executable)?;
    if !exe_metadata.file_type().is_file() {
        return Err(DesktopError::Package(
            "configured executable is not a regular file".into(),
        ));
    }
    if !working.is_dir() {
        return Err(DesktopError::Package(
            "configured working directory is not a directory".into(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if exe_metadata.permissions().mode() & 0o111 == 0 {
            return Err(DesktopError::Package(
                "configured executable is not executable".into(),
            ));
        }
    }
    Ok((executable, working))
}

fn checked_package_path(root: &Path, relative: &str) -> Result<PathBuf, DesktopError> {
    if relative == "." {
        return Ok(root.to_path_buf());
    }
    let rel = Path::new(relative);
    if rel.is_absolute()
        || rel
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err(DesktopError::Package(
            "configured launch path is not package-relative".into(),
        ));
    }
    let mut path = root.to_path_buf();
    for component in rel.components() {
        let std::path::Component::Normal(name) = component else {
            unreachable!()
        };
        path.push(name);
        let metadata = fs::symlink_metadata(&path)?;
        if is_link_or_reparse(&metadata) {
            return Err(DesktopError::Package(
                "configured launch path contains a symlink or reparse point".into(),
            ));
        }
        if !path.canonicalize()?.starts_with(root) {
            return Err(DesktopError::Package(
                "configured launch path escapes the package".into(),
            ));
        }
    }
    Ok(path)
}
pub(crate) fn validate_install_directory(
    directory: &Path,
    manifest: &InstallManifest,
    host: &HostPlatform,
) -> Result<(), DesktopError> {
    if !manifest
        .matches_host(host)
        .map_err(|error| DesktopError::Offer(error.to_string()))?
    {
        return Err(DesktopError::UnsupportedHost(
            "the installed build does not match this machine".into(),
        ));
    }
    let (executable, working) = configured_launch_paths(directory, manifest)?;
    let root = directory.canonicalize()?;
    let executable = executable.canonicalize()?;
    let working = working.canonicalize()?;
    if !executable.starts_with(&root) || !working.starts_with(&root) {
        return Err(DesktopError::Package(
            "executable or working directory escapes the package".into(),
        ));
    }
    let file = File::open(&executable)?;
    // SAFETY: read-only mapping of a regular file, never mutated through this mapping.
    let mapped = unsafe { memmap2::MmapOptions::new().map(&file) }
        .map_err(|error| DesktopError::Package(format!("could not map executable: {error}")))?;

    match manifest.recipe.os {
        HostOs::Macos => {
            let mach = goblin::mach::Mach::parse(&mapped).map_err(|error| {
                DesktopError::Package(format!("invalid Mach-O executable: {error}"))
            })?;
            let has_arch = match mach {
                goblin::mach::Mach::Binary(binary) => {
                    recipe_accepts_mach_arch(manifest, binary.header.cputype, host.arch)
                }
                goblin::mach::Mach::Fat(fat) => {
                    let mut found = false;
                    for arch in fat.iter_arches() {
                        let arch = arch.map_err(|error| {
                            DesktopError::Package(format!("invalid fat Mach-O: {error}"))
                        })?;
                        found |= recipe_accepts_mach_arch(manifest, arch.cputype, host.arch);
                    }
                    found
                }
            };
            if !has_arch {
                return Err(DesktopError::Package(
                    "Mach-O has no slice for the detected host architecture".into(),
                ));
            }
            Ok(())
        }
        HostOs::Windows => {
            match goblin::Object::parse(&mapped)
                .map_err(|error| DesktopError::Package(format!("invalid PE executable: {error}")))?
            {
                goblin::Object::PE(pe) => {
                    if !pe_arch_matches_host(pe.header.coff_header.machine, manifest, host.arch) {
                        return Err(DesktopError::Package(
                            "PE architecture does not match the detected host machine".into(),
                        ));
                    }
                }
                _ => {
                    return Err(DesktopError::Package(
                        "configured Windows executable is not a PE image".into(),
                    ));
                }
            }
            Ok(())
        }
    }
}

fn recipe_accepts_mach_arch(manifest: &InstallManifest, cputype: u32, host_arch: CpuArch) -> bool {
    let arch = match cputype {
        0x0100_0007 => CpuArch::X86_64,
        0x0100_000C => CpuArch::Aarch64,
        _ => return false,
    };
    arch == host_arch && manifest.recipe.architectures.contains(&arch)
}
fn pe_arch_matches_host(machine: u16, manifest: &InstallManifest, host_arch: CpuArch) -> bool {
    let arch = match machine {
        0x8664 => CpuArch::X86_64,
        0xAA64 => CpuArch::Aarch64,
        _ => return false,
    };
    arch == host_arch && manifest.recipe.architectures.contains(&host_arch)
}

fn write_manifest_file(directory: &Path, manifest: &InstallManifest) -> Result<(), DesktopError> {
    atomic_write(
        &directory.join(".recomphub-manifest.json"),
        serde_json::to_string(manifest)
            .map_err(json_error)?
            .as_bytes(),
    )
}
fn read_manifest_file(directory: &Path) -> Result<InstallManifest, DesktopError> {
    serde_json::from_slice(&fs::read(directory.join(".recomphub-manifest.json"))?)
        .map_err(json_error)
}

fn banjo_data_directory() -> Result<PathBuf, DesktopError> {
    directories::BaseDirs::new()
        .map(|dirs| dirs.data_local_dir().join("BanjoRecompiled"))
        .ok_or_else(|| {
            DesktopError::UnsupportedHost("could not resolve the game data folder".into())
        })
}

fn macos_app_bundle(executable: &Path) -> Option<&Path> {
    executable.ancestors().find(|path| {
        path.extension()
            .is_some_and(|extension| extension.as_encoded_bytes().eq_ignore_ascii_case(b"app"))
    })
}

fn reject_portable_marker(
    directory: &Path,
    manifest: &InstallManifest,
) -> Result<(), DesktopError> {
    let working = directory.join(&manifest.recipe.launch.working_directory);
    let executable = directory.join(&manifest.recipe.launch.executable);
    let app_parent = macos_app_bundle(&executable).and_then(Path::parent);
    if working.join("portable.txt").exists()
        || app_parent.is_some_and(|path| path.join("portable.txt").exists())
    {
        return Err(DesktopError::Package(
            "portable.txt prevents the managed data-directory contract".into(),
        ));
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), DesktopError> {
    let temp = path.with_extension(format!("tmp-{}", Uuid::new_v4()));
    {
        let mut file = File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(temp, path)?;
    Ok(())
}

fn relative(root: &Path, path: &Path) -> Result<String, DesktopError> {
    let path = path
        .strip_prefix(root)
        .map_err(|_| DesktopError::Package("path is outside the managed library".into()))?;
    let mut value = String::with_capacity(path.as_os_str().len());
    for component in path.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(DesktopError::Package(
                "managed paths must contain only normal components".into(),
            ));
        };
        let name = name.to_str().ok_or_else(|| {
            DesktopError::Package("managed paths must use valid Unicode names".into())
        })?;
        if !value.is_empty() {
            value.push('/');
        }
        value.push_str(name);
    }
    Ok(value)
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn json_error(error: serde_json::Error) -> DesktopError {
    DesktopError::Offer(format!("invalid local metadata: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample_manifest() -> InstallManifest {
        InstallManifest {
            schema_version: 1,
            recipe: recomphub_shared::InstallRecipe {
                id: "windows".into(),
                revision: 1,
                repo_url: "https://github.com/example/game".into(),
                os: HostOs::Windows,
                architectures: vec![CpuArch::X86_64],
                minimum_os_version: None,
                asset_name_template: "game-{tag}.zip".into(),
                package_format: recomphub_shared::PackageFormat::Zip,
                launch: recomphub_shared::LaunchSpec {
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
                "https://github.com/example/game/releases/download/v1.0.0/game-v1.0.0.zip".into(),
            size_bytes: 1,
            sha256: "00".repeat(32),
        }
    }
    #[test]
    fn device_favorites_persist_and_acknowledge_only_confirmed_ids() {
        let root = tempfile::tempdir().unwrap();
        {
            let library = LibraryManager::open(root.path().to_path_buf()).unwrap();
            assert_eq!(library.set_favorite_game(12, true).unwrap(), [12]);
            assert_eq!(library.set_favorite_game(4, true).unwrap(), [12, 4]);
            assert_eq!(library.set_favorite_game(7, true).unwrap(), [12, 4, 7]);
            assert_eq!(library.set_favorite_game(12, false).unwrap(), [4, 7]);
            assert_eq!(library.acknowledge_favorite_games(&[4]).unwrap(), [7]);
            assert_eq!(library.favorite_game_ids().unwrap(), [7]);
            assert!(library.set_favorite_game(0, true).is_err());
            for id in [100, 90, 80, 70, 60] {
                library.set_favorite_game(id, true).unwrap();
            }
            assert_eq!(
                library.acknowledge_favorite_games(&[7, 100, 90]).unwrap(),
                [80, 70, 60]
            );
            assert_eq!(
                library.set_favorite_game(50, true).unwrap(),
                [80, 70, 60, 50]
            );
        }
        let reopened = LibraryManager::open(root.path().to_path_buf()).unwrap();
        assert_eq!(reopened.favorite_game_ids().unwrap(), [80, 70, 60, 50]);
    }

    #[test]
    fn archive_paths_reject_escapes_and_windows_devices() {
        assert!(safe_zip_path("../escape").is_err());
        assert!(safe_zip_path("C:/escape").is_err());
        assert!(safe_zip_path("NUL").is_err());
        assert_eq!(
            safe_zip_path("BanjoRecompiled.app/Contents/MacOS/BanjoRecompiled").unwrap(),
            PathBuf::from("BanjoRecompiled.app/Contents/MacOS/BanjoRecompiled")
        );
    }

    #[test]
    fn launch_file_type_requires_expected_binary_format() {
        let root = tempfile::tempdir().unwrap();
        let exe = root.path().join("game.exe");
        fs::write(&exe, b"not-a-binary").unwrap();
        let manifest = InstallManifest {
            schema_version: 1,
            recipe: recomphub_shared::InstallRecipe {
                id: "windows".into(),
                revision: 1,
                repo_url: "https://github.com/example/game".into(),
                os: HostOs::Windows,
                architectures: vec![CpuArch::X86_64],
                minimum_os_version: None,
                asset_name_template: "game-{tag}.zip".into(),
                package_format: recomphub_shared::PackageFormat::Zip,
                launch: recomphub_shared::LaunchSpec {
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
                "https://github.com/example/game/releases/download/v1.0.0/game-v1.0.0.zip".into(),
            size_bytes: 1,
            sha256: "00".repeat(32),
        };
        let host = HostPlatform {
            os: HostOs::Windows,
            arch: CpuArch::X86_64,
            os_version: "10.0.0".into(),
        };
        assert!(validate_install_directory(root.path(), &manifest, &host).is_err());
    }

    #[test]
    fn generated_zip_unicode_aliases_are_rejected_before_extraction() {
        let root = tempfile::tempdir().unwrap();
        let package_path = root.path().join("aliases.zip");
        let destination = root.path().join("staging");
        let mut archive = zip::ZipWriter::new(File::create(&package_path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        archive
            .start_file("caf\u{00e9}/first.txt", options)
            .unwrap();
        archive.write_all(b"one").unwrap();
        archive
            .start_file("cafe\u{0301}/second.txt", options)
            .unwrap();
        archive.write_all(b"two").unwrap();
        archive.finish().unwrap();
        let manifest = InstallManifest {
            schema_version: 1,
            recipe: recomphub_shared::InstallRecipe {
                id: "windows".into(),
                revision: 1,
                repo_url: "https://github.com/example/game".into(),
                os: HostOs::Windows,
                architectures: vec![CpuArch::X86_64],
                minimum_os_version: None,
                asset_name_template: "game-{tag}.zip".into(),
                package_format: recomphub_shared::PackageFormat::Zip,
                launch: recomphub_shared::LaunchSpec {
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
            download_url: "https://github.com/example/game/releases/download/v1.0.0/game.zip"
                .into(),
            size_bytes: 1,
            sha256: "00".repeat(32),
        };
        assert!(extract_package(&package_path, &destination, &manifest, None).is_err());
        assert!(!destination.exists());
    }

    #[test]
    fn native_download_validator_rejects_repository_prefix_confusion() {
        assert!(
            validate_download_url(
                "https://github.com/example/game-extra/releases/download/v1.0.0/game.zip",
                "https://github.com/example/game",
            )
            .is_err()
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_shell_execute_child_returns_waitable_process_handle() {
        let root = tempfile::tempdir().unwrap();
        let manager = LibraryManager::open(root.path().join("library")).unwrap();
        let versions = ensure_versions_directory(manager.root(), 1).unwrap();
        let package = versions.join("v1");
        fs::create_dir(&package).unwrap();
        fs::copy(std::env::current_exe().unwrap(), package.join("game.exe")).unwrap();
        let mut manifest = sample_manifest();
        manifest.recipe.launch.arguments = vec!["--list".into()];
        let mut child =
            platform::launch_process_verified(manager.root(), &package, &manifest).unwrap();
        assert!(child.id() > 0);
        assert!(child.wait().unwrap().success());
    }
    #[test]
    fn invalid_installed_path_isolated_from_healthy_snapshot_entries() {
        let root = tempfile::tempdir().unwrap();
        let manager = LibraryManager::open(root.path().join("library")).unwrap();
        let versions = ensure_versions_directory(manager.root(), 1).unwrap();
        fs::create_dir(versions.join("v1")).unwrap();
        let healthy_manifest = sample_manifest();
        let healthy_offer = InstallOffer {
            game: recomphub_shared::GameCard {
                id: 1,
                slug: "a".into(),
                name: "A".into(),
                release_year: None,
                cover: None,
                port_count: 1,
                want_count: 0,
                targets: Vec::new(),
                best_playability: None,
                popularity: 0,
            },
            port: recomphub_shared::PortCard {
                id: 1,
                slug: "a-port".into(),
                game_id: 1,
                game_slug: "a".into(),
                game_name: "A".into(),
                name: "A Port".into(),
                kind: "native".into(),
                kind_label: "Native".into(),
                targets: Vec::new(),
                authors: Vec::new(),
                ai_usage: "unknown".into(),
                ai_tools: Vec::new(),
                ai_evidence: Vec::new(),
                playability: "unknown".into(),
                latest_version: Some("1.0.0".into()),
                latest_release_url: Some(healthy_manifest.release_url.clone()),
                latest_release_at: Some(healthy_manifest.published_at),
                release_state: "released".into(),
                release_state_label: "Released".into(),
                github_stars: 0,
                star_count: 0,
                repo_url: healthy_manifest.recipe.repo_url.clone(),
                cover: None,
                maintenance: None,
            },
            manifest: healthy_manifest.clone(),
            checked_at: Utc::now(),
        };
        healthy_offer.validate().unwrap();
        let good = serde_json::to_string(&healthy_manifest).unwrap();
        let good_offer = serde_json::to_string(&healthy_offer).unwrap();
        let mut damaged_manifest = healthy_manifest;
        damaged_manifest.port_id = 2;
        let damaged = serde_json::to_string(&damaged_manifest).unwrap();
        let db = manager.db.lock().unwrap();
        db.execute(
            "INSERT INTO installs(port_id,game_id,game_name,game_slug,port_name,port_slug,manifest_json,active_path,status,offer_json)
             VALUES(1,1,'A','a','A Port','a-port',?1,'games/1/versions/v1','ready',?2),
                   (2,2,'B','b','B Port','b-port',?3,'games/1/versions/v1','ready','{}')",
            params![good, good_offer, damaged],
        ).unwrap();
        drop(db);
        let games = manager.installed().unwrap();
        assert_eq!(games.len(), 2);
        let healthy = games.iter().find(|game| game.port_id == 1).unwrap();
        assert!(healthy.manifest.is_some());
        let damaged = games.iter().find(|game| game.port_id == 2).unwrap();
        assert!(damaged.manifest.is_none());
        assert_eq!(damaged.status, InstallStatus::RepairNeeded);
        assert_eq!(damaged.error.as_deref(), Some("Metadata damaged"));
    }

    #[test]
    fn accepted_precommit_cancellation_blocks_activation_transition() {
        let root = tempfile::tempdir().unwrap();
        let manager = LibraryManager::open(root.path().join("library")).unwrap();
        manager.db.lock().unwrap().execute(
            "INSERT INTO operations(operation_id,port_id,kind,phase,created_at) VALUES('op',1,'install','extracting','now')",
            [],
        ).unwrap();
        assert!(manager.cancel(1).unwrap());
        assert!(matches!(
            manager.enter_activation("op", 1),
            Err(DesktopError::Cancelled)
        ));
        manager.cancellations.lock().unwrap().remove(&1);
        manager.enter_activation("op", 1).unwrap();
        assert!(!manager.cancel(1).unwrap());
    }

    #[test]
    fn configured_executable_health_detects_missing_launch_file() {
        let root = tempfile::tempdir().unwrap();
        let mut manifest = sample_manifest();
        let executable = root.path().join("game.exe");
        fs::write(&executable, b"present executable placeholder").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert!(configured_launch_paths(root.path(), &manifest).is_ok());
        manifest.recipe.launch.working_directory = "missing-working-directory".into();
        assert!(configured_launch_paths(root.path(), &manifest).is_err());
        manifest.recipe.launch.working_directory = ".".into();
        fs::remove_file(executable).unwrap();
        assert!(configured_launch_paths(root.path(), &manifest).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn owned_package_path_rejects_symlinked_parent() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("games/2");
        fs::create_dir_all(source.join("versions")).unwrap();
        fs::create_dir_all(root.path().join("games")).unwrap();
        symlink(&source, root.path().join("games/1")).unwrap();
        assert!(owned_version_path(root.path(), 1, "games/1/versions/v1").is_err());
    }
    #[test]
    fn universal_recipe_binary_checks_require_the_detected_architecture() {
        let mut manifest = sample_manifest();
        manifest.recipe.architectures.push(CpuArch::Aarch64);
        assert!(recipe_accepts_mach_arch(
            &manifest,
            0x0100_0007,
            CpuArch::X86_64
        ));
        assert!(!recipe_accepts_mach_arch(
            &manifest,
            0x0100_000C,
            CpuArch::X86_64
        ));
        assert!(pe_arch_matches_host(0x8664, &manifest, CpuArch::X86_64));
        assert!(!pe_arch_matches_host(0xAA64, &manifest, CpuArch::X86_64));
    }

    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn dropping_manager_does_not_terminate_tracked_game_process() {
        if std::env::args().any(|arg| arg == "child-survival-probe") {
            std::thread::sleep(Duration::from_secs(4));
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let manager = LibraryManager::open(root.path().join("library")).unwrap();
        #[cfg(windows)]
        let (child, executable) = {
            let versions = ensure_versions_directory(manager.root(), 1).unwrap();
            let package = versions.join("v1");
            fs::create_dir(&package).unwrap();
            let executable = package.join("game.exe");
            fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
            let mut manifest = sample_manifest();
            manifest.recipe.launch.arguments = vec![
                "--exact".into(),
                "tests::dropping_manager_does_not_terminate_tracked_game_process".into(),
                "--nocapture".into(),
                "--".into(),
                "child-survival-probe".into(),
            ];
            let child =
                platform::launch_process_verified(manager.root(), &package, &manifest).unwrap();
            (child, executable.canonicalize().unwrap())
        };
        #[cfg(target_os = "macos")]
        let (child, executable) = {
            let executable = std::env::current_exe().unwrap();
            let cwd = std::env::current_dir().unwrap();
            let child = platform::launch_process(
                &executable,
                &cwd,
                &[
                    "--exact".into(),
                    "tests::dropping_manager_does_not_terminate_tracked_game_process".into(),
                    "--nocapture".into(),
                    "--".into(),
                    "child-survival-probe".into(),
                ],
            )
            .unwrap();
            (child, executable)
        };
        let pid = child.id();
        let started_at = platform::process_start_time(pid, &executable).unwrap();
        let identity = ProcessIdentity {
            port_id: 1,
            pid: pid as i64,
            started_at: started_at as i64,
            executable: executable.to_string_lossy().into_owned(),
        };
        manager
            .running
            .lock()
            .unwrap()
            .insert(1, RunningProcess { child, identity });
        drop(manager);
        std::thread::sleep(Duration::from_millis(200));
        assert!(platform::process_start_time(pid, &executable).is_ok());
        let deadline = std::time::Instant::now() + Duration::from_secs(6);
        while std::time::Instant::now() < deadline
            && platform::process_identity_matches(
                pid,
                platform::process_start_time(pid, &executable).unwrap_or_default(),
                &executable,
            )
            .unwrap_or(false)
        {
            std::thread::sleep(Duration::from_millis(100));
        }
        let still_alive = platform::process_start_time(pid, &executable)
            .ok()
            .and_then(|started| platform::process_identity_matches(pid, started, &executable).ok())
            .unwrap_or(false);
        assert!(!still_alive);
    }
}
#[cfg(all(test, any(target_os = "macos", windows)))]
#[path = "lifecycle.rs"]
mod lifecycle;
#[cfg(test)]
#[path = "recovery.rs"]
mod recovery;
