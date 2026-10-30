//! Real HTTP handler + shared Rust importer + durable Store, with no fixture
//! response backend and no CLI subprocess standing in for the import API.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use chat_hub::{importer::MAX_IMPORT_BYTES, server, store::Store};
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use tower::ServiceExt;

const EXPORT: &[u8] = include_bytes!("fixtures/chatgpt-export/array.json");
fn app() -> (PathBuf, Router) {
    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);
    let mut skipped_existing = 0;
    let dir = loop {
        let sequence = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "chat-hub-api-import-{}-{sequence}",
            std::process::id()
        ));
        match fs::create_dir(&dir) {
            Ok(()) => break dir,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                skipped_existing += 1
            }
            Err(error) => panic!("exclusive test-directory creation failed: {}", error.kind()),
        }
    };
    if skipped_existing > 0 {
        eprintln!(
            "skipped {skipped_existing} existing test directories; no existing database was reused"
        );
    }
    assert_eq!(
        fs::read_dir(&dir).unwrap().count(),
        0,
        "exclusive test directory must be empty"
    );
    let store = Store::open(dir.join("library.sqlite").to_str().unwrap()).unwrap_or_else(|error| {
        panic!("Store open failed in exclusively created empty test directory: {error}")
    });
    (dir, server::router(store, "secret".into()))
}
async fn post(
    app: &Router,
    body: impl Into<Body>,
    content_type: Option<&str>,
    auth: bool,
    length: Option<usize>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri("/api/v1/imports/chatgpt");
    if let Some(ct) = content_type {
        request = request.header("content-type", ct);
    }
    if auth {
        request = request.header("authorization", "Bearer secret");
    }
    if let Some(length) = length {
        request = request.header("content-length", length);
    }
    let response = app
        .clone()
        .oneshot(request.body(body.into()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    assert_eq!(response.headers()["content-type"], "application/json");
    let body = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}
async fn import(app: &Router, body: impl Into<Body>) -> (StatusCode, Value) {
    post(app, body, Some("application/json"), true, None).await
}
fn reopen(dir: &std::path::Path) -> Store {
    Store::open(dir.join("library.sqlite").to_str().unwrap()).unwrap()
}
fn receipt(imported: usize, updated: usize, unchanged: usize, failed: usize) -> Value {
    json!({"imported":imported,"updated":updated,"unchanged":unchanged,"failed":failed,"errors":[],"errors_truncated":false})
}

#[tokio::test]
async fn http_import_commits_reopens_and_duplicate_reimport_is_revision_noop() {
    let (dir, app) = app();
    let (status, first) = import(&app, EXPORT).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first, receipt(1, 0, 0, 0));
    let persisted = reopen(&dir);
    let revision = persisted.library_revision().unwrap();
    let snapshot = persisted.library_snapshot().unwrap();
    assert_eq!(snapshot.conversations.len(), 1);
    let conversation = &snapshot.conversations[0];
    assert!(conversation["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n["message_id"].is_null()));
    assert!(conversation["messages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|m| m["parts"].as_array().unwrap())
        .any(|p| p["kind"] == "unknown" && !p["raw"].is_null()));
    drop(persisted);
    let (status, second) = import(&app, EXPORT).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(second, receipt(0, 0, 1, 0));
    assert_eq!(reopen(&dir).library_revision().unwrap(), revision);
    let mut entries: Value = serde_json::from_slice(EXPORT).unwrap();
    entries[0]["title"] = json!("Synthetic updated HTTP title");
    entries[0]["update_time"] = json!(1900000000);
    let wrapper = serde_json::to_vec(&json!({"conversations":entries})).unwrap();
    let (status, updated) = import(&app, wrapper).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated, receipt(0, 1, 0, 0));
    assert!(reopen(&dir).library_revision().unwrap() > revision);
    drop(app);
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn global_failures_never_commit_any_items_and_errors_do_not_echo_source() {
    let (dir, app) = app();
    let too_many = serde_json::to_vec(&vec![Value::Null; 10001]).unwrap();
    for body in [
        b"not-json-private-synthetic-marker".to_vec(),
        vec![0xff],
        b"{}".to_vec(),
        b"true".to_vec(),
        b"PK synthetic zip".to_vec(),
        br#"{"path":"private-synthetic-marker"}"#.to_vec(),
        br#"{"base64":"private-synthetic-marker"}"#.to_vec(),
        too_many,
    ] {
        let (status, error) = import(&app, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(error["error"]["code"], "invalid_request");
        assert!(!error.to_string().contains("private-synthetic-marker"));
        assert_eq!(reopen(&dir).library_revision().unwrap(), 0);
    }
    drop(app);
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn http_item_failures_keep_committed_items_and_bound_error_receipt() {
    let (dir, app) = app();
    let source: Value = serde_json::from_slice(EXPORT).unwrap();
    let mixed = serde_json::to_vec(&json!([source[0],null,{"private-synthetic-marker":"invalid"}]))
        .unwrap();
    let (status, mixed) = import(&app, mixed).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(mixed["imported"], 1, "safe import receipt: {mixed}");
    assert_eq!(mixed["failed"], 2);
    assert_eq!(mixed["errors"][0]["item_index"], 1);
    assert_eq!(mixed["errors"][1]["item_index"], 2);
    assert_eq!(mixed["errors_truncated"], false);
    assert!(!mixed.to_string().contains("private-synthetic-marker"));
    let (status, truncated) =
        import(&app, serde_json::to_vec(&vec![Value::Null; 101]).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(truncated["failed"], 101);
    assert_eq!(truncated["errors"].as_array().unwrap().len(), 100);
    assert_eq!(truncated["errors_truncated"], true);
    assert_eq!(
        reopen(&dir).library_snapshot().unwrap().conversations.len(),
        1
    );
    drop(app);
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn auth_content_type_and_body_limits_are_admission_errors() {
    let (dir, app) = app();
    let (status, error) = post(&app, EXPORT, Some("application/json"), false, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(error["error"]["code"], "unauthorized");
    for ct in [None, Some("text/plain"), Some("application/zip")] {
        let (status, error) = post(&app, "[]", ct, true, None).await;
        assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(error["error"]["code"], "unsupported_media_type");
    }
    let (status, error) = post(
        &app,
        "[]",
        Some("application/json"),
        true,
        Some(MAX_IMPORT_BYTES + 1),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(error["error"]["code"], "payload_too_large");
    assert_eq!(reopen(&dir).library_revision().unwrap(), 0);
    // Proves this route has the requested 64 MiB limit, not Axum's 2 MiB
    // default. Whitespace is valid UTF-8 JSON and imports zero entries.
    let mut body = vec![b' '; 2 * 1024 * 1024 + 1];
    body.extend_from_slice(b"[]");
    let (status, result) = post(
        &app,
        body,
        Some("application/json; charset=utf-8"),
        true,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result, receipt(0, 0, 0, 0));
    drop(app);
    fs::remove_dir_all(dir).unwrap();
}
