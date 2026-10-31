//! Native-owned ChatGPT history jobs. Fetching uses the app's signed-in
//! provider webview; the provider never receives local API credentials or IPC.
use crate::{browser_history, desktop_api, desktop_archive, provider_view::ProviderViewState};
use serde::Serialize;
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager};

#[derive(Clone, Debug, Serialize, serde::Deserialize)]
pub struct ChatgptHistoryStatus {
    pub state: String,
    pub message: String,
    pub configured: bool,
    pub auto_sync: bool,
    pub processed_exports: u64,
    pub total_exports: u64,
    pub processed_conversations: u64,
    pub total_conversations: u64,
    pub imported: u64,
    pub updated: u64,
    pub unchanged: u64,
    pub failed: u64,
    pub error_code: Option<String>,
    pub retry_after: Option<u64>,
}

impl Default for ChatgptHistoryStatus {
    fn default() -> Self {
        Self {
            state: "idle".into(),
            message: "Connect ChatGPT to fetch your history.".into(),
            configured: true,
            auto_sync: false,
            processed_exports: 0,
            total_exports: 0,
            processed_conversations: 0,
            total_conversations: 0,
            imported: 0,
            updated: 0,
            unchanged: 0,
            failed: 0,
            error_code: None,
            retry_after: None,
        }
    }
}

#[derive(Clone)]
pub struct ChatgptHistoryState {
    status: Arc<Mutex<ChatgptHistoryStatus>>,
    cancel: Arc<AtomicBool>,
    next_attempt: Arc<Mutex<Option<Instant>>>,
    status_path: PathBuf,
    persist_lock: Arc<Mutex<()>>,
}

const STATUS_FILE_NAME: &str = "chatgpt-history-status.json";
static NEXT_STATUS_TEMP: AtomicU64 = AtomicU64::new(0);

impl ChatgptHistoryState {
    pub fn new(app_data_dir: PathBuf) -> Self {
        Self {
            status: Arc::new(Mutex::new(ChatgptHistoryStatus::default())),
            cancel: Arc::new(AtomicBool::new(false)),
            next_attempt: Arc::new(Mutex::new(None)),
            status_path: app_data_dir.join(STATUS_FILE_NAME),
            persist_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn snapshot(&self) -> Result<ChatgptHistoryStatus, String> {
        let _persist = self.persist_lock.lock().map_err(|_| "status_unavailable")?;
        let mut status = self
            .status
            .lock()
            .map_err(|_| "status_unavailable")?
            .clone();
        status.retry_after = self
            .next_attempt
            .lock()
            .map_err(|_| "status_unavailable")?
            .and_then(|deadline| deadline.checked_duration_since(Instant::now()))
            .map(|remaining| remaining.as_secs().saturating_add(1));
        // Diagnostic persistence is best-effort: a filesystem error must not
        // interrupt or fail the history job itself.
        let _ = write_status_snapshot(&self.status_path, &status);
        Ok(status)
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }

    /// Extend the shared provider cooldown after a separate provider action
    /// receives HTTP 429. This deliberately leaves the last sync receipt intact.
    pub fn note_provider_rate_limit(&self, retry_after: Option<u64>) {
        let seconds = retry_after.unwrap_or(60).clamp(1, 1800);
        let Some(deadline) = Instant::now().checked_add(Duration::from_secs(seconds)) else {
            return;
        };
        if let Ok(mut next_attempt) = self.next_attempt.lock() {
            if next_attempt.is_none_or(|current| current < deadline) {
                *next_attempt = Some(deadline);
            }
        }
    }

    pub fn start(
        &self,
        app: AppHandle,
        api: String,
        token: String,
        archive_permit: desktop_archive::Permit,
    ) -> Result<ChatgptHistoryStatus, String> {
        if self
            .next_attempt
            .lock()
            .map_err(|_| "status_unavailable")?
            .is_some_and(|deadline| deadline > Instant::now())
        {
            return Err("history_rate_limited".into());
        }
        let view = app
            .state::<ProviderViewState>()
            .history_webview()
            .map_err(|_| "chatgpt_not_open")?;
        {
            let mut status = self.status.lock().map_err(|_| "status_unavailable")?;
            if matches!(status.state.as_str(), "running" | "syncing") {
                return Err("archive_busy".into());
            }
            *status = ChatgptHistoryStatus {
                state: "running".into(),
                message: "Fetching history directly from your signed-in ChatGPT account…".into(),
                ..ChatgptHistoryStatus::default()
            };
        }
        self.cancel.store(false, Ordering::Release);
        let status = self.status.clone();
        let cancel = self.cancel.clone();
        let next_attempt = self.next_attempt.clone();
        let worker_status = status.clone();
        let result = thread::Builder::new()
            .name("chatgpt-history-sync".into())
            .spawn(move || {
                let _permit = archive_permit;
                let result = browser_history::run(
                    view,
                    &cancel,
                    |data, count| {
                        let receipt = tauri::async_runtime::block_on(
                            desktop_api::import_chatgpt_history(&api, &token, data.to_vec(), count),
                        )?;
                        let mut current = worker_status.lock().map_err(|_| "status_unavailable")?;
                        current.processed_exports += 1;
                        current.processed_conversations =
                            current.processed_conversations.saturating_add(count);
                        current.imported = current.imported.saturating_add(receipt.imported);
                        current.updated = current.updated.saturating_add(receipt.updated);
                        current.unchanged = current.unchanged.saturating_add(receipt.unchanged);
                        current.failed = current.failed.saturating_add(receipt.failed);
                        if receipt.failed != 0 {
                            return Err("history_import_failed".into());
                        }
                        Ok(())
                    },
                    |stage, count| {
                        if let Ok(mut current) = worker_status.lock() {
                            current.total_conversations = current.total_conversations.max(count);
                            current.message = match stage {
                                "detail" => "Saving fetched conversations in your local library…",
                                "project" => "Fetching ChatGPT project conversations…",
                                _ => "Fetching your conversation list from ChatGPT…",
                            }
                            .into();
                        }
                    },
                );
                if let Ok(mut current) = worker_status.lock() {
                    match result {
                        Ok(outcome) if outcome.complete => {
                            current.state = "completed".into();
                            current.message =
                                "ChatGPT history is saved in your local library.".into();
                            current.total_conversations = current.processed_conversations;
                        }
                        Ok(outcome) => {
                            current.state = if current.processed_conversations > 0 {
                                "partial"
                            } else {
                                "failed"
                            }
                            .into();
                            current.message =
                                history_error_message(outcome.error_code.as_deref()).into();
                            current.error_code = outcome.error_code;
                            if let Some(seconds) = outcome
                                .retry_after
                                .filter(|_| current.error_code.as_deref() == Some("rate_limited"))
                            {
                                if let Ok(mut deadline) = next_attempt.lock() {
                                    *deadline = Some(
                                        Instant::now()
                                            + Duration::from_secs(seconds.clamp(1, 1800)),
                                    );
                                }
                                current.retry_after = Some(seconds.clamp(1, 1800));
                            }
                        }
                        Err(code) => {
                            current.state = if cancel.load(Ordering::Acquire) {
                                "cancelled"
                            } else if current.processed_conversations > 0 {
                                "partial"
                            } else {
                                "failed"
                            }
                            .into();
                            current.message = history_error_message(Some(&code)).into();
                            current.error_code = Some(code);
                        }
                    }
                }
                let _ = app;
            });
        if result.is_err() {
            if let Ok(mut current) = status.lock() {
                current.state = "failed".into();
                current.message = "History sync could not start.".into();
            }
            return Err("history_start_failed".into());
        }
        self.snapshot()
    }
}

fn write_status_snapshot(destination: &Path, status: &ChatgptHistoryStatus) -> std::io::Result<()> {
    let parent = destination
        .parent()
        .ok_or_else(|| std::io::Error::other("missing status directory"))?;
    let parent_metadata = fs::symlink_metadata(parent)?;
    if !parent_metadata.file_type().is_dir() {
        return Err(std::io::Error::other(
            "status directory is not a regular directory",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    if let Ok(metadata) = fs::symlink_metadata(destination) {
        if !metadata.file_type().is_file() {
            return Err(std::io::Error::other("status path is not a regular file"));
        }
    }
    let sequence = NEXT_STATUS_TEMP.fetch_add(1, Ordering::Relaxed);
    let staging = parent.join(format!(
        ".{STATUS_FILE_NAME}.{}.{}.tmp",
        std::process::id(),
        sequence
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&staging)?;
        serde_json::to_writer(&mut file, status).map_err(std::io::Error::other)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        drop(file);
        fs::rename(&staging, destination)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(staging);
    }
    result
}

fn history_error_message(code: Option<&str>) -> &'static str {
    match code {
        Some("signed_out" | "login_required" | "chatgpt_not_open") => "Sign in to ChatGPT in this app, then sync history again. Your saved chats remain available.",
        Some("rate_limited") => "ChatGPT is limiting history requests. Fetched progress was saved; wait before retrying.",
        Some("cancelled" | "history_interrupted") => "History sync stopped. Conversations already saved remain in your library.",
        Some("network_error") => "ChatGPT could not be reached. Your saved chats remain available offline.",
        _ => "ChatGPT history could not finish. Any fetched conversations were saved; existing history was kept.",
    }
}

#[cfg(test)]
mod status_snapshot_tests {
    use super::*;

    #[test]
    fn snapshot_persists_only_status_dto_atomically_with_private_permissions() {
        let root = std::env::temp_dir().join(format!(
            "chat-hub-history-status-test-{}-{}",
            std::process::id(),
            NEXT_STATUS_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let state = ChatgptHistoryState::new(root.clone());
        let status = state.snapshot().unwrap();
        let path = root.join(STATUS_FILE_NAME);
        let bytes = fs::read(&path).unwrap();
        let stored: ChatgptHistoryStatus = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(stored.state, status.state);
        assert_eq!(stored.message, status.message);
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 14);
        assert!(value.get("account_scope").is_none());
        assert!(value.get("token").is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn diagnostic_write_failure_does_not_fail_status_snapshot() {
        let root = std::env::temp_dir().join(format!(
            "chat-hub-history-status-failure-test-{}-{}",
            std::process::id(),
            NEXT_STATUS_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(root.join(STATUS_FILE_NAME)).unwrap();
        let state = ChatgptHistoryState::new(root.clone());
        assert_eq!(state.snapshot().unwrap().state, "idle");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn external_rate_limit_extends_cooldown_without_replacing_sync_receipt() {
        let state = ChatgptHistoryState::new(PathBuf::new());
        {
            let mut status = state.status.lock().unwrap();
            status.state = "complete".into();
            status.imported = 7;
            status.updated = 3;
        }
        state.note_provider_rate_limit(Some(120));
        state.note_provider_rate_limit(Some(1));
        let status = state.status.lock().unwrap().clone();
        assert_eq!(status.state, "complete");
        assert_eq!(status.imported, 7);
        assert_eq!(status.updated, 3);
        let retry_after = state
            .next_attempt
            .lock()
            .unwrap()
            .and_then(|deadline| deadline.checked_duration_since(Instant::now()))
            .unwrap()
            .as_secs();
        assert!(retry_after >= 118, "remaining retry seconds: {retry_after}");
    }
}
