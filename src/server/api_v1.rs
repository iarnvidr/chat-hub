//! API v1 projection and routing. ReadAdapter owns transactional snapshot reads;
//! this module owns HTTP, keyset paging, and view-only graph selection. The
//! production adapter uses bounded Store snapshots and actual DB/lifecycle facts.

use super::authorized;
use axum::{
    extract::{rejection::QueryRejection, Path, Query, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};
use tokio::sync::Semaphore;

mod imports;

/// Every record and the revision in a snapshot must come from ONE store read
/// transaction. Never implement this by reading the revision and rows separately.
pub trait ReadAdapter: Send {
    fn list_page_snapshot(
        &mut self,
        limit: usize,
        after: Option<&ListKey>,
        expected_revision: Option<i64>,
    ) -> Result<ListPageSnapshot, ReadError>;
    /// Provider-filtered chat-side listing. Existing adapters without this
    /// capability remain valid and fail closed as unavailable.
    fn chat_list_page_snapshot(
        &mut self,
        _limit: usize,
        _after: Option<&ListKey>,
        _expected_revision: Option<i64>,
    ) -> Result<ListPageSnapshot, ReadError> {
        Err(ReadError::Unavailable)
    }
    /// RAW text, not FTS grammar. The store literalizes whitespace-separated
    /// Unicode terms and ANDs them. Do not prequote or resolve summaries later.
    fn search_page_snapshot(
        &mut self,
        query: &str,
        limit: usize,
        after: Option<&SearchKey>,
        expected_revision: Option<i64>,
    ) -> Result<SearchPageSnapshot, ReadError>;
    /// Provider-filtered chat-side search, using the same raw query semantics.
    fn chat_search_page_snapshot(
        &mut self,
        _query: &str,
        _limit: usize,
        _after: Option<&SearchKey>,
        _expected_revision: Option<i64>,
    ) -> Result<SearchPageSnapshot, ReadError> {
        Err(ReadError::Unavailable)
    }
    fn detail_snapshot(&mut self, canonical_id: &str) -> Result<DetailSnapshot, ReadError>;
    /// Service readiness must be known independently of the database probe.
    fn diagnostics(&mut self) -> Result<Diagnostics, ReadError>;
    fn schema_version(&mut self) -> Result<i64, ReadError>;
}

#[derive(Clone, Debug)]
pub struct ListPageSnapshot {
    pub library_revision: i64,
    pub items: Vec<Summary>,
}
#[derive(Clone, Debug)]
pub struct SearchPageSnapshot {
    pub library_revision: i64,
    pub hits: Vec<SearchHit>,
}
#[derive(Clone, Debug)]
pub struct DetailSnapshot {
    pub library_revision: i64,
    pub conversation: Option<Conversation>,
}
#[derive(Clone, Debug)]
pub struct ListKey {
    pub updated_at_ms: Option<i64>,
    pub id: String,
}
#[derive(Clone, Debug)]
pub struct SearchKey {
    pub score: f64,
    pub conversation_id: String,
    pub node_id: Option<String>,
}
#[derive(Clone, Copy, Debug)]
pub enum ReadError {
    Unavailable,
    InvalidData,
    BudgetExceeded,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Summary {
    pub id: String,
    pub provider: String,
    pub account_scope: String,
    pub source_id: String,
    pub title: String,
    pub updated_at_ms: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Conversation {
    #[serde(flatten)]
    pub summary: Summary,
    pub source_current_node_id: Option<String>,
    pub selected_node_id: Option<String>,
    pub selection_reason: String,
    pub thread_complete: bool,
    pub nodes: Vec<Node>,
    pub messages: Vec<Message>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub source_id: String,
    pub parent_id: Option<String>,
    pub message_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub source_id: Option<String>,
    pub node_id: String,
    pub role: String,
    pub author: Option<Value>,
    pub create_time: Option<f64>,
    #[serde(default)]
    pub created_at_ms: Option<i64>,
    pub content: String,
    pub parts: Vec<Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchHit {
    pub conversation: Summary,
    pub node_id: Option<String>,
    pub message_id: Option<String>,
    pub leaf_node_id: Option<String>,
    pub snippet: String,
    pub score: f64,
    pub selected_branch: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Component {
    pub status: String,
    pub code: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Diagnostics {
    pub status: String,
    pub service: Component,
    pub database: Component,
}

type Reader = Arc<Mutex<Box<dyn ReadAdapter>>>;
struct StoreReader {
    store: Arc<Mutex<crate::store::Store>>,
    readiness: Option<super::ReadinessProvider>,
}
pub(super) fn store_reader(
    store: Arc<Mutex<crate::store::Store>>,
    readiness: Option<super::ReadinessProvider>,
) -> Reader {
    Arc::new(Mutex::new(Box::new(StoreReader { store, readiness })))
}
impl ReadAdapter for StoreReader {
    fn list_page_snapshot(
        &mut self,
        limit: usize,
        after: Option<&ListKey>,
        expected_revision: Option<i64>,
    ) -> Result<ListPageSnapshot, ReadError> {
        let after = after.map(|key| crate::store::ListKey {
            updated_at_ms: key.updated_at_ms,
            id: key.id.clone(),
        });
        let snapshot = self
            .store
            .lock()
            .map_err(|_| ReadError::Unavailable)?
            .list_page_snapshot(limit, after.as_ref(), expected_revision)
            .map_err(store_read_error)?;
        Ok(ListPageSnapshot {
            library_revision: snapshot.library_revision,
            items: snapshot
                .items
                .into_iter()
                .map(serde_json::from_value)
                .collect::<Result<_, _>>()
                .map_err(|_| ReadError::InvalidData)?,
        })
    }
    fn search_page_snapshot(
        &mut self,
        query: &str,
        limit: usize,
        after: Option<&SearchKey>,
        expected_revision: Option<i64>,
    ) -> Result<SearchPageSnapshot, ReadError> {
        let after = after.map(|key| crate::store::SearchKey {
            score: key.score,
            conversation_id: key.conversation_id.clone(),
            node_id: key.node_id.clone(),
        });
        let snapshot = self
            .store
            .lock()
            .map_err(|_| ReadError::Unavailable)?
            .search_page_snapshot(query, limit, after.as_ref(), expected_revision)
            .map_err(store_read_error)?;
        Ok(SearchPageSnapshot {
            library_revision: snapshot.library_revision,
            hits: snapshot
                .hits
                .into_iter()
                .map(serde_json::from_value)
                .collect::<Result<_, _>>()
                .map_err(|_| ReadError::InvalidData)?,
        })
    }
    fn chat_list_page_snapshot(
        &mut self,
        limit: usize,
        after: Option<&ListKey>,
        expected_revision: Option<i64>,
    ) -> Result<ListPageSnapshot, ReadError> {
        let after = after.map(|key| crate::store::ListKey {
            updated_at_ms: key.updated_at_ms,
            id: key.id.clone(),
        });
        let snapshot = self
            .store
            .lock()
            .map_err(|_| ReadError::Unavailable)?
            .chat_list_page_snapshot(limit, after.as_ref(), expected_revision)
            .map_err(store_read_error)?;
        Ok(ListPageSnapshot {
            library_revision: snapshot.library_revision,
            items: snapshot
                .items
                .into_iter()
                .map(serde_json::from_value)
                .collect::<Result<_, _>>()
                .map_err(|_| ReadError::InvalidData)?,
        })
    }
    fn chat_search_page_snapshot(
        &mut self,
        query: &str,
        limit: usize,
        after: Option<&SearchKey>,
        expected_revision: Option<i64>,
    ) -> Result<SearchPageSnapshot, ReadError> {
        let after = after.map(|key| crate::store::SearchKey {
            score: key.score,
            conversation_id: key.conversation_id.clone(),
            node_id: key.node_id.clone(),
        });
        let snapshot = self
            .store
            .lock()
            .map_err(|_| ReadError::Unavailable)?
            .chat_search_page_snapshot(query, limit, after.as_ref(), expected_revision)
            .map_err(store_read_error)?;
        Ok(SearchPageSnapshot {
            library_revision: snapshot.library_revision,
            hits: snapshot
                .hits
                .into_iter()
                .map(serde_json::from_value)
                .collect::<Result<_, _>>()
                .map_err(|_| ReadError::InvalidData)?,
        })
    }
    fn detail_snapshot(&mut self, canonical_id: &str) -> Result<DetailSnapshot, ReadError> {
        let snapshot = self
            .store
            .lock()
            .map_err(|_| ReadError::Unavailable)?
            .detail_snapshot(canonical_id)
            .map_err(store_read_error)?;
        Ok(DetailSnapshot {
            library_revision: snapshot.library_revision,
            conversation: snapshot
                .conversation
                .map(serde_json::from_value)
                .transpose()
                .map_err(|_| ReadError::InvalidData)?,
        })
    }
    fn diagnostics(&mut self) -> Result<Diagnostics, ReadError> {
        let service = self
            .readiness
            .as_ref()
            .map(|readiness| readiness())
            .unwrap_or(super::ServiceReadiness {
                ready: false,
                code: Some("readiness_unknown"),
            });
        let healthy = self
            .store
            .lock()
            .ok()
            .and_then(|store| store.health().ok())
            .unwrap_or(false);
        Ok(Diagnostics {
            status: if service.ready && healthy {
                "healthy"
            } else {
                "failed"
            }
            .into(),
            service: Component {
                status: if service.ready { "ready" } else { "failed" }.into(),
                code: service.code.map(str::to_owned),
            },
            database: Component {
                status: if healthy { "healthy" } else { "failed" }.into(),
                code: (!healthy).then(|| "database_unavailable".into()),
            },
        })
    }
    fn schema_version(&mut self) -> Result<i64, ReadError> {
        self.store
            .lock()
            .map_err(|_| ReadError::Unavailable)?
            .schema_version()
            .map_err(|_| ReadError::Unavailable)
    }
}
#[derive(Clone)]
struct ApiState {
    capture: Option<super::CaptureStatusProvider>,
    token: Arc<str>,
    reader: Option<Reader>,
    db_ops: Arc<Semaphore>,
    import_store: Option<Arc<Mutex<crate::store::Store>>>,
    import_ops: Arc<Semaphore>,
}

fn store_read_error(error: crate::error::Error) -> ReadError {
    match error {
        crate::error::Error::ReadBudgetExceeded { .. } => ReadError::BudgetExceeded,
        _ => ReadError::Unavailable,
    }
}

/// Explicit adapter injection for route tests.
pub fn router(reader: Box<dyn ReadAdapter>, token: String) -> Router {
    routes(
        Arc::from(token),
        Some(Arc::new(Mutex::new(reader))),
        Arc::new(Semaphore::new(super::MAX_DB_OPS)),
        None,
        None,
    )
}
pub(super) fn routes(
    token: Arc<str>,
    reader: Option<Reader>,
    db_ops: Arc<Semaphore>,
    import_store: Option<Arc<Mutex<crate::store::Store>>>,
    capture: Option<super::CaptureStatusProvider>,
) -> Router {
    let state = ApiState {
        token,
        reader,
        db_ops,
        import_store,
        import_ops: Arc::new(Semaphore::new(1)),
        capture,
    };
    Router::new()
        .route("/api/v1/status", get(status))
        .route("/api/v1/version", get(version))
        .route("/api/v1/conversations", get(list))
        .route("/api/v1/search", get(search))
        .route("/api/v1/chat/conversations", get(chat_list))
        .route("/api/v1/chat/search", get(chat_search))
        .route("/api/v1/conversations/:id", get(detail))
        .route("/api/v1/imports/chatgpt", post(imports::import_chatgpt))
        .route(
            "/api/v1/imports/paseo-history",
            post(imports::import_paseo_history),
        )
        // Auth runs before parameter extraction, including malformed queries.
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate))
        .with_state(state)
}
async fn authenticate(
    State(s): State<ApiState>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    if !authorized(request.headers(), &s.token) {
        return error(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Authentication required.",
        )
        .into_response();
    }
    next.run(request).await
}
type ApiResult = Result<Json<Value>, ApiError>;
#[derive(Debug)]
struct ApiError(StatusCode, &'static str, &'static str);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.0,
            Json(json!({"error":{"code":self.1,"message":self.2}})),
        )
            .into_response()
    }
}
fn error(status: StatusCode, code: &'static str, message: &'static str) -> ApiError {
    ApiError(status, code, message)
}
fn unavailable() -> ApiError {
    error(
        StatusCode::SERVICE_UNAVAILABLE,
        "service_unavailable",
        "Service is unavailable.",
    )
}
fn invalid_request() -> ApiError {
    error(
        StatusCode::BAD_REQUEST,
        "invalid_request",
        "Invalid request parameters.",
    )
}
fn invalid_cursor() -> ApiError {
    error(
        StatusCode::BAD_REQUEST,
        "invalid_cursor",
        "Cursor is invalid for this request.",
    )
}
fn invalid_data() -> ApiError {
    error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal_error",
        "Unable to read library.",
    )
}
impl From<ReadError> for ApiError {
    fn from(value: ReadError) -> Self {
        match value {
            ReadError::Unavailable => unavailable(),
            ReadError::InvalidData => invalid_data(),
            ReadError::BudgetExceeded => error(
                StatusCode::SERVICE_UNAVAILABLE,
                "response_too_large",
                "Response exceeds the read budget.",
            ),
        }
    }
}
async fn run_read<T: Send + 'static, E: Into<ApiError> + Send + 'static>(
    s: &ApiState,
    f: impl FnOnce(&mut dyn ReadAdapter) -> Result<T, E> + Send + 'static,
) -> Result<T, ApiError> {
    let permit = s
        .db_ops
        .clone()
        .try_acquire_owned()
        .map_err(|_| unavailable())?;
    let reader = s.reader.clone().ok_or_else(unavailable)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let mut reader = reader.lock().map_err(|_| unavailable())?;
        f(reader.as_mut()).map_err(Into::into)
    })
    .await
    .map_err(|_| unavailable())?
}

pub const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
struct JsonBudget(usize);
impl std::io::Write for JsonBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.0 {
            return Err(std::io::Error::other("response budget exceeded"));
        }
        self.0 -= bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn check_json_budget(value: &Value, max_bytes: usize) -> Result<(), ApiError> {
    serde_json::to_writer(JsonBudget(max_bytes), value)
        .map_err(|_| ReadError::BudgetExceeded.into())
}
fn checked_json(value: Value) -> ApiResult {
    check_json_budget(&value, MAX_RESPONSE_BYTES)?;
    Ok(Json(value))
}

async fn status(State(s): State<ApiState>) -> Response {
    match run_read(&s, |r| r.diagnostics()).await {
        Ok(mut d) => {
            // Never infer service readiness from a successful database probe.
            let healthy = d.status == "healthy"
                && d.service.status == "ready"
                && d.database.status == "healthy";
            if !healthy {
                d.status = "failed".into();
            }
            let mut body = json!(d);
            if let Some(provider) = &s.capture {
                body["capture"] = json!(provider());
            }
            (
                if healthy {
                    StatusCode::OK
                } else {
                    StatusCode::SERVICE_UNAVAILABLE
                },
                Json(body),
            )
                .into_response()
        }
        Err(error) => error.into_response(),
    }
}
async fn version(State(s): State<ApiState>) -> ApiResult {
    let schema = run_read(&s, |r| r.schema_version()).await?;
    if schema < 0 {
        return Err(invalid_data());
    }
    Ok(Json(
        json!({"binary_version":env!("CARGO_PKG_VERSION"),"api_version":"v1","schema_version":schema}),
    ))
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PageQuery {
    limit: Option<usize>,
    cursor: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchQuery {
    q: String,
    limit: Option<usize>,
    cursor: Option<String>,
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DetailQuery {
    leaf_node_id: Option<String>,
}
fn query<T>(q: Result<Query<T>, QueryRejection>) -> Result<T, ApiError> {
    q.map(|Query(q)| q).map_err(|_| invalid_request())
}
fn limit(value: Option<usize>) -> Result<usize, ApiError> {
    match value.unwrap_or(50) {
        n @ 1..=100 => Ok(n),
        _ => Err(invalid_request()),
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u8,
    route: String,
    query: Option<String>,
    order: String,
    revision: i64,
    after: Key,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum Key {
    List {
        updated_at_ms: Option<i64>,
        id: String,
    },
    Search {
        score: f64,
        conversation_id: String,
        node_id: Option<String>,
    },
}
const MAX_CURSOR_BYTES: usize = 2048;
// Bind arbitrarily long normalized queries without putting them in the token.
// This is a fixed-size fingerprint, not cursor signing/authentication.
fn query_fingerprint(query: &str) -> String {
    ring::digest::digest(&ring::digest::SHA256, query.as_bytes())
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn decode_cursor(
    value: Option<&str>,
    route: &str,
    q: Option<&str>,
) -> Result<Option<Cursor>, ApiError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_empty() || value.len() > MAX_CURSOR_BYTES || value.len() % 2 != 0 {
        return Err(invalid_cursor());
    }
    let bytes: Result<Vec<u8>, _> = value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let hex = std::str::from_utf8(pair).map_err(|_| ())?;
            u8::from_str_radix(hex, 16).map_err(|_| ())
        })
        .collect();
    let cursor: Cursor = serde_json::from_slice(&bytes.map_err(|_| invalid_cursor())?)
        .map_err(|_| invalid_cursor())?;
    if cursor.version != 1
        || cursor.route != route
        || cursor.query != q.map(query_fingerprint)
        || cursor.order != format!("{route}-v1")
        || cursor.revision < 0
    {
        return Err(invalid_cursor());
    }
    match (&cursor.after, route) {
        (Key::List { id, .. }, "list" | "chat-list") if !id.is_empty() => {}
        (
            Key::Search {
                score,
                conversation_id,
                ..
            },
            "search" | "chat-search",
        ) if score.is_finite() && !conversation_id.is_empty() => {}
        _ => return Err(invalid_cursor()),
    }
    Ok(Some(cursor))
}
fn encode_cursor(
    route: &str,
    q: Option<&str>,
    revision: i64,
    after: Key,
) -> Result<String, ApiError> {
    let cursor = Cursor {
        version: 1,
        route: route.into(),
        query: q.map(query_fingerprint),
        order: format!("{route}-v1"),
        revision,
        after,
    };
    let bytes = serde_json::to_vec(&cursor).map_err(|_| invalid_data())?;
    let encoded: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    if encoded.len() > MAX_CURSOR_BYTES {
        return Err(invalid_data());
    }
    Ok(encoded)
}
fn revision(cursor: &Option<Cursor>, current: i64) -> Result<(), ApiError> {
    if current < 0 {
        return Err(invalid_data());
    }
    if cursor.as_ref().is_some_and(|c| c.revision != current) {
        return Err(error(
            StatusCode::CONFLICT,
            "stale_cursor",
            "Library changed. Restart from the first page.",
        ));
    }
    Ok(())
}
fn compare_list(a: &Summary, b: &Summary) -> Ordering {
    b.updated_at_ms
        .cmp(&a.updated_at_ms)
        .then_with(|| a.id.cmp(&b.id))
}
fn after_list(item: &Summary, key: &Key) -> bool {
    let Key::List { updated_at_ms, id } = key else {
        return false;
    };
    updated_at_ms
        .cmp(&item.updated_at_ms)
        .then_with(|| item.id.cmp(id))
        == Ordering::Greater
}
async fn list(State(s): State<ApiState>, q: Result<Query<PageQuery>, QueryRejection>) -> ApiResult {
    list_route(State(s), q, "list", false).await
}
async fn chat_list(
    State(s): State<ApiState>,
    q: Result<Query<PageQuery>, QueryRejection>,
) -> ApiResult {
    list_route(State(s), q, "chat-list", true).await
}
async fn list_route(
    State(s): State<ApiState>,
    q: Result<Query<PageQuery>, QueryRejection>,
    route: &'static str,
    chat_only: bool,
) -> ApiResult {
    let q = query(q)?;
    let limit = limit(q.limit)?;
    let cursor = decode_cursor(q.cursor.as_deref(), route, None)?;
    let after = cursor.as_ref().and_then(|c| match &c.after {
        Key::List { updated_at_ms, id } => Some(ListKey {
            updated_at_ms: *updated_at_ms,
            id: id.clone(),
        }),
        _ => None,
    });
    let expected = cursor.as_ref().map(|c| c.revision);
    let snapshot = run_read(&s, move |r| {
        if chat_only {
            r.chat_list_page_snapshot(limit + 1, after.as_ref(), expected)
        } else {
            r.list_page_snapshot(limit + 1, after.as_ref(), expected)
        }
    })
    .await?;
    revision(&cursor, snapshot.library_revision)?;
    let mut items = snapshot.items;
    if items.len() > limit + 1
        || items
            .windows(2)
            .any(|pair| compare_list(&pair[0], &pair[1]) != Ordering::Less)
        || items
            .iter()
            .any(|item| cursor.as_ref().is_some_and(|c| !after_list(item, &c.after)))
    {
        return Err(invalid_data());
    }
    let more = items.len() > limit;
    items.truncate(limit);
    let next_cursor = if more {
        let last = items.last().ok_or_else(invalid_data)?;
        Some(encode_cursor(
            route,
            None,
            snapshot.library_revision,
            Key::List {
                updated_at_ms: last.updated_at_ms,
                id: last.id.clone(),
            },
        )?)
    } else {
        None
    };
    checked_json(
        json!({"items":items,"next_cursor":next_cursor,"library_revision":snapshot.library_revision}),
    )
}
fn compare_search(a: &SearchHit, b: &SearchHit) -> Ordering {
    a.score
        .partial_cmp(&b.score)
        .unwrap_or(Ordering::Equal)
        .then_with(|| a.conversation.id.cmp(&b.conversation.id))
        .then_with(|| a.node_id.cmp(&b.node_id))
}
fn after_search(hit: &SearchHit, key: &Key) -> bool {
    let Key::Search {
        score,
        conversation_id,
        node_id,
    } = key
    else {
        return false;
    };
    hit.score
        .partial_cmp(score)
        .unwrap_or(Ordering::Equal)
        .then_with(|| hit.conversation.id.cmp(conversation_id))
        .then_with(|| hit.node_id.cmp(node_id))
        == Ordering::Greater
}
async fn search(
    State(s): State<ApiState>,
    q: Result<Query<SearchQuery>, QueryRejection>,
) -> ApiResult {
    search_route(State(s), q, "search", false).await
}
async fn chat_search(
    State(s): State<ApiState>,
    q: Result<Query<SearchQuery>, QueryRejection>,
) -> ApiResult {
    search_route(State(s), q, "chat-search", true).await
}
async fn search_route(
    State(s): State<ApiState>,
    q: Result<Query<SearchQuery>, QueryRejection>,
    route: &'static str,
    chat_only: bool,
) -> ApiResult {
    let q = query(q)?;
    let limit = limit(q.limit)?;
    let normalized = q.q.split_whitespace().collect::<Vec<_>>().join(" ");
    if !normalized.chars().any(char::is_alphanumeric) {
        return Err(invalid_request());
    }
    let cursor = decode_cursor(q.cursor.as_deref(), route, Some(&normalized))?;
    let raw_query = normalized.clone();
    let after = cursor.as_ref().and_then(|c| match &c.after {
        Key::Search {
            score,
            conversation_id,
            node_id,
        } => Some(SearchKey {
            score: *score,
            conversation_id: conversation_id.clone(),
            node_id: node_id.clone(),
        }),
        _ => None,
    });
    let expected = cursor.as_ref().map(|c| c.revision);
    let snapshot = run_read(&s, move |r| {
        if chat_only {
            r.chat_search_page_snapshot(&raw_query, limit + 1, after.as_ref(), expected)
        } else {
            r.search_page_snapshot(&raw_query, limit + 1, after.as_ref(), expected)
        }
    })
    .await?;
    revision(&cursor, snapshot.library_revision)?;
    for h in &snapshot.hits {
        let title = h.node_id.is_none()
            && h.message_id.is_none()
            && h.leaf_node_id.is_none()
            && !h.selected_branch;
        let message = h.node_id.is_some() && h.message_id.is_some() && h.leaf_node_id.is_some();
        if !h.score.is_finite() || !(title || message) {
            return Err(invalid_data());
        }
    }
    let mut items = snapshot.hits;
    if items.len() > limit + 1
        || items
            .windows(2)
            .any(|pair| compare_search(&pair[0], &pair[1]) != Ordering::Less)
        || items.iter().any(|item| {
            cursor
                .as_ref()
                .is_some_and(|c| !after_search(item, &c.after))
        })
    {
        return Err(invalid_data());
    }
    let more = items.len() > limit;
    items.truncate(limit);
    let next_cursor = if more {
        let h = items.last().ok_or_else(invalid_data)?;
        Some(encode_cursor(
            route,
            Some(&normalized),
            snapshot.library_revision,
            Key::Search {
                score: h.score,
                conversation_id: h.conversation.id.clone(),
                node_id: h.node_id.clone(),
            },
        )?)
    } else {
        None
    };
    checked_json(
        json!({"items":items,"next_cursor":next_cursor,"library_revision":snapshot.library_revision}),
    )
}

fn detail_value(mut c: Conversation, leaf: Option<&str>, revision: i64) -> Result<Value, ApiError> {
    let nodes: HashMap<&str, &Node> = c.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    if nodes.len() != c.nodes.len() {
        return Err(invalid_data());
    }
    let parents: HashSet<&str> = c
        .nodes
        .iter()
        .filter_map(|n| n.parent_id.as_deref())
        .collect();
    let mut leaves: Vec<&Node> = c
        .nodes
        .iter()
        .filter(|n| !parents.contains(n.id.as_str()))
        .collect();
    leaves.sort_by(|a, b| a.source_id.cmp(&b.source_id).then_with(|| a.id.cmp(&b.id)));
    let (selected, reason) = if let Some(leaf) = leaf {
        let selected = leaves.iter().find(|n| n.id == leaf).ok_or_else(|| {
            error(
                StatusCode::BAD_REQUEST,
                "invalid_leaf_node_id",
                "Leaf does not belong to this conversation.",
            )
        })?;
        (Some(selected.id.clone()), "leaf_override")
    } else if let Some(source) = c
        .source_current_node_id
        .as_ref()
        .filter(|id| nodes.contains_key(id.as_str()))
    {
        (Some(source.clone()), "source_current_node")
    } else if let Some(first) = leaves.first() {
        (Some(first.id.clone()), "fallback_leaf")
    } else {
        (None, "empty")
    };
    let mut thread = Vec::new();
    let mut seen = HashSet::new();
    let mut complete = selected.is_some() || c.nodes.is_empty();
    let mut current = selected.as_deref();
    while let Some(id) = current {
        if !seen.insert(id) {
            thread.clear();
            complete = false;
            break;
        }
        let Some(node) = nodes.get(id) else {
            complete = false;
            break;
        };
        thread.push(node.id.clone());
        current = node.parent_id.as_deref();
    }
    thread.reverse();
    let branches: Vec<_> = leaves
        .iter()
        .map(|n| json!({"leaf_node_id":n.id,"selected":selected.as_ref()==Some(&n.id)}))
        .collect();
    for message in &mut c.messages {
        message.created_at_ms = match message.create_time {
            None => None,
            Some(seconds) => {
                let millis = (seconds * 1000.0).floor();
                if !millis.is_finite()
                    || millis < i64::MIN as f64
                    || millis >= 9223372036854775808.0
                {
                    return Err(invalid_data());
                }
                Some(millis as i64)
            }
        };
    }
    Ok(
        json!({"conversation":c.summary,"selection":{"source_current_node_id":c.source_current_node_id,"selected_node_id":selected,"reason":reason,"thread_complete":complete},"nodes":c.nodes,"messages":c.messages,"thread_node_ids":thread,"branches":branches,"library_revision":revision}),
    )
}
async fn detail(
    State(s): State<ApiState>,
    Path(id): Path<String>,
    q: Result<Query<DetailQuery>, QueryRejection>,
) -> ApiResult {
    let q = query(q)?;
    // Keep graph traversal and bounded encoding off the async executor, under
    // the same admission permit even if the HTTP future is cancelled.
    run_read(&s, move |r| -> ApiResult {
        let snapshot = r.detail_snapshot(&id)?;
        revision(&None, snapshot.library_revision)?;
        let conversation = snapshot.conversation.ok_or_else(|| {
            error(
                StatusCode::NOT_FOUND,
                "not_found",
                "Conversation not found.",
            )
        })?;
        if conversation.summary.id != id {
            return Err(invalid_data());
        }
        checked_json(detail_value(
            conversation,
            q.leaf_node_id.as_deref(),
            snapshot.library_revision,
        )?)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    #[test]
    fn encoded_json_budget_counts_escaping_and_never_returns_partial_data() {
        let text = json!("\"\\\n");
        let size = serde_json::to_vec(&text).unwrap().len();
        assert!(check_json_budget(&text, size).is_ok());
        let error = check_json_budget(&text, size - 1).err().unwrap();
        assert_eq!(error.0, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(error.1, "response_too_large");
        assert_eq!(
            ApiError::from(ReadError::BudgetExceeded).1,
            "response_too_large"
        );
    }

    struct BlockingReader {
        started: Option<tokio::sync::oneshot::Sender<()>>,
        release: std::sync::mpsc::Receiver<()>,
    }
    impl ReadAdapter for BlockingReader {
        fn list_page_snapshot(
            &mut self,
            _: usize,
            _: Option<&ListKey>,
            _: Option<i64>,
        ) -> Result<ListPageSnapshot, ReadError> {
            self.started.take().unwrap().send(()).unwrap();
            self.release.recv().unwrap();
            Ok(ListPageSnapshot {
                library_revision: 0,
                items: vec![],
            })
        }
        fn search_page_snapshot(
            &mut self,
            _: &str,
            _: usize,
            _: Option<&SearchKey>,
            _: Option<i64>,
        ) -> Result<SearchPageSnapshot, ReadError> {
            Err(ReadError::Unavailable)
        }
        fn detail_snapshot(&mut self, _: &str) -> Result<DetailSnapshot, ReadError> {
            Err(ReadError::Unavailable)
        }
        fn diagnostics(&mut self) -> Result<Diagnostics, ReadError> {
            Err(ReadError::Unavailable)
        }
        fn schema_version(&mut self) -> Result<i64, ReadError> {
            Err(ReadError::Unavailable)
        }
    }

    #[tokio::test]
    async fn admission_is_bounded_and_cancelled_requests_keep_blocking_permit() {
        let (started, entered) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let permits = Arc::new(Semaphore::new(1));
        let reader = BlockingReader {
            started: Some(started),
            release: wait,
        };
        let app = routes(
            Arc::from("secret"),
            Some(Arc::new(Mutex::new(Box::new(reader)))),
            permits.clone(),
            None,
            None,
        );
        let req = || {
            Request::builder()
                .uri("/api/v1/conversations")
                .header("authorization", "Bearer secret")
                .body(Body::empty())
                .unwrap()
        };
        let first = tokio::spawn(app.clone().oneshot(req()));
        entered.await.unwrap();
        // The blocking operation is running off the async executor. Cancelling
        // its HTTP future must not admit more work while it still owns the DB.
        first.abort();
        let second = app.oneshot(req()).await.unwrap();
        assert_eq!(second.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(permits.available_permits(), 0);
        release.send(()).unwrap();
        let _released = permits.acquire().await.unwrap();
    }

    #[tokio::test]
    async fn real_store_null_time_cursor_does_not_restart_known_time_rows() {
        let mut store = crate::store::Store::in_memory().unwrap();
        let docs: Value = serde_json::from_slice(include_bytes!(
            "../../tests/fixtures/chatgpt-export/array.json"
        ))
        .unwrap();
        crate::importer::import_chatgpt(&mut store, &serde_json::to_vec(&docs).unwrap()).unwrap();
        let revision = store.library_revision().unwrap();
        let token = encode_cursor(
            "list",
            None,
            revision,
            Key::List {
                updated_at_ms: None,
                id: "already-passed-null-row".into(),
            },
        )
        .unwrap();
        let app = super::super::router(store, "secret".into());
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/conversations?cursor={token}"))
                    .header("authorization", "Bearer secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), MAX_RESPONSE_BYTES)
            .await
            .unwrap();
        let page: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(page["items"], json!([]));
        assert_eq!(page["next_cursor"], Value::Null);
        assert_eq!(page["library_revision"], revision);
    }

    #[test]
    fn cursor_is_strictly_parsed_and_binds_order_and_long_unicode_query() {
        let query = "星空 ".repeat(1000);
        let token = encode_cursor(
            "search",
            Some(&query),
            7,
            Key::Search {
                score: -1.0,
                conversation_id: "cv-a".into(),
                node_id: Some("n-a".into()),
            },
        )
        .unwrap_or_else(|_| panic!("encoding failed"));
        assert!(token.len() < MAX_CURSOR_BYTES);
        assert!(decode_cursor(Some(&token), "search", Some(&query)).is_ok());
        assert!(decode_cursor(Some(&token), "search", Some("other")).is_err());
        let bytes: Vec<u8> = token
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|h| u8::from_str_radix(std::str::from_utf8(h).unwrap(), 16).unwrap())
            .collect();
        let original: Value = serde_json::from_slice(&bytes).unwrap();
        for (field, replacement) in [
            ("order", json!("search-v2")),
            ("version", json!(2)),
            ("revision", json!(-1)),
            ("unexpected", json!(true)),
            (
                "after",
                json!({"kind":"List","id":"cv-a","updated_at_ms":null}),
            ),
        ] {
            let mut malformed = original.clone();
            malformed[field] = replacement;
            let token: String = serde_json::to_vec(&malformed)
                .unwrap()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            assert!(
                decode_cursor(Some(&token), "search", Some(&query)).is_err(),
                "{field}"
            );
        }
    }
}
