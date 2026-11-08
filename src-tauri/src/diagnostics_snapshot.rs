//! Metadata-only support reports. Unknown fields are dropped, not regex-redacted.
use serde_json::{json, Map, Value};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_EXPORT: AtomicU64 = AtomicU64::new(0);
const MAX_REPORT: usize = 64 * 1024;

fn numbers(value: &Value, names: &[&str]) -> Value {
    let mut output = Map::new();
    for name in names {
        if let Some(number) = value.get(name).and_then(Value::as_u64) {
            output.insert((*name).into(), number.into());
        }
    }
    Value::Object(output)
}
fn known<'a>(value: &'a Value, allowed: &[&str]) -> &'a str {
    value.as_str().filter(|s| allowed.contains(s)).unwrap_or("unknown")
}

pub fn metadata_report(raw: &Value, version: &str, commit: Option<&str>, now_ms: u128) -> Value {
    let mut capture = numbers(&raw["capture"], &["per_capture_bytes", "retained_byte_limit", "queue_limit"]);
    capture["status"] = known(&raw["capture"]["status"], &["available", "unavailable"]).into();
    capture["enabled"] = raw["capture"]["enabled"].as_bool().map(Value::Bool).unwrap_or(Value::Null);
    capture["counters"] = numbers(&raw["capture"]["counters"], &[
        "started", "active", "completed_responses", "admitted", "queued", "writer_in_flight",
        "committed_imported", "committed_updated", "committed_unchanged", "invalid_snapshot",
        "unsupported_encoding", "decode_failed", "oversized", "dropped", "interrupted",
        "store_failed", "shutdown_unprocessed", "retained_bytes",
    ]);
    capture["last_error_code"] = known(&raw["capture"]["counters"]["last_error_code"], &[
        "invalid_snapshot", "unsupported_encoding", "capture_decode_failed", "capture_oversized",
        "capture_dropped", "capture_interrupted", "capture_store_failed", "capture_shutdown_unprocessed",
    ]).into();
    let mut history = numbers(&raw["history"], &[
        "processed_exports", "total_exports", "processed_conversations", "total_conversations",
        "imported", "updated", "unchanged", "failed", "retry_after",
    ]);
    history["state"] = known(&raw["history"]["state"], &[
        "idle", "running", "syncing", "complete", "completed", "error", "failed", "partial", "cancelled",
    ]).into();
    history["error_code"] = known(&raw["history"]["error_code"], &[
        "rate_limited", "signed_out", "login_required", "verification_required", "cancelled",
        "history_import_transport_failed", "history_import_http_503_busy", "store_failed",
        "provider_not_ready", "invalid_response", "response_too_large", "status_unavailable",
    ]).into();
    for key in ["configured", "auto_sync", "automatic_retry_paused", "challenge_detected"] {
        history[key] = raw["history"][key].as_bool().map(Value::Bool).unwrap_or(Value::Null);
    }
    let mut journal = numbers(&raw["journal"], &["pending_items", "retained_bytes", "incomplete_files"]);
    journal["status"] = known(&raw["journal"]["status"], &["available", "unavailable"]).into();
    let mut recovery = numbers(&raw["startup_recovery"], &[
        "pending_items", "retained_bytes", "incomplete_files", "recovered", "failed",
    ]);
    recovery["status"] = known(&raw["startup_recovery"]["status"], &["complete", "pending", "failed"]).into();
    let valid_version = !version.is_empty() && version.len() <= 64 &&
        version.bytes().all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b));
    let commit = commit.filter(|value| value.len() == 40 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    json!({
        "schema": 1, "kind": "chat-hub-metadata-diagnostics", "exported_unix_ms": now_ms.min(u64::MAX as u128) as u64,
        "build": {"app_version": if valid_version {version} else {"unknown"}, "source_commit": commit},
        "platform": {"os": std::env::consts::OS, "architecture": std::env::consts::ARCH},
        "privacy": {"conversation_content":false,"identifiers":false,"credentials":false,"paths":false,"raw_logs":false},
        "service": {
            "state": known(&raw["service"]["state"], &["starting","ready","stopping","stopped","failed","unavailable"]),
            "database_last_probe_healthy": raw["service"]["database_healthy"].as_bool(),
            "legacy_ingest": numbers(&raw["service"]["legacy_ingest"], &["accepted","stored","ignored","failed","unprocessed"])
        },
        "capture":capture,"history":history,"capture_journal":journal,"startup_recovery":recovery,
        "limitations": {
            "sqlite_integrity_check":"not_run",
            "sidebar_pending_metadata":"not_collected",
            "raw_logs":"intentionally_excluded",
            "journal_scope":"completed_normalized_captures_only"
        }
    })
}

/// The native Save dialog chooses the destination. The webview supplies no path.
pub fn write_report(path: &Path, report: &Value) -> Result<(), &'static str> {
    let data = serde_json::to_vec_pretty(report).map_err(|_| "diagnostics_encode_failed")?;
    if data.len() > MAX_REPORT { return Err("diagnostics_too_large"); }
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).ok_or("diagnostics_invalid_destination")?;
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() { return Err("diagnostics_unsafe_destination"); }
    }
    let temp = parent.join(format!(".chat-hub-diagnostics-{}-{}.tmp", std::process::id(), NEXT_EXPORT.fetch_add(1, Ordering::Relaxed)));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp).map_err(|_| "diagnostics_create_failed")?;
        file.write_all(&data).map_err(|_| "diagnostics_write_failed")?;
        file.sync_all().map_err(|_| "diagnostics_sync_failed")?;
        fs::rename(&temp, path).map_err(|_| "diagnostics_publish_failed")?;
        #[cfg(unix)]
        File::open(parent).and_then(|directory| directory.sync_all()).map_err(|_| "diagnostics_durability_unconfirmed")?;
        Ok(())
    })();
    let _ = fs::remove_file(temp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn allowlist_drops_content_ids_paths_tokens_and_unrecognized_errors() {
        let raw = json!({
            "token":"SECRET","conversation_id":"SECRET","path":"/SECRET",
            "capture":{"status":"available","enabled":true,"counters":{"store_failed":3,"message":"SECRET","last_error_code":"SECRET"}},
            "history":{"state":"SECRET","message":"SECRET","error_code":"SECRET","provider_error_code":"SECRET","imported":2,"retry_after":39,"failed":"SECRET"},
            "journal":{"status":"available","pending_items":4,"filename":"SECRET"}
        });
        let report = metadata_report(&raw, "0.2.5", Some("SECRET"), 123);
        assert!(!report.to_string().contains("SECRET"));
        assert_eq!(report["capture"]["counters"]["store_failed"], 3);
        assert_eq!(report["history"]["retry_after"], 39);
        assert_eq!(report["capture_journal"]["pending_items"], 4);
        assert_eq!(report["history"]["state"], "unknown");
        assert!(report["build"]["source_commit"].is_null());
        assert!(report["history"].get("failed").is_none());
    }
    #[test]
    fn atomic_report_is_valid_json_and_private() {
        let dir = std::env::temp_dir().join(format!("chat-hub-support-{}-{}", std::process::id(), NEXT_EXPORT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("diagnostics.json");
        let report = metadata_report(&json!({}), "0.2.5", Some(&"a".repeat(40)), 42);
        write_report(&path, &report).unwrap();
        let loaded: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(loaded, report);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            std::os::unix::fs::symlink(&path, dir.join("link.json")).unwrap();
            assert_eq!(write_report(&dir.join("link.json"), &report), Err("diagnostics_unsafe_destination"));
        }
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn report_limit_fails_before_creating_any_file() {
        let result = write_report(Path::new("unused.json"), &json!("x".repeat(MAX_REPORT + 1)));
        assert_eq!(result, Err("diagnostics_too_large"));
    }
}
