use crate::{error::Error, store::Store};
use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub mod api_v1;

#[derive(Clone)]
struct AppState {
    store: Arc<Mutex<Store>>,
    token: Arc<str>,
    db_ops: Arc<Semaphore>,
    identity: Option<RuntimeIdentity>,
}

/// Runtime identity exposed only by the opt-in authenticated health router.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeIdentity {
    pub pid: u32,
    pub instance_id: String,
}

/// Neutral lifecycle snapshot supplied by the composition owner. A healthy DB
/// does not imply a ready service; callers without a provider remain unknown.
#[derive(Clone, Copy, Debug)]
pub struct ServiceReadiness {
    pub ready: bool,
    pub code: Option<&'static str>,
}
pub type ReadinessProvider = Arc<dyn Fn() -> ServiceReadiness + Send + Sync>;
pub type CaptureStatusProvider = Arc<dyn Fn() -> crate::live_capture::Snapshot + Send + Sync>;
const MAX_DB_OPS: usize = 16;
fn overload() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({"error":"unavailable"})),
    )
}
async fn run_db<T, F>(s: &AppState, f: F) -> Result<T, ()>
where
    T: Send + 'static,
    F: FnOnce(&mut Store) -> T + Send + 'static,
{
    let permit: OwnedSemaphorePermit = s.db_ops.clone().try_acquire_owned().map_err(|_| ())?;
    let store = s.store.clone();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let mut store = store.lock().map_err(|_| ())?;
        Ok::<T, ()>(f(&mut store))
    })
    .await
    .map_err(|_| ())?
}
fn authorized(headers: &HeaderMap, token: &str) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|v| !token.is_empty() && v == token)
}
async fn health(State(s): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    if !authorized(&headers, &s.token) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error":"unauthorized"})),
        );
    }
    match run_db(&s, |store| store.health()).await {
        Ok(Ok(true)) => {
            let mut response = serde_json::json!({"status":"ok"});
            if let Some(identity) = &s.identity {
                response["pid"] = serde_json::json!(identity.pid);
                response["instance_id"] = serde_json::json!(identity.instance_id);
            }
            (StatusCode::OK, Json(response))
        }
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"status":"unavailable"})),
        ),
    }
}
#[derive(Deserialize)]
pub struct SearchQuery {
    q: String,
    limit: Option<usize>,
}
async fn search(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<SearchQuery>,
) -> impl IntoResponse {
    if !authorized(&headers, &s.token) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error":"unauthorized"})),
        );
    }
    match run_db(&s, move |store| {
        store.search(&q.q, q.limit.unwrap_or(50).min(200))
    })
    .await
    {
        Err(()) => overload(),
        Ok(Ok(ids)) => (StatusCode::OK, Json(serde_json::json!({"results":ids}))),
        Ok(Err(_)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error":"internal error"})),
        ),
    }
}
async fn read(
    State(s): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    if !authorized(&headers, &s.token) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error":"unauthorized"})),
        );
    }
    match run_db(&s, move |store| store.read(&id)).await {
        Err(()) => overload(),
        Ok(Ok(Some(item))) => (StatusCode::OK, Json(item)),
        Ok(Ok(None)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error":"not found"})),
        ),
        Ok(Err(_)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error":"internal error"})),
        ),
    }
}
pub fn router(store: Store, token: String) -> Router {
    build_router(store, token, None, None)
}

/// Builds a router whose successful authenticated health response includes the runtime identity.
/// `instance_id` must contain at most 128 bytes.
pub fn router_with_identity(store: Store, token: String, identity: RuntimeIdentity) -> Router {
    assert!(
        identity.instance_id.len() <= 128,
        "instance_id must be at most 128 bytes"
    );
    build_router(store, token, Some(identity), None)
}

/// Additive shared-service entrypoint. The provider must take only a quick
/// lifecycle snapshot; it must not perform I/O or wait for a contended lock.
pub fn router_with_readiness(
    store: Store,
    token: String,
    identity: RuntimeIdentity,
    readiness: ReadinessProvider,
) -> Router {
    assert!(
        identity.instance_id.len() <= 128,
        "instance_id must be at most 128 bytes"
    );
    build_router(store, token, Some(identity), Some(readiness))
}

pub fn router_with_capture_status(
    store: Store,
    token: String,
    identity: RuntimeIdentity,
    readiness: ReadinessProvider,
    capture: CaptureStatusProvider,
) -> Router {
    build_router_capture(store, token, Some(identity), Some(readiness), Some(capture))
}

fn build_router(
    store: Store,
    token: String,
    identity: Option<RuntimeIdentity>,
    readiness: Option<ReadinessProvider>,
) -> Router {
    build_router_capture(store, token, identity, readiness, None)
}
fn build_router_capture(
    store: Store,
    token: String,
    identity: Option<RuntimeIdentity>,
    readiness: Option<ReadinessProvider>,
    capture: Option<CaptureStatusProvider>,
) -> Router {
    let state = AppState {
        store: Arc::new(Mutex::new(store)),
        token: Arc::from(token),
        db_ops: Arc::new(Semaphore::new(MAX_DB_OPS)),
        identity,
    };
    // Share bounded Store reads and actual DB/lifecycle diagnostics with API v1.
    let reader = api_v1::store_reader(state.store.clone(), readiness);
    let v1 = api_v1::routes(
        state.token.clone(),
        Some(reader),
        state.db_ops.clone(),
        Some(state.store.clone()),
        capture,
    );
    Router::new()
        .route("/api/health", get(health))
        .route("/api/search", get(search))
        .route("/api/conversations/:id", get(read))
        .with_state(state)
        .merge(v1)
}

#[cfg(test)]
pub(crate) fn status_test_router(store: Arc<Mutex<Store>>, readiness: ReadinessProvider) -> Router {
    api_v1::routes(
        Arc::from("secret"),
        Some(api_v1::store_reader(store.clone(), Some(readiness))),
        Arc::new(Semaphore::new(MAX_DB_OPS)),
        Some(store),
        None,
    )
}
pub async fn serve(addr: std::net::SocketAddr, store: Store, token: String) -> Result<(), Error> {
    if !addr.ip().is_loopback() {
        return Err(Error::Http("server address must be loopback".into()));
    }
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router(store, token))
        .await
        .map_err(|e| Error::Http(e.to_string()))
}
#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    #[tokio::test]
    async fn bearer_auth_protects_health() {
        let app = router(Store::in_memory().unwrap(), "secret".into());
        let unauth = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);
        let auth = app
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .header(header::AUTHORIZATION, "Bearer secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(auth.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn identity_health_is_authenticated_and_includes_identity_only_on_success() {
        let app = router_with_identity(
            Store::in_memory().unwrap(),
            "secret".into(),
            RuntimeIdentity {
                pid: 42,
                instance_id: "instance-a".into(),
            },
        );
        let unauth = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);
        let body = axum::body::to_bytes(unauth.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.get("pid").is_none());
        assert!(json.get("instance_id").is_none());

        let auth = app
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .header(header::AUTHORIZATION, "Bearer secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(auth.status(), StatusCode::OK);
        let body = axum::body::to_bytes(auth.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"status":"ok", "pid":42, "instance_id":"instance-a"})
        );
    }
    #[test]
    fn only_loopback_config_binding() {
        assert!("127.0.0.1"
            .parse::<std::net::IpAddr>()
            .unwrap()
            .is_loopback());
        assert!(!"0.0.0.0".parse::<std::net::IpAddr>().unwrap().is_loopback());
    }
}
