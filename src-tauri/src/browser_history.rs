//! Native bridge for importing history from the app-owned ChatGPT webview.
//! Only bounded conversation chunks and opaque progress cross this boundary.

use ring::digest::{digest, SHA256};
use ring::rand::SecureRandom;
use serde_json::Value;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use tauri::{Webview, Wry};

const MAX_CALLBACK_BYTES: usize = 64 * 1024 * 1024;
const MAX_CHUNK_BYTES: usize = 60 * 1024 * 1024;
const POLL_WAIT: Duration = Duration::from_secs(2);
const RUN_DEADLINE: Duration = Duration::from_secs(30 * 60);
const CANCEL_GRACE: Duration = Duration::from_secs(10);
const POLL_PAUSE: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserHistoryOutcome {
    pub complete: bool,
    pub error_code: Option<String>,
    pub retry_after: Option<u64>,
    pub retry_after_too_long: bool,
    pub response_content_type: Option<String>,
    pub challenge_detected: bool,
    pub provider_error_code: Option<String>,
    pub processed_conversations: u64,
    pub account_scope: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ChunkMeta {
    sequence: u64,
    owner: String,
    account_scope: String,
    conversations: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AckedChunk {
    sequence: u64,
    payload_hash: [u8; 32],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChunkAction {
    Import,
    Replay,
}

fn nonce() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "browser_history_unavailable".to_string())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn parse_callback(raw: &str) -> Result<Value, String> {
    if raw.len() > MAX_CALLBACK_BYTES {
        return Err("browser_history_result_too_large".into());
    }
    let mut value: Value =
        serde_json::from_str(raw).map_err(|_| "browser_history_invalid_result".to_string())?;
    for _ in 0..3 {
        let Some(nested) = value.as_str() else {
            break;
        };
        if nested.len() > MAX_CALLBACK_BYTES {
            return Err("browser_history_result_too_large".into());
        }
        value = serde_json::from_str(nested)
            .map_err(|_| "browser_history_invalid_result".to_string())?;
    }
    if !value.is_object() && !value.is_null() {
        return Err("browser_history_invalid_result".into());
    }
    Ok(value)
}

fn parse_bool_callback(raw: &str) -> Option<bool> {
    if raw.len() > 1024 {
        return None;
    }
    let mut value: Value = serde_json::from_str(raw).ok()?;
    for _ in 0..3 {
        let Some(nested) = value.as_str() else {
            break;
        };
        value = serde_json::from_str(nested).ok()?;
    }
    value.as_bool()
}

fn verify_chatgpt_origin(view: &Webview<Wry>) -> Result<(), String> {
    let (tx, rx) = mpsc::sync_channel(1);
    view.eval_with_callback(
        r#"JSON.stringify(location.origin === "https://chatgpt.com")"#,
        move |result| {
            let _ = tx.send(result);
        },
    )
    .map_err(|_| "browser_history_unavailable".to_string())?;
    let result = rx
        .recv_timeout(Duration::from_secs(2))
        .map_err(|_| "browser_history_unavailable".to_string())?;
    if parse_bool_callback(&result) == Some(true) {
        Ok(())
    } else {
        Err("browser_history_not_ready".into())
    }
}

fn lower_hex(value: &str, exact_len: usize) -> bool {
    value.len() == exact_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn validate_chunk(value: &Value) -> Result<ChunkMeta, String> {
    const FIELDS: &[&str] = &[
        "sequence",
        "owner",
        "account_scope",
        "format",
        "conversations",
        "project_catalog",
        "conversation_projects",
    ];
    if value
        .as_object()
        .is_none_or(|object| object.keys().any(|key| !FIELDS.contains(&key.as_str())))
    {
        return Err("browser_history_invalid_chunk".into());
    }
    let sequence = value
        .get("sequence")
        .and_then(Value::as_u64)
        .ok_or_else(|| "browser_history_invalid_chunk".to_string())?;
    let owner = value
        .get("owner")
        .and_then(Value::as_str)
        .ok_or_else(|| "browser_history_invalid_chunk".to_string())?;
    let scope = value
        .get("account_scope")
        .and_then(Value::as_str)
        .ok_or_else(|| "browser_history_invalid_chunk".to_string())?;
    let format = value
        .get("format")
        .and_then(Value::as_str)
        .ok_or_else(|| "browser_history_invalid_chunk".to_string())?;
    let conversations = value
        .get("conversations")
        .and_then(Value::as_array)
        .ok_or_else(|| "browser_history_invalid_chunk".to_string())?;
    let chunk_bytes = serde_json::to_vec(value)
        .map_err(|_| "browser_history_invalid_chunk".to_string())?
        .len();
    if !lower_hex(owner, 24)
        || !scope
            .strip_prefix("chatgpt:observed:")
            .is_some_and(|digest| lower_hex(digest, 64))
        || format != "chat-hub-history-v1"
        || (conversations.is_empty()
            && value
                .get("conversation_projects")
                .and_then(Value::as_array)
                .is_none_or(Vec::is_empty)
            && value
                .get("project_catalog")
                .and_then(Value::as_array)
                .is_none_or(Vec::is_empty))
        || conversations.len() > 1000
        || conversations
            .iter()
            .any(|conversation| !conversation.is_object())
        || chunk_bytes > MAX_CHUNK_BYTES
    {
        return Err("browser_history_invalid_chunk".into());
    }
    for field in ["project_catalog", "conversation_projects"] {
        if value.get(field).is_some_and(|v| {
            v.as_array().is_none_or(|items| {
                items.len() > 1000 || items.iter().any(|item| !item.is_object())
            })
        }) {
            return Err("browser_history_invalid_chunk".into());
        }
    }
    Ok(ChunkMeta {
        sequence,
        owner: owner.to_string(),
        account_scope: scope.to_string(),
        conversations: conversations.len(),
    })
}

fn classify_chunk(
    sequence: u64,
    expected_sequence: u64,
    last_acked: Option<&AckedChunk>,
    payload_hash: [u8; 32],
) -> Result<ChunkAction, String> {
    if sequence == expected_sequence {
        return Ok(ChunkAction::Import);
    }
    if last_acked.is_some_and(|last| last.sequence == sequence && last.payload_hash == payload_hash)
    {
        return Ok(ChunkAction::Replay);
    }
    Err("browser_history_invalid_chunk".into())
}

fn validate_status_shape(value: &Value) -> bool {
    const FIELDS: &[&str] = &[
        "status",
        "stage",
        "complete",
        "sequence",
        "conversations",
        "error_code",
        "retry_after",
        "retry_after_too_long",
        "http_status",
        "response_content_type",
        "challenge_detected",
        "provider_error_code",
        "project_count",
        "chunk",
    ];
    if !value.as_object().is_some_and(|object| {
        object.len() == FIELDS.len() && FIELDS.iter().all(|field| object.contains_key(*field))
    }) {
        return false;
    }
    matches!(
        status_str(value, "status"),
        Some("starting" | "running" | "complete" | "waiting" | "failed" | "cancelled")
    ) && matches!(
        status_str(value, "stage"),
        Some("auth" | "list" | "detail" | "project" | "complete" | "cancelled")
    ) && value.get("complete").and_then(Value::as_bool).is_some()
        && value
            .get("sequence")
            .and_then(Value::as_u64)
            .is_some_and(|count| count <= 100_000)
        && value
            .get("conversations")
            .and_then(Value::as_u64)
            .is_some_and(|count| count <= 100_000)
        && value
            .get("error_code")
            .and_then(Value::as_str)
            .is_some_and(|code| code.len() <= 64)
        && value
            .get("retry_after")
            .and_then(Value::as_u64)
            .is_some_and(|seconds| seconds <= 31_536_000)
        && value
            .get("retry_after_too_long")
            .and_then(Value::as_bool)
            .is_some_and(|too_long| {
                !too_long
                    || (status_str(value, "status") == Some("waiting")
                        && status_str(value, "error_code") == Some("rate_limited")
                        && status_u64(value, "retry_after") == 0)
            })
        && value
            .get("http_status")
            .and_then(Value::as_u64)
            .is_some_and(|status| status <= 599)
        && matches!(
            status_str(value, "response_content_type"),
            Some("json" | "html" | "other" | "missing")
        )
        && value
            .get("challenge_detected")
            .and_then(Value::as_bool)
            .is_some()
        && value
            .get("provider_error_code")
            .and_then(Value::as_str)
            .is_some_and(|code| code.is_empty() || allowed_provider_error_code(code))
        && value
            .get("project_count")
            .and_then(Value::as_u64)
            .is_some_and(|count| count <= 1000)
        && value
            .get("chunk")
            .is_some_and(|chunk| chunk.is_null() || chunk.is_object())
}

fn safe_error_code(raw: &str) -> String {
    const ALLOWED: &[&str] = &[
        "signed_out",
        "login_required",
        "auth_invalid",
        "wrong_origin",
        "rate_limited",
        "http_error",
        "invalid_request",
        "invalid_response",
        "response_too_large",
        "network_error",
        "timeout",
        "result_limit",
        "invalid_pagination",
        "page_limit",
        "conversation_too_large",
        "cancelled",
        "sync_failed",
    ];
    if ALLOWED.contains(&raw) {
        raw.to_string()
    } else {
        "sync_failed".into()
    }
}

fn allowed_provider_error_code(code: &str) -> bool {
    matches!(
        code,
        "rate_limit_exceeded"
            | "rate_limit_reached"
            | "too_many_requests"
            | "insufficient_quota"
            | "usage_limit_exceeded"
            | "model_rate_limit_exceeded"
    )
}

fn status_str<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value.get(field)?.as_str()
}

fn status_u64(value: &Value, field: &str) -> u64 {
    value.get(field).and_then(Value::as_u64).unwrap_or(0)
}

fn status_bool(value: &Value, field: &str) -> bool {
    value.get(field).and_then(Value::as_bool).unwrap_or(false)
}

fn set_cancelled(view: &Webview<Wry>, key_json: &str) {
    let _ = view.eval(&format!(
        "(()=>{{const s=window[{key_json}];if(s)s.cancelled=true}})()"
    ));
}

fn cleanup(view: &Webview<Wry>, key_json: &str) {
    let _ = view.eval(&format!("delete window[{key_json}]"));
}

fn terminal_outcome(
    value: &Value,
    processed: u64,
    account_scope: Option<String>,
) -> Option<BrowserHistoryOutcome> {
    match status_str(value, "status")? {
        "complete" => {
            let succeeded = value.get("complete").and_then(Value::as_bool) == Some(true);
            Some(BrowserHistoryOutcome {
                complete: succeeded,
                error_code: (!succeeded).then(|| "sync_failed".into()),
                retry_after: None,
                retry_after_too_long: false,
                response_content_type: None,
                challenge_detected: false,
                provider_error_code: None,
                processed_conversations: processed,
                account_scope,
            })
        }
        "failed" | "waiting" => {
            let error_code = status_str(value, "error_code")
                .map(safe_error_code)
                .unwrap_or_else(|| "sync_failed".into());
            let retry_after = status_u64(value, "retry_after");
            Some(BrowserHistoryOutcome {
                complete: false,
                error_code: Some(error_code),
                retry_after: (!status_bool(value, "retry_after_too_long") && retry_after > 0)
                    .then_some(retry_after),
                retry_after_too_long: status_bool(value, "retry_after_too_long"),
                response_content_type: Some(
                    status_str(value, "response_content_type")?.to_string(),
                ),
                challenge_detected: status_bool(value, "challenge_detected"),
                provider_error_code: status_str(value, "provider_error_code")
                    .filter(|code| !code.is_empty())
                    .map(ToOwned::to_owned),
                processed_conversations: processed,
                account_scope,
            })
        }
        "cancelled" => Some(BrowserHistoryOutcome {
            complete: false,
            error_code: Some("cancelled".into()),
            retry_after: None,
            retry_after_too_long: false,
            response_content_type: None,
            challenge_detected: false,
            provider_error_code: None,
            processed_conversations: processed,
            account_scope,
        }),
        _ => None,
    }
}

/// Runs the bundled reader in ChatGPT's own webview and synchronously hands
/// each acknowledged chunk to the caller before allowing the reader to move on.
pub fn run(
    view: Webview<Wry>,
    cancel: &AtomicBool,
    mut on_chunk: impl FnMut(&[u8], u64) -> Result<(), String>,
    mut on_progress: impl FnMut(&str, u64),
) -> Result<BrowserHistoryOutcome, String> {
    verify_chatgpt_origin(&view)?;
    let nonce = nonce()?;
    let key = format!("__chatHubHistory_{nonce}");
    let key_json = serde_json::to_string(&key).map_err(|_| "browser_history_unavailable")?;
    let worker = format!("({})({key_json})", include_str!("browser_history.js"));
    view.eval(&worker)
        .map_err(|_| "browser_history_unavailable".to_string())?;

    let deadline = Instant::now() + RUN_DEADLINE;
    let startup_deadline = Instant::now() + Duration::from_secs(10);
    let mut cancel_deadline = None;
    let mut last_stage = String::new();
    let mut last_processed = 0u64;
    let mut imported_conversations = 0u64;
    let mut expected_sequence = 1u64;
    let mut last_acked: Option<AckedChunk> = None;
    let mut chunk_owner: Option<String> = None;
    let mut account_scope: Option<String> = None;
    let mut last_debug_state: Option<(String, String, String, u64, u64, u64)> = None;
    let key_json_for_poll = key_json.clone();

    loop {
        if cancel.load(Ordering::Acquire) && cancel_deadline.is_none() {
            set_cancelled(&view, &key_json);
            cancel_deadline = Some(Instant::now() + CANCEL_GRACE);
        }
        if Instant::now() >= deadline
            || cancel_deadline.is_some_and(|limit| Instant::now() >= limit)
        {
            set_cancelled(&view, &key_json);
            cleanup(&view, &key_json);
            return Ok(BrowserHistoryOutcome {
                complete: false,
                error_code: Some(if cancel_deadline.is_some() {
                    "cancelled".into()
                } else {
                    "timeout".into()
                }),
                retry_after: None,
                retry_after_too_long: false,
                response_content_type: None,
                challenge_detected: false,
                provider_error_code: None,
                processed_conversations: imported_conversations,
                account_scope,
            });
        }

        let (tx, rx) = mpsc::sync_channel(1);
        let tx_for_callback = tx.clone();
        let poll = format!(
            r#"(()=>{{const s=window[{key_json_for_poll}];if(!s)return "null";const c=s.chunk;return JSON.stringify({{status:s.status,stage:s.stage,complete:s.complete,sequence:s.sequence,conversations:s.conversations,error_code:s.error_code,retry_after:s.retry_after,retry_after_too_long:s.retry_after_too_long,http_status:s.http_status,response_content_type:s.response_content_type,challenge_detected:s.challenge_detected,provider_error_code:s.provider_error_code,project_count:c&&Array.isArray(c.project_catalog)?c.project_catalog.length:0,chunk:c?{{sequence:c.sequence,owner:c.owner,account_scope:c.account_scope,format:c.format,conversations:c.conversations,project_catalog:c.project_catalog,conversation_projects:c.conversation_projects}}:null}})}})()"#
        );
        if view
            .eval_with_callback(poll, move |result| {
                let _ = tx_for_callback.send(result);
            })
            .is_err()
        {
            set_cancelled(&view, &key_json);
            cleanup(&view, &key_json);
            return Err("browser_history_unavailable".into());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let wait = remaining.min(POLL_WAIT);
        let raw = match rx.recv_timeout(wait) {
            Ok(raw) => raw,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                set_cancelled(&view, &key_json);
                cleanup(&view, &key_json);
                return Err("browser_history_unavailable".into());
            }
        };
        let state = match parse_callback(&raw) {
            Ok(value) => value,
            Err(error) => {
                set_cancelled(&view, &key_json);
                cleanup(&view, &key_json);
                return Err(error);
            }
        };
        if state.is_null() {
            if Instant::now() >= startup_deadline {
                cleanup(&view, &key_json);
                return Err("browser_history_unavailable".into());
            }
            std::thread::sleep(POLL_PAUSE);
            continue;
        }
        if !validate_status_shape(&state) {
            set_cancelled(&view, &key_json);
            cleanup(&view, &key_json);
            return Err("browser_history_invalid_result".into());
        }

        #[cfg(debug_assertions)]
        {
            let status = status_str(&state, "status").unwrap_or("unknown");
            let stage = status_str(&state, "stage").unwrap_or("unknown");
            let code = status_str(&state, "error_code")
                .filter(|code| !code.is_empty())
                .map(safe_error_code)
                .unwrap_or_else(|| "none".into());
            let http_status = status_u64(&state, "http_status").min(599);
            let retry_after = status_u64(&state, "retry_after").min(1800);
            let project_count = status_u64(&state, "project_count").min(1000);
            let diagnostic = (
                status.to_owned(),
                stage.to_owned(),
                code,
                http_status,
                retry_after,
                project_count,
            );
            if last_debug_state.as_ref() != Some(&diagnostic) {
                eprintln!(
                    "[chatgpt-history] status={} stage={} code={} http_status={} retry_after={} project_count={}",
                    diagnostic.0,
                    diagnostic.1,
                    diagnostic.2,
                    diagnostic.3,
                    diagnostic.4,
                    diagnostic.5
                );
                last_debug_state = Some(diagnostic);
            }
        }

        let stage = status_str(&state, "stage").unwrap_or("syncing");
        let processed = status_u64(&state, "conversations");
        if stage != last_stage || processed != last_processed {
            let safe_stage = match stage {
                "auth" | "list" | "detail" | "project" | "complete" | "cancelled" => stage,
                _ => "syncing",
            };
            on_progress(safe_stage, processed);
            last_stage = safe_stage.to_string();
            last_processed = processed;
        }

        if let Some(chunk) = state.get("chunk").filter(|chunk| !chunk.is_null()) {
            let meta = match validate_chunk(chunk) {
                Ok(meta) => meta,
                Err(error) => {
                    set_cancelled(&view, &key_json);
                    cleanup(&view, &key_json);
                    return Err(error);
                }
            };
            if status_u64(&state, "sequence") != meta.sequence {
                set_cancelled(&view, &key_json);
                cleanup(&view, &key_json);
                return Err("browser_history_invalid_chunk".into());
            }
            if chunk_owner
                .as_ref()
                .is_some_and(|owner| owner != &meta.owner)
                || account_scope
                    .as_ref()
                    .is_some_and(|scope| scope != &meta.account_scope)
            {
                set_cancelled(&view, &key_json);
                cleanup(&view, &key_json);
                return Err("browser_history_invalid_chunk".into());
            }
            let bytes = match serde_json::to_vec(chunk) {
                Ok(bytes) if bytes.len() <= MAX_CHUNK_BYTES => bytes,
                _ => {
                    set_cancelled(&view, &key_json);
                    cleanup(&view, &key_json);
                    return Err("browser_history_invalid_chunk".into());
                }
            };
            let mut payload_hash = [0u8; 32];
            payload_hash.copy_from_slice(digest(&SHA256, &bytes).as_ref());
            let action = match classify_chunk(
                meta.sequence,
                expected_sequence,
                last_acked.as_ref(),
                payload_hash,
            ) {
                Ok(action) => action,
                Err(error) => {
                    set_cancelled(&view, &key_json);
                    cleanup(&view, &key_json);
                    return Err(error);
                }
            };
            if action == ChunkAction::Replay {
                let ack = format!(
                    "(()=>{{const s=window[{key_json}];if(s&&s.chunk&&s.chunk.sequence==={}){{s.ackSequence={};s.chunk=null}}}})()",
                    meta.sequence, meta.sequence
                );
                if view.eval(&ack).is_err() {
                    set_cancelled(&view, &key_json);
                    cleanup(&view, &key_json);
                    return Err("browser_history_unavailable".into());
                }
                std::thread::sleep(POLL_PAUSE);
                continue;
            }
            if on_chunk(&bytes, meta.conversations as u64).is_err() {
                set_cancelled(&view, &key_json);
                cleanup(&view, &key_json);
                return Err("browser_history_import_failed".into());
            }
            imported_conversations =
                match imported_conversations.checked_add(meta.conversations as u64) {
                    Some(count) => count,
                    None => {
                        set_cancelled(&view, &key_json);
                        cleanup(&view, &key_json);
                        return Err("browser_history_invalid_chunk".into());
                    }
                };
            chunk_owner.get_or_insert(meta.owner);
            account_scope.get_or_insert(meta.account_scope);
            expected_sequence = match expected_sequence.checked_add(1) {
                Some(sequence) => sequence,
                None => {
                    set_cancelled(&view, &key_json);
                    cleanup(&view, &key_json);
                    return Err("browser_history_invalid_chunk".into());
                }
            };
            last_acked = Some(AckedChunk {
                sequence: meta.sequence,
                payload_hash,
            });
            let ack = format!(
                "(()=>{{const s=window[{key_json}];if(s&&s.chunk&&s.chunk.sequence==={}){{s.ackSequence={};s.chunk=null}}}})()",
                meta.sequence, meta.sequence
            );
            if view.eval(&ack).is_err() {
                set_cancelled(&view, &key_json);
                cleanup(&view, &key_json);
                return Err("browser_history_unavailable".into());
            }
            std::thread::sleep(POLL_PAUSE);
            continue;
        }

        if let Some(outcome) =
            terminal_outcome(&state, imported_conversations, account_scope.clone())
        {
            cleanup(&view, &key_json);
            return Ok(outcome);
        }
        std::thread::sleep(POLL_PAUSE);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        classify_chunk, parse_bool_callback, parse_callback, validate_chunk, validate_status_shape,
        AckedChunk, ChunkAction, MAX_CALLBACK_BYTES,
    };
    use ring::digest::{digest, SHA256};
    use serde_json::{json, Value};

    fn chunk(sequence: u64) -> Value {
        json!({
            "sequence": sequence,
            "owner": "0123456789abcdef01234567",
            "account_scope": format!("chatgpt:observed:{}", "a".repeat(64)),
            "format": "chat-hub-history-v1",
            "conversations": [{"id": "opaque-raw-graph"}]
        })
    }

    #[test]
    fn callback_decoder_handles_direct_and_nested_json_with_size_bound() {
        let direct = json!({"status":"running"}).to_string();
        assert_eq!(parse_callback(&direct).unwrap()["status"], "running");
        let once = serde_json::to_string(&direct).unwrap();
        let twice = serde_json::to_string(&once).unwrap();
        assert_eq!(parse_callback(&twice).unwrap()["status"], "running");
        assert_eq!(
            parse_callback(&"x".repeat(MAX_CALLBACK_BYTES + 1)),
            Err("browser_history_result_too_large".into())
        );
        assert!(parse_callback("not-json").is_err());
        assert_eq!(parse_bool_callback("true"), Some(true));
        assert_eq!(parse_bool_callback(r#""false""#), Some(false));
        assert_eq!(parse_bool_callback("null"), None);
    }

    #[test]
    fn chunk_validator_requires_next_sequence_and_known_identity_shape() {
        let good = chunk(3);
        let validated = validate_chunk(&good).unwrap();
        assert_eq!(validated.sequence, 3);
        assert_eq!(validated.conversations, 1);
        let mut bad_owner = chunk(1);
        bad_owner["owner"] = json!("upper0123456789abcdef0123ABC");
        assert!(validate_chunk(&bad_owner).is_err());
        let mut bad_scope = chunk(1);
        bad_scope["account_scope"] = json!("chatgpt:other:opaque");
        assert!(validate_chunk(&bad_scope).is_err());
        let mut empty = chunk(1);
        empty["conversations"] = json!([]);
        assert!(validate_chunk(&empty).is_err());
        empty["project_catalog"] = json!([{"id":"project-a","title":"Research"}]);
        assert_eq!(validate_chunk(&empty).unwrap().conversations, 0);
        empty["conversation_projects"] =
            json!([{"conversation_id":"chat-a","project_id":"project-a"}]);
        assert_eq!(validate_chunk(&empty).unwrap().conversations, 0);
        empty["conversation_projects"] = json!("not-an-array");
        assert!(validate_chunk(&empty).is_err());
        empty["conversation_projects"] = json!([{}]);
        empty["unexpected"] = json!(true);
        assert!(validate_chunk(&empty).is_err());
    }

    #[test]
    fn repeated_pending_chunk_is_reacked_without_reimport() {
        let value = chunk(1);
        let payload = serde_json::to_vec(&value).unwrap();
        let mut hash = [0u8; 32];
        hash.copy_from_slice(digest(&SHA256, &payload).as_ref());
        let last_acked = AckedChunk {
            sequence: 1,
            payload_hash: hash,
        };
        assert_eq!(
            classify_chunk(1, 2, Some(&last_acked), hash).unwrap(),
            ChunkAction::Replay
        );
        assert_eq!(
            classify_chunk(2, 2, Some(&last_acked), hash).unwrap(),
            ChunkAction::Import
        );
        let mut changed = hash;
        changed[0] ^= 1;
        assert!(classify_chunk(1, 2, Some(&last_acked), changed).is_err());
        assert!(classify_chunk(3, 2, Some(&last_acked), hash).is_err());
    }

    #[test]
    fn status_projection_accepts_only_safe_protocol_fields() {
        let valid = json!({
            "status": "running", "stage": "list", "complete": false,
            "sequence": 0, "conversations": 0, "error_code": "",
            "retry_after": 0, "retry_after_too_long": false, "http_status": 0,
            "response_content_type": "missing", "challenge_detected": false,
            "provider_error_code": "", "project_count": 0, "chunk": null
        });
        assert!(validate_status_shape(&valid));
        let mut bad_http_status = valid.clone();
        bad_http_status["http_status"] = json!(600);
        assert!(!validate_status_shape(&bad_http_status));
        let mut excessive_project_count = valid.clone();
        excessive_project_count["project_count"] = json!(1001);
        assert!(!validate_status_shape(&excessive_project_count));
        let mut with_secret = valid.clone();
        with_secret["accessToken"] = json!("must-not-cross-boundary");
        assert!(!validate_status_shape(&with_secret));
        let mut invalid_stage = valid;
        invalid_stage["stage"] = json!("provider supplied text");
        assert!(!validate_status_shape(&invalid_stage));
    }

    #[test]
    fn status_projection_bounds_provider_codes_and_long_retry_marker() {
        let mut value = json!({
            "status":"waiting", "stage":"list", "complete":false,
            "sequence":0, "conversations":0, "error_code":"rate_limited",
            "retry_after":3600, "retry_after_too_long":false, "http_status":429,
            "response_content_type":"json", "challenge_detected":false,
            "provider_error_code":"rate_limit_exceeded", "project_count":0, "chunk":null
        });
        assert!(validate_status_shape(&value));
        value["provider_error_code"] = json!("provider supplied text");
        assert!(!validate_status_shape(&value));
        value["provider_error_code"] = json!("");
        value["retry_after"] = json!(0);
        value["retry_after_too_long"] = json!(true);
        assert!(validate_status_shape(&value));
        value["retry_after"] = json!(1);
        assert!(!validate_status_shape(&value));
    }
}
