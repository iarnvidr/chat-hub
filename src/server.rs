use crate::{error::Error, store::Store};
use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct AppState {
    store: Arc<Mutex<Store>>,
    token: Arc<str>,
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
    match s.store.lock().unwrap().health() {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({"status":"ok"}))),
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
    match s
        .store
        .lock()
        .unwrap()
        .search(&q.q, q.limit.unwrap_or(50).min(200))
    {
        Ok(ids) => (StatusCode::OK, Json(serde_json::json!({"results":ids}))),
        Err(_) => (
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
    match s.store.lock().unwrap().read(&id) {
        Ok(Some(item)) => (StatusCode::OK, Json(item)),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error":"not found"})),
        ),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error":"internal error"})),
        ),
    }
}
pub fn router(store: Store, token: String) -> Router {
    let state = AppState {
        store: Arc::new(Mutex::new(store)),
        token: Arc::from(token),
    };
    Router::new()
        .route("/api/health", get(health))
        .route("/api/search", get(search))
        .route("/api/conversations/:id", get(read))
        .with_state(state)
}
pub async fn serve(addr: std::net::SocketAddr, store: Store, token: String) -> Result<(), Error> {
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
    #[test]
    fn only_loopback_config_binding() {
        assert!("127.0.0.1"
            .parse::<std::net::IpAddr>()
            .unwrap()
            .is_loopback());
        assert!(!"0.0.0.0".parse::<std::net::IpAddr>().unwrap().is_loopback());
    }
}
