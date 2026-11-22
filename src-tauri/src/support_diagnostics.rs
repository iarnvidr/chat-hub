//! Native-menu initiated export: no new provider-accessible IPC or file-write API.
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_dialog::DialogExt;

#[path = "diagnostics_snapshot.rs"]
mod snapshot;

fn collect<R: Runtime>(app: &AppHandle<R>) -> Value {
    let mut raw = json!({"service":{"state":"unavailable"}});
    let mut database = None;
    if let Some(state) = app.try_state::<crate::AppState>() {
        if let Ok(runtime) = state.runtime.try_lock() {
            if let Some(runtime) = runtime.as_ref() {
                let status = runtime.status();
                let lifecycle = match status.state {
                    chat_hub::service::ServiceState::Starting => "starting",
                    chat_hub::service::ServiceState::Ready => "ready",
                    chat_hub::service::ServiceState::Stopping => "stopping",
                    chat_hub::service::ServiceState::Stopped => "stopped",
                    chat_hub::service::ServiceState::Failed(_) => "failed",
                };
                raw["service"] = json!({
                    "state":lifecycle,"database_healthy":status.database_healthy,
                    "legacy_ingest":{"accepted":status.ingest.accepted,"stored":status.ingest.stored,
                        "ignored":status.ingest.ignored,"failed":status.ingest.failed,"unprocessed":status.ingest.unprocessed}
                });
                raw["capture"] =
                    serde_json::to_value(runtime.capture_snapshot()).unwrap_or(Value::Null);
                raw["startup_recovery"] = runtime.capture_recovery.clone();
                database = Some(runtime.database_path.clone());
            }
        };
    }
    // Drop the runtime mutex before filesystem work. Export must not stop capture.
    raw["journal"] = database
        .as_ref()
        .and_then(|path| path.to_str())
        .and_then(|path| chat_hub::store::Store::capture_recovery_status(path).ok())
        .and_then(|status| serde_json::to_value(status).ok())
        .map(|mut value| {
            value["status"] = "available".into();
            value
        })
        .unwrap_or_else(|| json!({"status":"unavailable"}));
    raw["history"] = app
        .try_state::<crate::chatgpt_history::ChatgptHistoryState>()
        .and_then(|state| state.snapshot().ok())
        .and_then(|status| serde_json::to_value(status).ok())
        .unwrap_or(Value::Null);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    snapshot::metadata_report(
        &raw,
        env!("CARGO_PKG_VERSION"),
        option_env!("CHAT_HUB_BUILD_COMMIT"),
        now,
    )
}

pub fn export<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let report = collect(&app);
        let (sender, receiver) = tokio::sync::oneshot::channel();
        app.dialog()
            .file()
            .set_title("Export metadata-only diagnostics")
            .set_file_name("chat-hub-diagnostics.json")
            .add_filter("JSON", &["json"])
            .save_file(move |path| {
                let _ = sender.send(path);
            });
        let Ok(Some(path)) = receiver.await else {
            return;
        };
        let Ok(path) = path.into_path() else {
            app.dialog()
                .message("The selected diagnostics destination is unavailable.")
                .title("Chat Hub diagnostics")
                .show(|_| {});
            return;
        };
        let result =
            tokio::task::spawn_blocking(move || snapshot::write_report(&path, &report)).await;
        let message = match result {
            Ok(Ok(())) => "Metadata-only diagnostics saved. Conversation text, identifiers, credentials, paths and raw logs were excluded.",
            Ok(Err("diagnostics_durability_unconfirmed")) => "The diagnostics file was written, but disk durability could not be confirmed.",
            _ => "Diagnostics could not be saved. Choose another destination and retry; your library was not changed.",
        };
        app.dialog()
            .message(message)
            .title("Chat Hub diagnostics")
            .show(|_| {});
    });
}
