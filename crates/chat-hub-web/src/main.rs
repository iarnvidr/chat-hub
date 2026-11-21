use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, Request, StatusCode, Uri},
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
enum WebAuth {
    Token(Arc<str>),
    TrustedProxy(Uri),
}

impl WebAuth {
    fn login_required(&self) -> bool {
        matches!(self, Self::Token(_))
    }
}

#[derive(Clone)]
struct WebState {
    auth: WebAuth,
    api_token: Arc<str>,
    sessions: Arc<Mutex<HashMap<String, Session>>>,
    api_router: Arc<Mutex<Option<Router>>>,
    capture: bool,
}

fn trusted_origin(addr: SocketAddr, origin: Option<String>) -> Result<Uri, String> {
    if !addr.ip().is_loopback() {
        return Err("CHAT_HUB_WEB_AUTH=none requires a loopback bind address".into());
    }
    let origin = origin.unwrap_or_else(|| format!("http://{addr}"));
    let uri: Uri = origin
        .parse()
        .map_err(|_| "invalid CHAT_HUB_WEB_ORIGIN".to_owned())?;
    let (Some(scheme @ ("http" | "https")), Some(host)) = (uri.scheme_str(), uri.authority())
    else {
        return Err("CHAT_HUB_WEB_ORIGIN must be an HTTP(S) origin without a path".into());
    };
    if host.as_str().contains('@') || origin != format!("{scheme}://{host}") {
        return Err("CHAT_HUB_WEB_ORIGIN must be an HTTP(S) origin without a path".into());
    }
    Ok(uri)
}

async fn restrict_trusted_origin(
    State(state): State<WebState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let WebAuth::TrustedProxy(origin) = &state.auth else {
        return next.run(request).await;
    };
    // Probes reach loopback directly. Library routes require the configured
    // host even without an Origin header, so DNS rebinding cannot read them.
    if request.uri().path() == "/healthz" {
        return next.run(request).await;
    }
    let headers = request.headers();
    let expected_host = origin.authority().unwrap().as_str();
    let expected_origin = format!("{}://{expected_host}", origin.scheme_str().unwrap());
    if headers.get(header::HOST).map(HeaderValue::as_bytes) != Some(expected_host.as_bytes())
        || headers
            .get(header::ORIGIN)
            .is_some_and(|value| value.as_bytes() != expected_origin.as_bytes())
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(request).await
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
    if !has_read_access(&state, request.headers()) {
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
    let WebAuth::Token(access_token) = &state.auth else {
        return StatusCode::NOT_FOUND.into_response();
    };
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
        || login.token.len() != access_token.len()
        || !bool::from(login.token.as_bytes().ct_eq(access_token.as_bytes()))
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
    if !state.auth.login_required() {
        return StatusCode::NOT_FOUND.into_response();
    }
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

fn has_read_access(state: &WebState, headers: &HeaderMap) -> bool {
    !state.auth.login_required()
        || cookie(headers, "chat_hub_session")
            .and_then(|sid| {
                state.sessions.lock().ok().and_then(|mut sessions| {
                    sessions.retain(|_, session| session.expires > Instant::now());
                    sessions.contains_key(&sid).then_some(())
                })
            })
            .is_some()
}

async fn capabilities(State(state): State<WebState>, headers: HeaderMap) -> Response {
    if !has_read_access(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let stable = env::var("CHAT_HUB_WEB_MODE").is_ok_and(|mode| mode == "stable");
    let service_worker =
        stable && env::var("CHAT_HUB_WEB_SERVICE_WORKER").is_ok_and(|value| value == "1");
    Json(serde_json::json!({"authenticated":true,"login_required":state.auth.login_required(),"capabilities":{"read":true,"writes":false,"catalog_write":false,"capture":state.capture,"capture_controls":false,"desktop":false,"desktop_diagnostics":false,"import_export":false,"native_file_dialog":false,"provider_view":false,"updater":false,"dev_mode":!stable,"service_worker":service_worker}})).into_response()
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
        if !search && key == "query" && value.as_str().is_some_and(|query| query.trim().is_empty())
        {
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

fn check_bridge_access(state: &WebState, headers: &HeaderMap) -> Result<(), StatusCode> {
    if !state.auth.login_required() {
        return Ok(());
    }
    let (Some(sid), Some(csrf)) = (
        cookie(headers, "chat_hub_session"),
        cookie(headers, "chat_hub_csrf"),
    ) else {
        return Err(StatusCode::UNAUTHORIZED);
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
    session_valid.then_some(()).ok_or(StatusCode::FORBIDDEN)
}

async fn bridge(
    State(state): State<WebState>,
    Path(command): Path<String>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Response {
    if let Err(status) = check_bridge_access(&state, &headers) {
        return status.into_response();
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

async fn healthz(State(state): State<WebState>) -> Json<serde_json::Value> {
    let capture = if state.capture { "enabled" } else { "disabled" };
    Json(serde_json::json!({"status":"ok", "capture":capture}))
}

/// `CHAT_HUB_WEB_CAPTURE` opts into the loopback capture proxy; anything unrecognised is an error.
fn parse_capture(value: Option<&str>) -> Result<bool, String> {
    match value {
        None | Some("0" | "false") => Ok(false),
        Some("1" | "true") => Ok(true),
        Some(_) => Err("CHAT_HUB_WEB_CAPTURE must be 1, true, 0 or false".into()),
    }
}

/// Runs the existing live-capture service (proxy + sole store writer). Its own API
/// listener binds an ephemeral loopback port with a private token and is never exposed.
async fn start_capture(
    database_path: &std::path::Path,
) -> Result<
    (
        chat_hub_runtime::service::ServiceHandle,
        tokio::sync::mpsc::Sender<chat_hub_runtime::proxy::CaptureEvent>,
    ),
    Box<dyn std::error::Error>,
> {
    let proxy_addr: SocketAddr = env::var("CHAT_HUB_PROXY_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8788".into())
        .parse()
        .map_err(|_| "invalid CHAT_HUB_PROXY_ADDR")?;
    if !proxy_addr.ip().is_loopback() {
        return Err("CHAT_HUB_PROXY_ADDR must use a loopback address".into());
    }
    use chat_hub_runtime::config::Config;
    let config = Config {
        capture_enabled: true,
        capture_profile_scope: Config::capture_profile_scope_from_env()?,
        bind_addr: "127.0.0.1:0".parse()?,
        bearer_token: random_secret()?,
        database_path: database_path
            .to_str()
            .ok_or("database path is not UTF-8")?
            .to_owned(),
        ca_dir: required_path("CHAT_HUB_CA_DIR", None)?,
        proxy_addr,
    };
    Ok(chat_hub_runtime::service::start(config, 64).await?)
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            tokio::select! {_ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {}}
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

fn required_path(name: &str, default: Option<&str>) -> Result<PathBuf, String> {
    env::var_os(name)
        .map(PathBuf::from)
        .or_else(|| default.map(PathBuf::from))
        .ok_or_else(|| format!("{name} is required"))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = env::var("CHAT_HUB_WEB_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8080".into())
        .parse()?;
    let auth = match env::var("CHAT_HUB_WEB_AUTH") {
        Ok(mode) if mode == "none" => {
            WebAuth::TrustedProxy(trusted_origin(addr, env::var("CHAT_HUB_WEB_ORIGIN").ok())?)
        }
        Ok(mode) if mode != "token" => return Err("CHAT_HUB_WEB_AUTH must be token or none".into()),
        Err(error) if error != env::VarError::NotPresent => return Err(error.into()),
        _ => {
            let token_path = required_path("CHAT_HUB_WEB_TOKEN_FILE", None)?;
            let access_token = fs::read_to_string(token_path)?.trim().to_owned();
            if access_token.is_empty() {
                return Err("web access token file is empty".into());
            }
            WebAuth::Token(Arc::from(access_token))
        }
    };
    let capture = parse_capture(env::var("CHAT_HUB_WEB_CAPTURE").ok().as_deref())?;
    let db_path = required_path("CHAT_HUB_DATABASE", None)?;
    let ui_path = required_path("CHAT_HUB_DESKTOP_UI", Some("desktop-ui"))?;
    let store = chat_hub_store::store::Store::open_existing(
        db_path.to_str().ok_or("database path is not UTF-8")?,
    )?;
    let api_token = random_secret()?;
    let state = WebState {
        auth,
        api_token: Arc::from(api_token.clone()),
        sessions: Arc::default(),
        api_router: Arc::new(Mutex::new(None)),
        capture,
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
        .layer(middleware::from_fn_with_state(
            state.clone(),
            restrict_trusted_origin,
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store, no-cache, must-revalidate"),
        ))
        .with_state(state);
    let capture_service = if capture {
        Some(start_capture(&db_path).await?)
    } else {
        None
    };
    let listener = tokio::net::TcpListener::bind(addr).await?;
    eprintln!("chat-hub-web listening on {}", listener.local_addr()?);
    if let Some((handle, _)) = &capture_service {
        if let Some(proxy) = handle.status().proxy_addr {
            eprintln!("chat-hub-web capture proxy listening on {proxy}");
        }
    }
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    if let Some((handle, _sender)) = capture_service {
        handle.shutdown().await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::to_bytes, http::Request};
    use tower::ServiceExt;

    fn test_app() -> Router {
        test_app_with_auth(WebAuth::Token(Arc::from("synthetic-access-token")))
    }

    fn test_app_with_auth(auth: WebAuth) -> Router {
        test_app_with(auth, false)
    }

    fn test_app_with(auth: WebAuth, capture: bool) -> Router {
        let state = WebState {
            auth,
            api_token: Arc::from("synthetic-api-token"),
            sessions: Arc::default(),
            api_router: Arc::new(Mutex::new(None)),
            capture,
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
            .route("/healthz", get(healthz))
            .route("/api/session", post(login).delete(logout))
            .route("/api/capabilities", get(capabilities))
            .route("/api/bridge/:command", post(bridge))
            .merge(api)
            .layer(middleware::from_fn_with_state(
                state.clone(),
                restrict_trusted_origin,
            ))
            .with_state(state)
    }

    async fn get_json(app: Router, uri: &str) -> Value {
        let response = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap()
    }

    #[test]
    fn capture_env_is_off_by_default_and_strict() {
        assert_eq!(parse_capture(None), Ok(false));
        assert_eq!(parse_capture(Some("0")), Ok(false));
        assert_eq!(parse_capture(Some("1")), Ok(true));
        for invalid in ["", "yes", "TRUE", " 1", "secret-sentinel"] {
            assert_eq!(
                parse_capture(Some(invalid)).unwrap_err(),
                "CHAT_HUB_WEB_CAPTURE must be 1, true, 0 or false"
            );
        }
    }

    #[tokio::test]
    async fn capture_flag_drives_capabilities_and_health() {
        for (capture, health) in [(false, "disabled"), (true, "enabled")] {
            let origin = "http://127.0.0.1:8080".parse().unwrap();
            let app = test_app_with(WebAuth::TrustedProxy(origin), capture);
            let caps = get_json(app.clone(), "/api/capabilities").await;
            assert_eq!(caps["capabilities"]["capture"], capture);
            assert_eq!(caps["capabilities"]["provider_view"], false);
            assert_eq!(caps["capabilities"]["writes"], false);
            let status = get_json(app, "/healthz").await;
            assert_eq!(status, serde_json::json!({"status":"ok","capture":health}));
        }
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
                    .body(Body::from(
                        r#"{"args":{"query":"","cursor":null,"source":"chatgpt","projectKey":null,"tag":null,"archiveState":"active"}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bridge_read.status(), StatusCode::OK);
        for (command, args, expected) in [
            (
                "list_conversations",
                r#"{"query":"  ","cursor":null,"source":"chatgpt","projectKey":null,"tag":null,"archiveState":"active"}"#,
                StatusCode::OK,
            ),
            (
                "list_conversations",
                r#"{"query":"synthetic query","cursor":null,"source":"chatgpt","projectKey":null,"tag":null,"archiveState":"active"}"#,
                StatusCode::OK,
            ),
            ("list_projects", r#"{"source":"chatgpt"}"#, StatusCode::OK),
            ("list_local_tags", r#"{}"#, StatusCode::OK),
            (
                "get_target_tags",
                r#"{"targetKind":"conversation","targetId":"synthetic-id"}"#,
                StatusCode::OK,
            ),
            (
                "catalog_overview",
                r#"{"source":"chatgpt","projectKey":null,"tag":null}"#,
                StatusCode::OK,
            ),
            (
                "catalog_graph",
                r#"{"source":"chatgpt","projectKey":null,"tag":null,"focusId":null,"limit":100}"#,
                StatusCode::OK,
            ),
            (
                "catalog_entities",
                r#"{"source":"chatgpt","projectKey":null,"tag":null,"kind":"file_artifact","conversationId":null,"query":null,"artifactState":"metadata_present","cursor":null,"limit":40}"#,
                StatusCode::OK,
            ),
            (
                "catalog_entity_graph",
                r#"{"source":"chatgpt","projectKey":null,"tag":null,"kind":"repository","conversationId":null,"query":null,"artifactState":null,"limit":40}"#,
                StatusCode::OK,
            ),
            (
                "get_conversation",
                r#"{"id":"missing-id"}"#,
                StatusCode::NOT_FOUND,
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
    async fn trusted_proxy_needs_loopback_and_its_configured_browser_origin() {
        for addr in ["0.0.0.0:8080", "100.64.0.1:8080", "[::]:8080"] {
            assert!(
                trusted_origin(addr.parse().unwrap(), None).is_err(),
                "{addr}"
            );
        }
        let bind = "127.0.0.1:8080".parse().unwrap();
        assert_eq!(
            trusted_origin(bind, None)
                .unwrap()
                .authority()
                .unwrap()
                .as_str(),
            "127.0.0.1:8080"
        );
        assert!(trusted_origin(bind, Some("http://preview.example.test/path".into())).is_err());
        let origin = "http://preview.example.test:5173";
        let app = test_app_with_auth(WebAuth::TrustedProxy(
            trusted_origin(bind, Some(origin.into())).unwrap(),
        ));
        let host = "preview.example.test:5173";
        for (method, path, sent_host, sent_origin, expected) in [
            ("GET", "/api/capabilities", host, origin, StatusCode::OK),
            ("GET", "/api/v1/conversations", host, origin, StatusCode::OK),
            (
                "POST",
                "/api/bridge/list_conversations",
                host,
                origin,
                StatusCode::OK,
            ),
            (
                "POST",
                "/api/bridge/rebuild_catalog_index",
                host,
                origin,
                StatusCode::FORBIDDEN,
            ),
            (
                "POST",
                "/api/v1/catalog/rebuild",
                host,
                origin,
                StatusCode::FORBIDDEN,
            ),
            (
                "DELETE",
                "/api/session",
                host,
                origin,
                StatusCode::NOT_FOUND,
            ),
            (
                "GET",
                "/api/capabilities",
                "untrusted.example.test",
                origin,
                StatusCode::FORBIDDEN,
            ),
            (
                "POST",
                "/api/bridge/list_conversations",
                host,
                "http://untrusted.example.test",
                StatusCode::FORBIDDEN,
            ),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .header(header::HOST, sent_host)
                        .header(header::ORIGIN, sent_origin)
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(r#"{"args":{}}"#))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected, "{method} {path}");
            if path == "/api/capabilities" && response.status().is_success() {
                let body = to_bytes(response.into_body(), 4096).await.unwrap();
                let capabilities: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(capabilities["login_required"], false);
                assert_eq!(capabilities["authenticated"], true);
            }
        }
    }

    #[tokio::test]
    async fn health_response_contains_only_metadata() {
        let app = test_app_with_auth(WebAuth::TrustedProxy("http://127.0.0.1:8080".parse().unwrap()));
        let response = app.oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap()).await.unwrap();
        let bytes = to_bytes(response.into_response().into_body(), 1024)
            .await
            .unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(body.contains("capture"));
        assert!(!body.contains("token"));
        assert!(!body.contains("conversation"));
    }
}
