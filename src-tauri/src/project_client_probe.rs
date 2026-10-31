//! Bounded read-only inspection of public ChatGPT frontend bundles.
//!
//! Runs only in the app-owned ChatGPT webview. It fetches public JavaScript with
//! credentials omitted and returns route/field identifiers, never runtime data.

use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tauri::{Webview, Wry};

const MAX_CALLBACK_BYTES: usize = 256 * 1024;
const MAX_WAIT: Duration = Duration::from_secs(30);

const SCRIPT: &str = include_str!("project_client_probe.js");

fn callback_value(raw: &str) -> Option<Value> {
    if raw.len() > MAX_CALLBACK_BYTES {
        return None;
    }
    let mut value: Value = serde_json::from_str(raw).ok()?;
    for _ in 0..3 {
        let Some(nested) = value.as_str() else {
            break;
        };
        if nested.len() > MAX_CALLBACK_BYTES {
            return None;
        }
        value = serde_json::from_str(nested).ok()?;
    }
    Some(value)
}

fn nonce() -> String {
    let ticks = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{:032x}", ticks)
}

fn safe_result(value: &Value, expected_nonce: &str) -> Result<Value, String> {
    if value.get("nonce").and_then(Value::as_str) != Some(expected_nonce)
        || value.get("state").and_then(Value::as_str) != Some("complete")
    {
        return Err("project_client_probe_invalid_result".into());
    }

    let module_count = value
        .get("module_count")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(512);
    let fetched_count = value
        .get("fetched_count")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(64);
    let scanned_path_count = value
        .get("scanned_path_count")
        .and_then(Value::as_u64)
        .unwrap_or(fetched_count)
        .min(64);
    let interesting_token_count = value
        .get("interesting_token_count")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(100_000);
    let bytes = value
        .get("bytes")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(16 * 1024 * 1024);

    let mut findings = Vec::new();
    if let Some(items) = value.get("findings").and_then(Value::as_array) {
        for item in items.iter().take(20) {
            let Some(url) = item.get("url").and_then(Value::as_str) else {
                continue;
            };
            if url.len() > 2048
                || !url.starts_with('/')
                || url.contains(['?', '#', '@'])
                || !url.ends_with(".js") && !url.ends_with(".mjs")
            {
                continue;
            }
            let routes = item
                .get("routes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|route| {
                    let path = route.get("path")?.as_str()?;
                    let base_uncertain = route
                        .get("base_uncertain")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let backend_path = path.starts_with("/backend-api/")
                        || path.starts_with("https://chatgpt.com/backend-api/");
                    let family_path = ["/gizmo", "/gizmos", "/project", "/projects", "/conversation", "/conversations"]
                        .iter()
                        .any(|prefix| {
                            path == *prefix
                                || path
                                    .strip_prefix(*prefix)
                                    .is_some_and(|rest| rest.starts_with('/'))
                        });
                    if path.len() > 220
                        || (!backend_path && !(base_uncertain && family_path))
                        || path.contains(['?', '#', '@'])
                    {
                        return None;
                    }
                    let methods = route
                        .get("methods")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .filter(|method| {
                            matches!(*method, "PATCH" | "PUT" | "POST" | "GET" | "DELETE")
                        })
                        .take(5)
                        .map(str::to_owned)
                        .collect::<Vec<_>>();
                    let body_fields = route
                        .get("body_fields")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .filter(|field| {
                            matches!(
                                *field,
                                "gizmo_id"
                                    | "project_id"
                                    | "projectId"
                                    | "display"
                                    | "display_name"
                                    | "displayName"
                                    | "name"
                                    | "title"
                            )
                        })
                        .take(12)
                        .map(str::to_owned)
                        .collect::<Vec<_>>();
                    let context = route
                        .get("context")
                        .and_then(Value::as_str)
                        .filter(|context| context.len() <= 1200)
                        .map(str::to_owned);
                    Some(json!({"path": path, "base_uncertain": base_uncertain, "methods": methods, "body_fields": body_fields, "context": context}))
                })
                .take(30)
                .collect::<Vec<_>>();
            if !routes.is_empty() {
                findings.push(json!({"url": url, "routes": routes}));
            }
        }
    }

    Ok(json!({
        "state": "complete",
        "module_count": module_count,
        "fetched_count": fetched_count,
        "scanned_path_count": scanned_path_count,
        "interesting_token_count": interesting_token_count,
        "bytes": bytes,
        "findings": findings,
    }))
}

/// Inspect public ChatGPT frontend bundle strings without reading or changing
/// account data. The caller must pass the guarded app-owned provider webview.
pub fn inspect(view: Webview<Wry>) -> Result<Value, String> {
    let nonce = nonce();
    let key = format!("__chat_hub_project_probe_{nonce}");
    let key_json =
        serde_json::to_string(&key).map_err(|_| "project_client_probe_unavailable".to_string())?;
    let nonce_json = serde_json::to_string(&nonce)
        .map_err(|_| "project_client_probe_unavailable".to_string())?;

    view.eval(format!("({SCRIPT})({key_json},{nonce_json})"))
        .map_err(|_| "project_client_probe_unavailable".to_string())?;

    let cleanup = || {
        let _ = view.eval(format!("delete window[{key_json}]"));
    };

    let deadline = std::time::Instant::now() + MAX_WAIT;
    loop {
        if std::time::Instant::now() >= deadline {
            cleanup();
            return Err("project_client_probe_timeout".into());
        }
        let (tx, rx) = mpsc::sync_channel(1);
        view.eval_with_callback(
            format!("JSON.stringify(window[{key_json}]||null)"),
            move |result| {
                let _ = tx.send(result);
            },
        )
        .map_err(|_| {
            cleanup();
            "project_client_probe_unavailable".to_string()
        })?;
        if let Ok(raw) = rx.recv_timeout(Duration::from_secs(2)) {
            let Some(value) = callback_value(&raw) else {
                cleanup();
                return Err("project_client_probe_invalid_result".into());
            };
            match value.get("state").and_then(Value::as_str) {
                Some("complete") => {
                    let result = safe_result(&value, &nonce);
                    cleanup();
                    return result;
                }
                Some("running") => {}
                _ => {
                    cleanup();
                    return Err("project_client_probe_not_ready".into());
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_result_keeps_only_allowlisted_route_and_field_metadata() {
        let value = json!({
            "nonce": "abc",
            "state": "complete",
            "module_count": 900,
            "fetched_count": 65,
            "scanned_path_count": 65,
            "interesting_token_count": 100_001,
            "bytes": 20_000_000,
            "findings": [{
                "url": "/cdn/assets/app.js",
                "routes": [
                    {"path":"/backend-api/gizmos/{id}", "base_uncertain":false, "methods":["PATCH","TRACE"], "body_fields":["gizmo_id","accessToken","name"], "context":"method: PATCH gizmo_id name"},
                    {"path":"/gizmos/{id}", "base_uncertain":true, "methods":["PUT"], "body_fields":["display","name"], "context":"base.gizmos/{id} display.name"},
                    {"path":"https://evil.example/steal", "base_uncertain":true, "methods":[], "body_fields":[]},
                    {"path":"/backend-api/project?secret=1", "base_uncertain":false, "methods":[], "body_fields":[]}
                ]
            }]
        });
        let safe = safe_result(&value, "abc").unwrap();
        assert_eq!(safe["module_count"], 512);
        assert_eq!(safe["fetched_count"], 64);
        assert_eq!(safe["scanned_path_count"], 64);
        assert_eq!(safe["interesting_token_count"], 100_000);
        assert_eq!(safe["bytes"], 16 * 1024 * 1024);
        assert_eq!(
            safe["findings"][0]["routes"][0]["path"],
            "/backend-api/gizmos/{id}"
        );
        assert_eq!(
            safe["findings"][0]["routes"][0]["methods"],
            json!(["PATCH"])
        );
        assert_eq!(
            safe["findings"][0]["routes"][0]["body_fields"],
            json!(["gizmo_id", "name"])
        );
        assert_eq!(safe["findings"][0]["routes"][1]["path"], "/gizmos/{id}");
        assert_eq!(safe["findings"][0]["routes"][1]["base_uncertain"], true);
        assert_eq!(
            safe["findings"][0]["routes"][1]["body_fields"],
            json!(["display", "name"])
        );
    }

    #[test]
    fn safe_result_rejects_wrong_nonce_or_incomplete_probe() {
        let value = json!({"nonce":"wrong", "state":"complete"});
        assert!(safe_result(&value, "expected").is_err());
        let value = json!({"nonce":"expected", "state":"running"});
        assert!(safe_result(&value, "expected").is_err());
    }

    #[test]
    fn callback_rejects_oversized_values() {
        assert!(callback_value(&"x".repeat(MAX_CALLBACK_BYTES + 1)).is_none());
    }
}
