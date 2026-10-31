//! Background import bridge for the user's local Paseo ChatGPT history plugin.
use crate::{desktop_api, desktop_archive};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    env,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use tauri::AppHandle;

const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_STDERR_BYTES: usize = 16 * 1024;
const MAX_EXPORTS: usize = 4096;
const MAX_EXPORT_BYTES: u64 = 60 * 1024 * 1024;
const MAX_BRIDGE_TIME: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Debug, Serialize)]
pub struct PaseoHistoryStatus {
    pub state: String,
    pub message: String,
    pub configured: bool,
    pub processed_exports: u64,
    pub total_exports: u64,
    pub processed_conversations: u64,
    pub total_conversations: u64,
    pub imported: u64,
    pub updated: u64,
    pub unchanged: u64,
    pub failed: u64,
}

impl Default for PaseoHistoryStatus {
    fn default() -> Self {
        Self {
            state: "idle".into(),
            message: "Paseo history has not been synced yet.".into(),
            configured: false,
            processed_exports: 0,
            total_exports: 0,
            processed_conversations: 0,
            total_conversations: 0,
            imported: 0,
            updated: 0,
            unchanged: 0,
            failed: 0,
        }
    }
}

#[derive(Clone)]
pub struct PaseoHistoryState {
    app_data_dir: PathBuf,
    status: Arc<Mutex<PaseoHistoryStatus>>,
    cancel: Arc<AtomicBool>,
}

#[derive(Deserialize)]
struct SourceConfig {
    plugin_path: Option<String>,
    python_path: String,
    cache_database: Option<String>,
    cache_owner: Option<String>,
}

#[derive(Deserialize)]
struct Manifest {
    owner: String,
    exports: Vec<String>,
    conversations: u64,
}

impl PaseoHistoryState {
    pub fn new(app_data_dir: PathBuf) -> Self {
        let configured = read_source_config(&app_data_dir).is_ok();
        Self {
            app_data_dir,
            status: Arc::new(Mutex::new(PaseoHistoryStatus {
                configured,
                message: if configured {
                    "Paseo history is ready to sync.".into()
                } else {
                    "Paseo history is not configured.".into()
                },
                ..PaseoHistoryStatus::default()
            })),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn snapshot(&self) -> Result<PaseoHistoryStatus, String> {
        self.status
            .lock()
            .map(|s| s.clone())
            .map_err(|_| "status_unavailable".into())
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }

    pub fn start(
        &self,
        app: AppHandle,
        api: String,
        token: String,
        _archive_permit: desktop_archive::Permit,
    ) -> Result<PaseoHistoryStatus, String> {
        let config = read_source_config(&self.app_data_dir)?;
        let shared_home = if config.cache_database.is_some() {
            None
        } else {
            Some(resolve_shared_home()?)
        };
        let work = prepare_workdirs(&self.app_data_dir)?;
        let bridge = match if config.cache_database.is_some() {
            write_cache_export_script(&work.state_dir)
        } else {
            write_bridge_script(&work.state_dir)
        } {
            Ok(bridge) => bridge,
            Err(error) => {
                let _ = fs::remove_dir_all(&work.output_dir);
                return Err(error);
            }
        };
        {
            let mut status = self.status.lock().map_err(|_| "status_unavailable")?;
            if matches!(status.state.as_str(), "starting" | "running" | "syncing") {
                let _ = fs::remove_dir_all(&work.output_dir);
                return Err("archive_busy".into());
            }
            *status = PaseoHistoryStatus {
                state: "running".into(),
                message: if config.cache_database.is_some() {
                    "Loading saved Paseo history…".into()
                } else {
                    "Connecting to your local Paseo history…".into()
                },
                configured: true,
                ..PaseoHistoryStatus::default()
            };
        }
        self.cancel.store(false, Ordering::Release);
        let status = self.status.clone();
        let cancel = self.cancel.clone();
        let spawn = thread::Builder::new()
            .name("paseo-history-sync".into())
            .spawn(move || {
                let _permit = _archive_permit;
                run_sync(
                    app,
                    api,
                    token,
                    config,
                    shared_home,
                    bridge,
                    work,
                    status,
                    cancel,
                );
            });
        if spawn.is_err() {
            set_status(
                &self.status,
                "failed",
                "Paseo history sync could not be started.",
            );
            return Err("sync_start_failed".into());
        }
        self.snapshot()
    }
}

struct WorkDirs {
    state_dir: PathBuf,
    output_dir: PathBuf,
}

fn prepare_workdirs(app_data_dir: &Path) -> Result<WorkDirs, String> {
    let state_dir = app_data_dir.join("paseo-history");
    fs::create_dir_all(&state_dir).map_err(|_| "private_storage_unavailable")?;
    set_private_dir(&state_dir)?;

    let output_root = state_dir.join("sync-output");
    fs::create_dir_all(&output_root).map_err(|_| "private_storage_unavailable")?;
    set_private_dir(&output_root)?;
    let output_dir = output_root.join(random_id()?);
    // Both exporters publish this directory atomically. The private parent is
    // prepared here; the destination must remain absent until publication.
    Ok(WorkDirs {
        state_dir,
        output_dir,
    })
}

fn read_source_config(app_data_dir: &Path) -> Result<SourceConfig, String> {
    let path = app_data_dir.join("paseo-history-source.json");
    let metadata = fs::symlink_metadata(&path).map_err(|_| "source_not_configured")?;
    if !metadata.file_type().is_file() || metadata.len() > 16 * 1024 {
        return Err("invalid_source_config".into());
    }
    let bytes = fs::read(path).map_err(|_| "invalid_source_config")?;
    let config: SourceConfig =
        serde_json::from_slice(&bytes).map_err(|_| "invalid_source_config")?;
    let python = PathBuf::from(&config.python_path);
    if !python.is_absolute() || !python.is_file() {
        return Err("python_unavailable".into());
    }
    match (&config.cache_database, &config.cache_owner) {
        (Some(database), Some(owner)) => {
            let database = PathBuf::from(database);
            if !database.is_absolute() || !database.is_file() || !valid_owner(owner) {
                return Err("saved_cache_unavailable".into());
            }
        }
        (None, None) => {
            let plugin = config
                .plugin_path
                .as_deref()
                .map(PathBuf::from)
                .ok_or("invalid_source_config")?;
            if !plugin.is_absolute() || !plugin.is_dir() {
                return Err("source_unavailable".into());
            }
        }
        _ => return Err("invalid_source_config".into()),
    }
    Ok(config)
}

fn resolve_shared_home() -> Result<PathBuf, String> {
    let home = env::var_os("PASEO_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|p| PathBuf::from(p).join(".paseo")))
        .ok_or("shared_paseo_unavailable")?;
    if !home.is_absolute() || !home.is_dir() {
        return Err("shared_paseo_unavailable".into());
    }
    Ok(home)
}

fn write_bridge_script(state_dir: &Path) -> Result<PathBuf, String> {
    write_embedded_script(
        state_dir,
        "paseo_history_bridge.py",
        include_str!("../../tools/paseo_history_bridge.py"),
    )
}

fn write_cache_export_script(state_dir: &Path) -> Result<PathBuf, String> {
    write_embedded_script(
        state_dir,
        "paseo_cache_export.py",
        include_str!("../../tools/paseo_cache_export.py"),
    )
}

fn write_embedded_script(state_dir: &Path, name: &str, source: &str) -> Result<PathBuf, String> {
    let destination = state_dir.join(name);
    let temporary = state_dir.join(format!("paseo_history_bridge.{}.tmp", random_id()?));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|_| "private_storage_unavailable")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| "private_storage_unavailable")?;
    }
    file.write_all(source.as_bytes())
        .map_err(|_| "private_storage_unavailable")?;
    file.sync_all().map_err(|_| "private_storage_unavailable")?;
    fs::rename(&temporary, &destination).map_err(|_| "private_storage_unavailable")?;
    Ok(destination)
}

fn random_id() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|_| "secure_random_unavailable")?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn set_private_dir(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| "private_storage_unavailable")?;
    }
    Ok(())
}

fn set_status(status: &Arc<Mutex<PaseoHistoryStatus>>, state: &str, message: &str) {
    if let Ok(mut current) = status.lock() {
        current.state = state.into();
        current.message = message.into();
    }
}

fn update_status<F: FnOnce(&mut PaseoHistoryStatus)>(
    status: &Arc<Mutex<PaseoHistoryStatus>>,
    update: F,
) {
    if let Ok(mut current) = status.lock() {
        update(&mut current);
    }
}

fn run_sync(
    app: AppHandle,
    api: String,
    token: String,
    config: SourceConfig,
    shared_home: Option<PathBuf>,
    bridge: PathBuf,
    work: WorkDirs,
    status: Arc<Mutex<PaseoHistoryStatus>>,
    cancel: Arc<AtomicBool>,
) {
    let result = if let (Some(database), Some(owner)) = (
        config.cache_database.as_deref(),
        config.cache_owner.as_deref(),
    ) {
        run_cache_export(&config, database, owner, &bridge, &work.output_dir, &cancel)
    } else if let Some(shared_home) = shared_home.as_deref() {
        run_bridge(
            &config,
            shared_home,
            &bridge,
            &work.state_dir,
            &work.output_dir,
            &cancel,
        )
    } else {
        Err(("failed", "Paseo history source is unavailable."))
    };
    let manifest = match result {
        Ok(manifest) => manifest,
        Err((state, message)) => {
            set_status(&status, state, message);
            return;
        }
    };

    update_status(&status, |s| {
        s.state = "syncing".into();
        s.message = "Importing conversations into your local library…".into();
        s.total_exports = manifest.exports.len() as u64;
        s.total_conversations = manifest.conversations;
    });

    let mut imported_total = 0u64;
    let mut updated_total = 0u64;
    let mut unchanged_total = 0u64;
    let mut failed_total = 0u64;
    let mut processed_conversations = 0u64;
    for name in &manifest.exports {
        if cancel.load(Ordering::Acquire) {
            set_status(
                &status,
                "cancelled",
                "Sync stopped because Chat Hub is closing.",
            );
            return;
        }
        let path = match export_path(&work.output_dir, name) {
            Ok(path) => path,
            Err(_) => {
                set_status(
                    &status,
                    "failed",
                    "Paseo history produced an invalid export file.",
                );
                return;
            }
        };
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata)
                if metadata.file_type().is_file()
                    && metadata.len() > 0
                    && metadata.len() <= MAX_EXPORT_BYTES =>
            {
                metadata
            }
            _ => {
                set_status(
                    &status,
                    "failed",
                    "A Paseo history export could not be read safely.",
                );
                return;
            }
        };
        let data = match fs::read(&path) {
            Ok(data) if data.len() as u64 == metadata.len() => data,
            _ => {
                set_status(
                    &status,
                    "failed",
                    "A Paseo history export could not be read safely.",
                );
                return;
            }
        };
        let count = match validate_export(&data, &manifest.owner, config.cache_database.is_some()) {
            Ok(count) => count,
            Err(_) => {
                set_status(
                    &status,
                    "failed",
                    "Paseo history returned an invalid export.",
                );
                return;
            }
        };
        if count > desktop_api::MAX_IMPORT_CONVERSATIONS as u64 {
            set_status(
                &status,
                "failed",
                "A Paseo history export exceeded the import limit.",
            );
            return;
        }

        let receipt = tauri::async_runtime::block_on(desktop_api::import_paseo_history(
            &api, &token, data, count,
        ));
        match receipt {
            Ok(receipt) => {
                imported_total = imported_total.saturating_add(receipt.imported);
                updated_total = updated_total.saturating_add(receipt.updated);
                unchanged_total = unchanged_total.saturating_add(receipt.unchanged);
                failed_total = failed_total.saturating_add(receipt.failed);
                processed_conversations = processed_conversations.saturating_add(count);
                update_status(&status, |s| {
                    s.processed_exports = s.processed_exports.saturating_add(1);
                    s.processed_conversations = processed_conversations;
                    s.imported = imported_total;
                    s.updated = updated_total;
                    s.unchanged = unchanged_total;
                    s.failed = failed_total;
                });
                if receipt.failed > 0 {
                    set_status(
                        &status,
                        "failed",
                        "Some conversations could not be imported; the private exports were kept for retry.",
                    );
                    return;
                }
            }
            Err(_) => {
                set_status(
                    &status,
                    "failed",
                    "The local library could not finish importing Paseo history; private exports were kept for retry.",
                );
                return;
            }
        }
    }

    if processed_conversations != manifest.conversations {
        set_status(
            &status,
            "failed",
            "Paseo history export counts did not match its manifest.",
        );
        return;
    }
    let cached_transcripts = config.cache_database.is_some();
    if fs::remove_dir_all(&work.output_dir).is_err() {
        update_status(&status, |s| {
            s.state = "completed".into();
            s.message = if cached_transcripts {
                "Saved Paseo history loaded. Temporary private exports remain and can be removed later.".into()
            } else {
                "Paseo history synced. Temporary private exports remain and can be removed later."
                    .into()
            };
        });
        return;
    }
    set_status(
        &status,
        "completed",
        if cached_transcripts {
            "Saved Paseo history loaded into your local library."
        } else {
            "Paseo history synced to your local library."
        },
    );
    let _ = app;
}

fn run_bridge(
    config: &SourceConfig,
    shared_home: &Path,
    bridge: &Path,
    state_dir: &Path,
    output_dir: &Path,
    cancel: &AtomicBool,
) -> Result<Manifest, (&'static str, &'static str)> {
    let plugin_path = config
        .plugin_path
        .as_deref()
        .ok_or(("failed", "Paseo history source is unavailable."))?;
    let child = Command::new(&config.python_path)
        .arg(bridge)
        .arg("--plugin")
        .arg(plugin_path)
        .arg("--state-dir")
        .arg(state_dir)
        .arg("--shared-paseo-home")
        .arg(shared_home)
        .arg("--output-dir")
        .arg(output_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| ("failed", "Paseo history could not be started."))?;

    read_export_manifest(child, cancel)
}

fn run_cache_export(
    config: &SourceConfig,
    database: &str,
    owner: &str,
    exporter: &Path,
    output_dir: &Path,
    cancel: &AtomicBool,
) -> Result<Manifest, (&'static str, &'static str)> {
    let child = Command::new(&config.python_path)
        .arg(exporter)
        .arg("--database")
        .arg(database)
        .arg("--owner")
        .arg(owner)
        .arg("--output-dir")
        .arg(output_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| ("failed", "Saved Paseo history could not be read."))?;
    read_export_manifest(child, cancel).map_err(|(state, message)| {
        if state == "cancelled" {
            (state, message)
        } else {
            ("failed", "Saved Paseo history could not be read.")
        }
    })
}

fn read_export_manifest(
    mut child: Child,
    cancel: &AtomicBool,
) -> Result<Manifest, (&'static str, &'static str)> {
    let stdout = child
        .stdout
        .take()
        .ok_or(("failed", "Paseo history could not be started."))?;
    let stderr = child
        .stderr
        .take()
        .ok_or(("failed", "Paseo history could not be started."))?;
    let stdout_reader = thread::spawn(move || read_bounded(stdout, MAX_MANIFEST_BYTES));
    let stderr_reader = thread::spawn(move || read_bounded(stderr, MAX_STDERR_BYTES));
    let started = Instant::now();
    let exit = loop {
        if cancel.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(("cancelled", "Sync stopped because Chat Hub is closing."));
        }
        if started.elapsed() >= MAX_BRIDGE_TIME {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(("failed", "Paseo history sync timed out. Try again later."));
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(Duration::from_millis(100)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(("failed", "Paseo history sync stopped unexpectedly."));
            }
        }
    };
    let (stdout, stdout_overflow) = stdout_reader
        .join()
        .map_err(|_| ("failed", "Paseo history returned an invalid manifest."))?;
    let _ = stderr_reader.join(); // Deliberately discard provider/plugin diagnostics.
    if !exit.success() {
        return Err((
            "failed",
            "Paseo could not read history from the configured source.",
        ));
    }
    if stdout_overflow {
        return Err(("failed", "Paseo history returned an oversized manifest."));
    }
    let manifest: Manifest = serde_json::from_slice(&stdout)
        .map_err(|_| ("failed", "Paseo history returned an invalid manifest."))?;
    if !valid_owner(&manifest.owner)
        || manifest.exports.len() > MAX_EXPORTS
        || manifest.conversations > 1_000_000
        || (manifest.exports.is_empty() && manifest.conversations != 0)
        || manifest.exports.iter().any(|name| !valid_export_name(name))
    {
        return Err(("failed", "Paseo history returned an invalid manifest."));
    }
    Ok(manifest)
}

fn read_bounded<R: Read>(mut reader: R, limit: usize) -> (Vec<u8>, bool) {
    let mut kept = Vec::with_capacity(limit.min(16 * 1024));
    let mut buffer = [0u8; 8192];
    let mut overflow = false;
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(size) => {
                let remaining = limit.saturating_sub(kept.len());
                let keep = remaining.min(size);
                kept.extend_from_slice(&buffer[..keep]);
                overflow |= keep < size;
            }
        }
    }
    (kept, overflow)
}

fn valid_owner(owner: &str) -> bool {
    owner.len() == 24 && owner.bytes().all(|b| b.is_ascii_hexdigit())
}

fn valid_export_name(name: &str) -> bool {
    name.ends_with(".json")
        && !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && name != ".json"
        && Path::new(name).components().count() == 1
}

fn export_path(output_dir: &Path, name: &str) -> Result<PathBuf, String> {
    if !valid_export_name(name) {
        return Err("invalid_export".into());
    }
    Ok(output_dir.join(name))
}

fn validate_export(data: &[u8], owner: &str, cached_transcripts: bool) -> Result<u64, String> {
    if data.len() as u64 > MAX_EXPORT_BYTES {
        return Err("file_too_large".into());
    }
    desktop_api::validate_import(data)?;
    let value: Value = serde_json::from_slice(data).map_err(|_| "invalid_json")?;
    if value.get("owner").and_then(Value::as_str) != Some(owner) {
        return Err("owner_mismatch".into());
    }
    match value.get("format").and_then(Value::as_str) {
        Some("paseo-cached-transcripts-v1") if cached_transcripts => {}
        None if !cached_transcripts => {}
        _ => return Err("invalid_format".into()),
    }
    let count = value
        .get("conversations")
        .and_then(Value::as_array)
        .ok_or("invalid_export")?
        .len();
    Ok(count as u64)
}
