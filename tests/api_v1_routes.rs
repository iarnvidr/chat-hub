//! Phase-A route tests use ONLY the injected fixture adapter. These are not
//! evidence of real Store integration, FTS execution, or production readiness.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use chat_hub::server::{
    self,
    api_v1::{
        self, Conversation, Diagnostics, LibrarySnapshot, ReadAdapter, ReadError, SearchHit,
        SearchSnapshot,
    },
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicI64, Ordering},
    Arc, Mutex,
};
use tower::ServiceExt;

const SELECTED: &str = include_str!("fixtures/api-v1/conversation-selected.json");
const ALTERNATE: &str = include_str!("fixtures/api-v1/conversation-alternate.json");
const FALLBACK: &str = include_str!("fixtures/api-v1/conversation-fallback.json");
const LIST1: &str = include_str!("fixtures/api-v1/conversations-page-1.json");
const LIST2: &str = include_str!("fixtures/api-v1/conversations-page-2.json");
const SEARCH1: &str = include_str!("fixtures/api-v1/search-page-1.json");
const SEARCH2: &str = include_str!("fixtures/api-v1/search-page-2.json");
const TITLE: &str = include_str!("fixtures/api-v1/search-title.json");
const READY: &str = include_str!("fixtures/api-v1/status-ready.json");
const FAILED: &str = include_str!("fixtures/api-v1/status-failed.json");
const DB_FAILED: &str = include_str!("fixtures/api-v1/status-database-failed.json");

fn value(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}
fn normalized(text: &str) -> Conversation {
    let d = value(text);
    let mut c = d["conversation"].clone();
    c["source_current_node_id"] = d["selection"]["source_current_node_id"].clone();
    c["selected_node_id"] = d["selection"]["selected_node_id"].clone();
    c["selection_reason"] = d["selection"]["reason"].clone();
    c["thread_complete"] = d["selection"]["thread_complete"].clone();
    c["nodes"] = d["nodes"].clone();
    c["messages"] = d["messages"].clone();
    serde_json::from_value(c).unwrap()
}
#[derive(Clone)]
struct FixtureReader {
    revision: Arc<AtomicI64>,
    queries: Arc<Mutex<Vec<String>>>,
    conversations: Vec<Conversation>,
    diagnostics: Diagnostics,
}
impl FixtureReader {
    fn new() -> Self {
        Self {
            revision: Arc::new(AtomicI64::new(7)),
            queries: Arc::new(Mutex::new(Vec::new())),
            conversations: vec![normalized(FALLBACK), normalized(SELECTED)],
            diagnostics: serde_json::from_str(READY).unwrap(),
        }
    }
    fn app(&self) -> Router {
        api_v1::router(Box::new(self.clone()), "secret".into())
    }
}
impl ReadAdapter for FixtureReader {
    fn library_snapshot(&mut self) -> Result<LibrarySnapshot, ReadError> {
        Ok(LibrarySnapshot {
            library_revision: self.revision.load(Ordering::SeqCst),
            conversations: self.conversations.clone(),
        })
    }
    fn search_snapshot(&mut self, query: &str) -> Result<SearchSnapshot, ReadError> {
        self.queries.lock().unwrap().push(query.to_owned());
        let hits: Vec<SearchHit> = match query {
            "amber telescope" => vec![
                value(SEARCH2)["items"][0].clone(),
                value(SEARCH1)["items"][0].clone(),
            ],
            "branches" => vec![value(TITLE)["items"][0].clone()],
            _ => vec![],
        }
        .into_iter()
        .map(|v| serde_json::from_value(v).unwrap())
        .collect();
        Ok(SearchSnapshot {
            library_revision: self.revision.load(Ordering::SeqCst),
            hits,
        })
    }
    fn diagnostics(&mut self) -> Result<Diagnostics, ReadError> {
        Ok(self.diagnostics.clone())
    }
    fn schema_version(&mut self) -> Result<i64, ReadError> {
        Ok(3)
    }
}
async fn request(app: &Router, uri: &str, auth: Option<&str>) -> (StatusCode, Value) {
    let mut builder = Request::builder().uri(uri);
    if let Some(auth) = auth {
        builder = builder.header("authorization", auth);
    }
    let response = app
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 1_000_000).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}
async fn get(app: &Router, uri: &str) -> (StatusCode, Value) {
    request(app, uri, Some("Bearer secret")).await
}
fn same_page(actual: &Value, expected: &str) {
    let mut expected = value(expected);
    // JSON numbers are mathematically equal across integer/float spellings.
    for item in expected["items"].as_array_mut().unwrap() {
        if let Some(score) = item.get("score") {
            item["score"] = json!(score.as_f64().unwrap());
        }
    }
    assert_eq!(actual["items"], expected["items"]);
    assert_eq!(actual["library_revision"], expected["library_revision"]);
    assert_eq!(
        actual["next_cursor"].is_null(),
        expected["next_cursor"].is_null()
    );
    if let Some(cursor) = actual["next_cursor"].as_str() {
        assert!(cursor.len() <= 2048);
    }
}

#[tokio::test]
async fn auth_precedes_query_parsing_for_every_route() {
    let app = FixtureReader::new().app();
    for uri in [
        "/api/v1/status",
        "/api/v1/version",
        "/api/v1/conversations?limit=bad",
        "/api/v1/search",
        "/api/v1/conversations/cv-a?leaf_node_id=n-b",
    ] {
        for auth in [None, Some("Bearer wrong"), Some("bearer secret")] {
            let (status, body) = request(&app, uri, auth).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
            assert_eq!(body["error"]["code"], "unauthorized");
        }
    }
}
#[tokio::test]
async fn fixture_detail_views_preserve_full_graph_and_raw_parts() {
    let app = FixtureReader::new().app();
    for (uri, fixture) in [
        ("/api/v1/conversations/cv-a", SELECTED),
        ("/api/v1/conversations/cv-a?leaf_node_id=n-b", ALTERNATE),
        ("/api/v1/conversations/cv-b", FALLBACK),
    ] {
        let (status, body) = get(&app, uri).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, value(fixture));
    }
    // View override is never written back to the reader or its revision.
    assert_eq!(
        get(&app, "/api/v1/conversations/cv-a").await.1,
        value(SELECTED)
    );
    let (status, body) = get(&app, "/api/v1/conversations/no-such-conversation").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        body,
        value(include_str!("fixtures/api-v1/error-not-found.json"))
    );
    for leaf in ["n-user", "f-a", "unknown", "msg-a"] {
        let (status, body) = get(
            &app,
            &format!("/api/v1/conversations/cv-a?leaf_node_id={leaf}"),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], "invalid_leaf_node_id");
    }
}
#[tokio::test]
async fn keyset_list_pages_tied_timestamps_and_nulls() {
    let reader = FixtureReader::new();
    let app = reader.app();
    let (status, first) = get(&app, "/api/v1/conversations?limit=1").await;
    assert_eq!(status, StatusCode::OK);
    same_page(&first, LIST1);
    let cursor = first["next_cursor"].as_str().unwrap();
    let (_, second) = get(
        &app,
        &format!("/api/v1/conversations?limit=100&cursor={cursor}"),
    )
    .await;
    same_page(&second, LIST2);
    let mut with_unknown = reader;
    let mut unknown = normalized(FALLBACK);
    unknown.summary.id = "cv-0".into();
    unknown.summary.updated_at_ms = None;
    with_unknown.conversations.push(unknown);
    let app = with_unknown.app();
    let (_, page) = get(&app, "/api/v1/conversations?limit=2").await;
    assert_eq!(page["items"][0]["id"], "cv-a");
    assert_eq!(page["items"][1]["id"], "cv-b");
    let cursor = page["next_cursor"].as_str().unwrap();
    let (_, last) = get(&app, &format!("/api/v1/conversations?cursor={cursor}")).await;
    assert_eq!(last["items"][0]["id"], "cv-0");
    assert!(last["next_cursor"].is_null());
}
#[tokio::test]
async fn search_pages_anchor_alternate_branch_and_title_hit() {
    let reader = FixtureReader::new();
    let app = reader.app();
    let (status, first) = get(&app, "/api/v1/search?q=amber%20%20telescope&limit=1").await;
    assert_eq!(status, StatusCode::OK);
    same_page(&first, SEARCH1);
    let cursor = first["next_cursor"].as_str().unwrap();
    let (_, second) = get(
        &app,
        &format!("/api/v1/search?q=amber%20telescope&cursor={cursor}"),
    )
    .await;
    same_page(&second, SEARCH2);
    let leaf = second["items"][0]["leaf_node_id"].as_str().unwrap();
    let (_, detail) = get(
        &app,
        &format!("/api/v1/conversations/cv-a?leaf_node_id={leaf}"),
    )
    .await;
    assert!(detail["thread_node_ids"]
        .as_array()
        .unwrap()
        .contains(&second["items"][0]["node_id"]));
    let (_, title) = get(&app, "/api/v1/search?q=branches").await;
    same_page(&title, TITLE);
    let (_, empty) = get(&app, "/api/v1/search?q=nonexistentterm").await;
    assert_eq!(
        empty,
        value(include_str!("fixtures/api-v1/search-empty.json"))
    );
    assert_eq!(reader.queries.lock().unwrap()[0], "amber telescope");
    // F3 passes raw text without prequoting; literal FTS execution is F1's test gate.
    get(
        &app,
        "/api/v1/search?q=title%3Afoo%20OR%20%22bar%22%20baz%2A",
    )
    .await;
    assert_eq!(
        reader.queries.lock().unwrap().last().unwrap(),
        "title:foo OR \"bar\" baz*"
    );
}
#[tokio::test]
async fn cursor_binding_validation_and_revision_conflicts() {
    let reader = FixtureReader::new();
    let app = reader.app();
    let (_, page) = get(&app, "/api/v1/conversations?limit=1").await;
    let list_cursor = page["next_cursor"].as_str().unwrap();
    let (_, search_page) = get(&app, "/api/v1/search?q=amber%20telescope&limit=1").await;
    let search_cursor = search_page["next_cursor"].as_str().unwrap();
    for uri in [
        format!("/api/v1/search?q=amber&cursor={list_cursor}"),
        format!("/api/v1/conversations?cursor={search_cursor}"),
        format!("/api/v1/search?q=branches&cursor={search_cursor}"),
        "/api/v1/conversations?cursor=invalid".into(),
        format!("/api/v1/conversations?cursor={}", "a".repeat(2050)),
        "/api/v1/conversations?cursor=".into(),
    ] {
        let (status, body) = get(&app, &uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(
            body,
            value(include_str!("fixtures/api-v1/error-invalid-cursor.json"))
        );
    }
    reader.revision.store(8, Ordering::SeqCst);
    for uri in [
        format!("/api/v1/conversations?cursor={list_cursor}"),
        format!("/api/v1/search?q=amber%20telescope&cursor={search_cursor}"),
    ] {
        let (status, body) = get(&app, &uri).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            body,
            value(include_str!("fixtures/api-v1/error-stale-cursor.json"))
        );
    }
    assert_eq!(
        get(&app, "/api/v1/conversations").await.1["library_revision"],
        8
    );
}
#[tokio::test]
async fn invalid_limits_queries_and_unknown_parameters_are_json_errors() {
    let app = FixtureReader::new().app();
    for uri in [
        "/api/v1/conversations?limit=0",
        "/api/v1/conversations?limit=101",
        "/api/v1/conversations?limit=-1",
        "/api/v1/conversations?limit=x",
        "/api/v1/conversations?limit=1&limit=2",
        "/api/v1/search",
        "/api/v1/search?q=%20%20",
        "/api/v1/search?q=%22%2A%3A",
        "/api/v1/search?q=branches&limit=101",
        "/api/v1/conversations?offset=10",
        "/api/v1/conversations/cv-a?leaf=wrong",
    ] {
        let (status, body) = get(&app, uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(body["error"]["code"], "invalid_request");
    }
    assert_eq!(
        get(&app, "/api/v1/search?q=%E6%98%9F%E7%A9%BA").await.0,
        StatusCode::OK
    );
}
#[tokio::test]
async fn status_separates_database_from_service_and_version_is_real_binary() {
    for fixture in [READY, FAILED, DB_FAILED] {
        let mut reader = FixtureReader::new();
        reader.diagnostics = serde_json::from_str(fixture).unwrap();
        let (status, body) = get(&reader.app(), "/api/v1/status").await;
        assert_eq!(
            status,
            if fixture == READY {
                StatusCode::OK
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            }
        );
        assert_eq!(body, value(fixture));
    }
    let (_, body) = get(&FixtureReader::new().app(), "/api/v1/version").await;
    assert_eq!(
        body,
        json!({"binary_version":env!("CARGO_PKG_VERSION"),"api_version":"v1","schema_version":3})
    );
}
#[tokio::test]
async fn production_router_fails_closed_and_preserves_legacy_responses() {
    let app = server::router(
        chat_hub::store::Store::in_memory().unwrap(),
        "secret".into(),
    );
    assert_eq!(
        get(&app, "/api/health").await,
        (StatusCode::OK, json!({"status":"ok"}))
    );
    assert_eq!(
        get(&app, "/api/search?q=none").await,
        (StatusCode::OK, json!({"results":[]}))
    );
    assert_eq!(
        get(&app, "/api/conversations/missing").await,
        (StatusCode::NOT_FOUND, json!({"error":"not found"}))
    );
    for uri in [
        "/api/v1/status",
        "/api/v1/version",
        "/api/v1/conversations",
        "/api/v1/search?q=foo",
        "/api/v1/conversations/missing",
    ] {
        let (status, body) = get(&app, uri).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["error"]["code"], "service_unavailable");
    }
}
#[tokio::test]
async fn graph_selection_handles_internal_null_nodes_dangling_and_cycles() {
    let mut reader = FixtureReader::new();
    reader.conversations[1].source_current_node_id = Some("n-root".into());
    let (_, body) = get(&reader.app(), "/api/v1/conversations/cv-a").await;
    assert_eq!(body["selection"]["selected_node_id"], "n-root");
    assert_eq!(body["thread_node_ids"], json!(["n-root"]));
    assert!(body["branches"]
        .as_array()
        .unwrap()
        .iter()
        .all(|b| b["selected"] == false));
    reader.conversations[1].source_current_node_id = Some("n-user".into());
    assert_eq!(
        get(&reader.app(), "/api/v1/conversations/cv-a").await.1["thread_node_ids"],
        json!(["n-root", "n-user"])
    );
    reader.conversations[1].source_current_node_id = Some("n-a".into());
    reader.conversations[1].nodes[1].parent_id = Some("missing".into());
    let (_, partial) = get(&reader.app(), "/api/v1/conversations/cv-a").await;
    assert_eq!(partial["thread_node_ids"], json!(["n-user", "n-a"]));
    assert_eq!(partial["selection"]["thread_complete"], false);
    reader.conversations[1].nodes[1].parent_id = Some("n-a".into());
    let (_, cyclic) = get(&reader.app(), "/api/v1/conversations/cv-a").await;
    assert_eq!(cyclic["thread_node_ids"], json!([]));
    assert_eq!(cyclic["selection"]["thread_complete"], false);
    reader.conversations[1].source_current_node_id = None;
    reader.conversations[1].nodes.clear();
    reader.conversations[1].messages.clear();
    let (_, empty) = get(&reader.app(), "/api/v1/conversations/cv-a").await;
    assert_eq!(
        empty["selection"],
        json!({"source_current_node_id":null,"selected_node_id":null,"reason":"empty","thread_complete":true})
    );
}
