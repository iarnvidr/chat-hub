use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use ring::rand::{SecureRandom, SystemRandom};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::HashMap,
    env, fs,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tower_http::{services::ServeDir, set_header::SetResponseHeaderLayer};

const SESSION_TTL: Duration = Duration::from_secs(12 * 60 * 60);
const MAX_SESSIONS: usize = 64;

#[derive(Clone)]
struct WebState {
    access_token: Arc<str>,
    api_token: Arc<str>,
    sessions: Arc<Mutex<HashMap<String, Session>>>,
    api_router: Arc<Mutex<Option<Router>>>,
}

struct Session {
    csrf: String,
    expires: Instant,
}

fn random_secret() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "secure random unavailable".to_owned())?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.to_owned())
}

async fn authenticate(
    State(state): State<WebState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    if *request.method() != axum::http::Method::GET || !read_path_allowed(request.uri().path()) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(sid) = cookie(request.headers(), "chat_hub_session") else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let valid = state
        .sessions
        .lock()
        .ok()
        .map(|mut sessions| {
            sessions.retain(|_, session| session.expires > Instant::now());
            sessions.contains_key(&sid)
        })
        .unwrap_or(false);
    if !valid {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if let Ok(value) = HeaderValue::from_str(&format!("Bearer {}", state.api_token)) {
        request.headers_mut().insert(header::AUTHORIZATION, value);
    }
    next.run(request).await
}

fn read_path_allowed(path: &str) -> bool {
    matches!(
        path,
        "/api/health"
            | "/api/search"
            | "/api/v1/version"
            | "/api/v1/conversations"
            | "/api/v1/projects"
            | "/api/v1/catalog/overview"
            | "/api/v1/catalog/graph"
            | "/api/v1/search"
            | "/api/v1/chat/conversations"
            | "/api/v1/chat/search"
            | "/api/v1/tags"
            | "/api/v1/tags/assignments"
            | "/api/v1/catalog/entities"
            | "/api/v1/catalog/entity-graph"
    ) || (path.starts_with("/api/conversations/") && path.len() > "/api/conversations/".len())
        || (path.starts_with("/api/v1/conversations/")
            && path.len() > "/api/v1/conversations/".len()
            && !path.ends_with("/state"))
        || (path.starts_with("/api/v1/catalog/entities/")
            && path.len() > "/api/v1/catalog/entities/".len())
}

#[derive(Deserialize)]
struct LoginRequest {
    token: String,
}

async fn login(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(login): Json<LoginRequest>,
) -> Response {
    if let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        let Some(host) = headers
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
        else {
            return StatusCode::FORBIDDEN.into_response();
        };
        let local_http = env::var("CHAT_HUB_WEB_COOKIE_SECURE")
            .map(|v| v == "0")
            .unwrap_or(false);
        let scheme = if local_http { "http" } else { "https" };
        if origin != format!("{scheme}://{host}") {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    if login.token.is_empty()
        || login.token.len() != state.access_token.len()
        || !bool::from(login.token.as_bytes().ct_eq(state.access_token.as_bytes()))
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let (Ok(sid), Ok(csrf)) = (random_secret(), random_secret()) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let Ok(mut sessions) = state.sessions.lock() else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    sessions.retain(|_, session| session.expires > Instant::now());
    if sessions.len() >= MAX_SESSIONS {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    sessions.insert(
        sid.clone(),
        Session {
            csrf: csrf.clone(),
            expires: Instant::now() + SESSION_TTL,
        },
    );
    let secure = env::var("CHAT_HUB_WEB_COOKIE_SECURE")
        .map(|v| v != "0")
        .unwrap_or(true);
    let secure_flag = if secure { "; Secure" } else { "" };
    let mut response = Json(serde_json::json!({"authenticated": true})).into_response();
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "chat_hub_session={sid}; Path=/; HttpOnly; SameSite=Strict; Max-Age=43200{secure_flag}"
        ))
        .unwrap(),
    );
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "chat_hub_csrf={csrf}; Path=/; SameSite=Strict; Max-Age=43200{secure_flag}"
        ))
        .unwrap(),
    );
    response
}

async fn logout(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let (Some(sid), Some(csrf)) = (
        cookie(&headers, "chat_hub_session"),
        cookie(&headers, "chat_hub_csrf"),
    ) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if headers.get("x-csrf-token").and_then(|v| v.to_str().ok()) != Some(csrf.as_str()) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Ok(mut sessions) = state.sessions.lock() {
        if sessions
            .get(&sid)
            .is_none_or(|session| session.csrf != csrf || session.expires <= Instant::now())
        {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        sessions.remove(&sid);
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_static("chat_hub_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0"),
    );
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_static("chat_hub_csrf=; Path=/; SameSite=Strict; Max-Age=0"),
    );
    response
}

async fn capabilities(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let valid = cookie(&headers, "chat_hub_session")
        .and_then(|sid| {
            state.sessions.lock().ok().and_then(|mut sessions| {
                sessions.retain(|_, session| session.expires > Instant::now());
                sessions.contains_key(&sid).then_some(())
            })
        })
        .is_some();
    if !valid {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let stable = env::var("CHAT_HUB_WEB_MODE").is_ok_and(|mode| mode == "stable");
    let service_worker =
        stable && env::var("CHAT_HUB_WEB_SERVICE_WORKER").is_ok_and(|value| value == "1");
    Json(serde_json::json!({"authenticated":true,"capabilities":{"read":true,"writes":false,"catalog_write":false,"capture":false,"capture_controls":false,"desktop":false,"desktop_diagnostics":false,"import_export":false,"native_file_dialog":false,"provider_view":false,"updater":false,"dev_mode":!stable,"service_worker":service_worker}})).into_response()
}

fn query_string(args: &Value, search: bool) -> Result<String, StatusCode> {
    let object = args.as_object().ok_or(StatusCode::BAD_REQUEST)?;
    let mut pairs = Vec::new();
    for (key, value) in object {
        if key == "id" {
            continue;
        }
        if value.is_null() {
            continue;
        }
        let key = if search && key == "query" {
            "q".to_owned()
        } else {
            snake_case(key)
        };
        let value = match value {
            Value::String(value) => value.clone(),
            Value::Bool(value) => value.to_string(),
            Value::Number(value) => value.to_string(),
            _ => return Err(StatusCode::BAD_REQUEST),
        };
        pairs.push(format!(
            "{}={}",
            encode_query_component(&key),
            encode_query_component(&value)
        ));
    }
    Ok(pairs.join("&"))
}

fn path_segment(value: &str) -> Option<String> {
    if value.is_empty()
        || value.len() > 1024
        || value.chars().any(char::is_control)
        || value
            .bytes()
            .any(|byte| matches!(byte, b'/' | b'?' | b'#' | b'%' | b'\\'))
    {
        return None;
    }
    Some(encode_query_component(value))
}

fn encode_query_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push_str(&format!("{byte:02X}"));
        }
    }
    encoded
}

fn snake_case(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_ascii_uppercase() {
            result.push('_');
            result.push(character.to_ascii_lowercase());
        } else {
            result.push(character);
        }
    }
    result
}

async fn bridge(
    State(state): State<WebState>,
    Path(command): Path<String>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Response {
    let (Some(sid), Some(csrf)) = (
        cookie(&headers, "chat_hub_session"),
        cookie(&headers, "chat_hub_csrf"),
    ) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let session_valid = state
        .sessions
        .lock()
        .ok()
        .and_then(|mut sessions| {
            sessions.retain(|_, session| session.expires > Instant::now());
            sessions.get(&sid).map(|session| {
                session.csrf == csrf
                    && headers.get("x-csrf-token").and_then(|v| v.to_str().ok())
                        == Some(csrf.as_str())
            })
        })
        .unwrap_or(false);
    if !session_valid {
        return StatusCode::FORBIDDEN.into_response();
    }
    let args = match payload.get("args") {
        Some(args) => args,
        None => return StatusCode::BAD_REQUEST.into_response(),
    };
    let (path, search) = match command.as_str() {
        "list_conversations" => {
            let is_search = args
                .get("query")
                .and_then(Value::as_str)
                .is_some_and(|query| !query.trim().is_empty());
            (
                if is_search {
                    "/api/v1/search"
                } else {
                    "/api/v1/conversations"
                },
                is_search,
            )
        }
        "get_conversation" => {
            let Some(id) = args
                .get("id")
                .and_then(Value::as_str)
                .and_then(path_segment)
            else {
                return StatusCode::BAD_REQUEST.into_response();
            };
            return dispatch_api(&state, &format!("/api/v1/conversations/{id}"), args, false).await;
        }
        "list_projects" => ("/api/v1/projects", false),
        "list_local_tags" => {
            let mut args = args.clone();
            if args.get("limit").is_none() {
                args["limit"] = Value::from(200);
            }
            return dispatch_api(&state, "/api/v1/tags", &args, false).await;
        }
        "get_target_tags" => ("/api/v1/tags/assignments", false),
        "catalog_overview" => ("/api/v1/catalog/overview", false),
        "catalog_graph" => ("/api/v1/catalog/graph", false),
        "catalog_entities" => ("/api/v1/catalog/entities", false),
        "catalog_entity_graph" => {
            let mut args = args.clone();
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(40)
                .clamp(1, 40);
            args["limit"] = Value::from(limit);
            return dispatch_api(&state, "/api/v1/catalog/entity-graph", &args, false).await;
        }
        "catalog_entity" => {
            let Some(id) = args
                .get("id")
                .and_then(Value::as_str)
                .and_then(path_segment)
            else {
                return StatusCode::BAD_REQUEST.into_response();
            };
            return dispatch_api(
                &state,
                &format!("/api/v1/catalog/entities/{id}"),
                args,
                false,
            )
            .await;
        }
        _ => return StatusCode::FORBIDDEN.into_response(),
    };
    dispatch_api(&state, path, args, search).await
}

async fn dispatch_api(state: &WebState, path: &str, args: &Value, search: bool) -> Response {
    let query = match query_string(args, search) {
        Ok(query) => query,
        Err(status) => return status.into_response(),
    };
    let uri = if query.is_empty() {
        path.to_owned()
    } else {
        format!("{path}?{query}")
    };
    let request = match Request::builder()
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {}", state.api_token))
        .body(Body::empty())
    {
        Ok(request) => request,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let router = match state
        .api_router
        .lock()
        .ok()
        .and_then(|router| router.clone())
    {
        Some(router) => router,
        None => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    match tower::ServiceExt::oneshot(router, request).await {
        Ok(response) => normalize_api_error(response).await,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn normalize_api_error(response: Response) -> Response {
    if response.status().is_success() {
        return response;
    }
    let status = response.status();
    let Ok(bytes) = axum::body::to_bytes(response.into_body(), 4096).await else {
        return status.into_response();
    };
    let code = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|body| {
            body.pointer("/error/code")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "service_unavailable".to_owned());
    (status, Json(serde_json::json!({"code":code}))).into_response()
}

async fn healthz() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status":"ok", "capture":"disabled"}))
}

fn required_path(name: &str, default: Option<&str>) -> Result<PathBuf, String> {
    env::var_os(name)
        .map(PathBuf::from)
        .or_else(|| default.map(PathBuf::from))
        .ok_or_else(|| format!("{name} is required"))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db_path = required_path("CHAT_HUB_DATABASE", None)?;
    let token_path = required_path("CHAT_HUB_WEB_TOKEN_FILE", None)?;
    let ui_path = required_path("CHAT_HUB_DESKTOP_UI", Some("desktop-ui"))?;
    let access_token = fs::read_to_string(token_path)?.trim().to_owned();
    if access_token.is_empty() {
        return Err("web access token file is empty".into());
    }
    let store = chat_hub_store::store::Store::open_existing(
        db_path.to_str().ok_or("database path is not UTF-8")?,
    )?;
    let api_token = random_secret()?;
    let state = WebState {
        access_token: Arc::from(access_token),
        api_token: Arc::from(api_token.clone()),
        sessions: Arc::default(),
        api_router: Arc::new(Mutex::new(None)),
    };
    let api_base = chat_hub_api::server::router(store, api_token);
    if let Ok(mut router) = state.api_router.lock() {
        *router = Some(api_base.clone());
    }
    let api = api_base
        .layer(middleware::from_fn_with_state(state.clone(), authenticate))
        .with_state(());
    let static_files = ServeDir::new(ui_path).append_index_html_on_directories(true);
    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/api/session", post(login).delete(logout))
        .route("/api/capabilities", get(capabilities))
        .route("/api/bridge/:command", post(bridge))
        .merge(api)
        .fallback_service(static_files)
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store, no-cache, must-revalidate"),
        ))
        .with_state(state);
    let addr: SocketAddr = env::var("CHAT_HUB_WEB_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8080".into())
        .parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    eprintln!("chat-hub-web listening on {}", listener.local_addr()?);
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::to_bytes, http::Request};
    use tower::ServiceExt;

    fn test_app() -> Router {
        let state = WebState {
            access_token: Arc::from("synthetic-access-token"),
            api_token: Arc::from("synthetic-api-token"),
            sessions: Arc::default(),
            api_router: Arc::new(Mutex::new(None)),
        };
        let api_base = chat_hub_api::server::router(
            chat_hub_store::store::Store::in_memory().unwrap(),
            "synthetic-api-token".into(),
        );
        if let Ok(mut router) = state.api_router.lock() {
            *router = Some(api_base.clone());
        }
        let api = api_base
            .layer(middleware::from_fn_with_state(state.clone(), authenticate))
            .with_state(());
        Router::new()
            .route("/api/session", post(login).delete(logout))
            .route("/api/capabilities", get(capabilities))
            .route("/api/bridge/:command", post(bridge))
            .merge(api)
            .with_state(state)
    }

    async fn bridge_call(
        app: Router,
        command: &str,
        cookie: &str,
        csrf: &str,
        args: &str,
    ) -> Response {
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/bridge/{command}"))
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!("{{\"args\":{args}}}")))
                .unwrap(),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn synthetic_token_exchanges_for_protected_revocable_session() {
        let app = test_app();
        let denied = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
        let bearer_without_session = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .header(header::AUTHORIZATION, "Bearer synthetic-api-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bearer_without_session.status(), StatusCode::UNAUTHORIZED);

        let bad_login = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/session")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"token":"wrong"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bad_login.status(), StatusCode::UNAUTHORIZED);

        let cross_origin_login = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/session")
                    .header(header::HOST, "chat.example.test")
                    .header(header::ORIGIN, "https://attacker.example")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"token":"synthetic-access-token"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cross_origin_login.status(), StatusCode::FORBIDDEN);

        let login_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/session")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"token":"synthetic-access-token"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(login_response.status(), StatusCode::OK);
        let cookies = login_response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .collect::<Vec<_>>();
        let session = cookies[0].split(';').next().unwrap();
        let csrf = cookies[1]
            .split(';')
            .next()
            .unwrap()
            .split_once('=')
            .unwrap()
            .1;
        let cookie_header = format!("{}; {}", session, cookies[1].split(';').next().unwrap());

        let read = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .header(header::COOKIE, &cookie_header)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(read.status(), StatusCode::OK);

        let capabilities = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/capabilities")
                    .header(header::COOKIE, &cookie_header)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(capabilities.status(), StatusCode::OK);
        let bridge_read = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/bridge/list_conversations")
                    .header(header::COOKIE, &cookie_header)
                    .header("x-csrf-token", csrf)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"args":{"limit":10}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bridge_read.status(), StatusCode::OK);
        for (command, args, expected) in [
            (
                "list_conversations",
                r#"{"query":"synthetic query"}"#,
                StatusCode::OK,
            ),
            ("list_projects", r#"{"source":"all"}"#, StatusCode::OK),
            ("list_local_tags", r#"{}"#, StatusCode::OK),
            (
                "get_target_tags",
                r#"{"targetKind":"conversation","targetId":"synthetic-id"}"#,
                StatusCode::OK,
            ),
            ("catalog_overview", r#"{"source":"all"}"#, StatusCode::OK),
            (
                "catalog_graph",
                r#"{"source":"all","limit":40}"#,
                StatusCode::OK,
            ),
            (
                "catalog_entities",
                r#"{"source":"all","kind":"repository","limit":40}"#,
                StatusCode::OK,
            ),
            (
                "catalog_entity_graph",
                r#"{"source":"all","kind":"repository","limit":80}"#,
                StatusCode::OK,
            ),
            (
                "get_conversation",
                r#"{"id":"missing-id","leafNodeId":"synthetic-leaf"}"#,
                StatusCode::NOT_FOUND,
            ),
            (
                "catalog_entity",
                r#"{"id":"missing-entity"}"#,
                StatusCode::NOT_FOUND,
            ),
        ] {
            let response = bridge_call(app.clone(), command, &cookie_header, csrf, args).await;
            assert_eq!(response.status(), expected, "command {command}");
            let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap();
            if expected.is_success() {
                let value: Value = serde_json::from_slice(&body).unwrap();
                let required: &[&str] = match command {
                    "list_conversations" => &["items", "next_cursor", "library_revision"],
                    "list_projects" => &["projects", "library_revision"],
                    "list_local_tags" => &["items"],
                    "get_target_tags" => &["target_kind", "target_id", "tags"],
                    "catalog_overview" => &["index", "stats"],
                    "catalog_graph" => &["index", "stats", "nodes", "edges"],
                    "catalog_entities" => &["index", "source", "items", "limit"],
                    "catalog_entity_graph" => &["index", "source", "nodes", "edges"],
                    _ => &[],
                };
                for key in required {
                    assert!(value.get(key).is_some(), "{command} missing {key}");
                }
            }
        }
        for bad_id in ["bad?query", "bad#fragment", "bad%escape", "bad/segment"] {
            let args = serde_json::json!({"id":bad_id}).to_string();
            let response =
                bridge_call(app.clone(), "get_conversation", &cookie_header, csrf, &args).await;
            assert_eq!(
                response.status(),
                StatusCode::BAD_REQUEST,
                "unsafe id {bad_id}"
            );
        }
        let blocked_write = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/catalog/rebuild")
                    .header(header::COOKIE, &cookie_header)
                    .header("x-csrf-token", csrf)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(blocked_write.status(), StatusCode::FORBIDDEN);
        let csrf_denied_bridge = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/bridge/list_conversations")
                    .header(header::COOKIE, &cookie_header)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"args":{}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(csrf_denied_bridge.status(), StatusCode::FORBIDDEN);

        let no_csrf = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/session")
                    .header(header::COOKIE, &cookie_header)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(no_csrf.status(), StatusCode::FORBIDDEN);
        let logout = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/session")
                    .header(header::COOKIE, &cookie_header)
                    .header("x-csrf-token", csrf)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(logout.status(), StatusCode::NO_CONTENT);
        let revoked = app
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .header(header::COOKIE, cookie_header)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn health_response_contains_only_metadata() {
        let response = healthz().await;
        let bytes = to_bytes(response.into_response().into_body(), 1024)
            .await
            .unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(body.contains("capture"));
        assert!(!body.contains("token"));
        assert!(!body.contains("conversation"));
    }
}
