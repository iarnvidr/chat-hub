//! Server-mode prompt sending and live conversation stream.
//!
//! `GET /api/live/stream` relays text deltas the capture proxy observed (never
//! pixels). `POST /api/live/prompt` forwards a prompt to the headless browser
//! sidecar over loopback HTTP with a shared bearer secret; the PWA never talks
//! to the sidecar. Prompt text is forwarded and never logged or stored here.
use crate::{check_bridge_access, has_read_access, WebState};
use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use chat_hub_runtime::live_capture::LiveEvents;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{convert::Infallible, env, fs, net::SocketAddr, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::broadcast::error::RecvError,
};

const MAX_PROMPT_BYTES: usize = 32 * 1024;
const MAX_SIDECAR_RESPONSE: u64 = 64 * 1024;
const SIDECAR_TIMEOUT: Duration = Duration::from_secs(10);
const KEEPALIVE: Duration = Duration::from_secs(15);
const DEFAULT_BROWSER_URL: &str = "http://127.0.0.1:8790";

pub struct Live {
    events: LiveEvents,
    addr: SocketAddr,
    secret: String,
}

impl Live {
    pub fn new(events: LiveEvents, addr: SocketAddr, secret: &str) -> Result<Self, String> {
        let secret = secret.trim();
        if secret.len() < 16 || secret.len() > 256 || !secret.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(
                "CHAT_HUB_BROWSER_SECRET_FILE must hold 16..=256 printable characters".into(),
            );
        }
        Ok(Self {
            events,
            addr,
            secret: secret.to_owned(),
        })
    }

    /// Live view is configured by `CHAT_HUB_BROWSER_SECRET_FILE`; the sidecar URL defaults to loopback.
    pub fn from_env(events: LiveEvents) -> Result<Option<Self>, Box<dyn std::error::Error>> {
        let Some(path) = env::var_os("CHAT_HUB_BROWSER_SECRET_FILE") else {
            return Ok(None);
        };
        let url = env::var("CHAT_HUB_BROWSER_URL").unwrap_or_else(|_| DEFAULT_BROWSER_URL.into());
        let secret = fs::read_to_string(path)?;
        Ok(Some(Self::new(events, parse_browser_url(&url)?, &secret)?))
    }

    async fn request(&self, method: &str, path: &str, body: &[u8]) -> Result<(u16, Vec<u8>), ()> {
        tokio::time::timeout(SIDECAR_TIMEOUT, self.exchange(method, path, body))
            .await
            .map_err(|_| ())?
    }

    /// One HTTP/1.1 request with `Connection: close`; the sidecar answers with Content-Length.
    async fn exchange(&self, method: &str, path: &str, body: &[u8]) -> Result<(u16, Vec<u8>), ()> {
        let mut stream = TcpStream::connect(self.addr).await.map_err(|_| ())?;
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.addr,
            self.secret,
            body.len()
        );
        stream.write_all(head.as_bytes()).await.map_err(|_| ())?;
        stream.write_all(body).await.map_err(|_| ())?;
        let mut response = Vec::new();
        stream
            .take(MAX_SIDECAR_RESPONSE)
            .read_to_end(&mut response)
            .await
            .map_err(|_| ())?;
        parse_response(&response)
    }
}

fn parse_browser_url(value: &str) -> Result<SocketAddr, String> {
    let addr: SocketAddr = value
        .strip_prefix("http://")
        .map(|rest| rest.trim_end_matches('/'))
        .and_then(|rest| rest.parse().ok())
        .ok_or("CHAT_HUB_BROWSER_URL must be http://<loopback IP>:<port>")?;
    if !addr.ip().is_loopback() {
        return Err("CHAT_HUB_BROWSER_URL must use a loopback address".into());
    }
    Ok(addr)
}

fn parse_response(bytes: &[u8]) -> Result<(u16, Vec<u8>), ()> {
    let split = bytes.windows(4).position(|w| w == b"\r\n\r\n").ok_or(())?;
    let head = std::str::from_utf8(&bytes[..split]).map_err(|_| ())?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .ok_or(())?;
    Ok((status, bytes[split + 4..].to_vec()))
}

fn valid_conversation_id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PromptRequest {
    conversation_id: Option<String>,
    text: String,
}

fn valid_prompt(request: &PromptRequest) -> bool {
    request
        .conversation_id
        .as_deref()
        .is_none_or(valid_conversation_id)
        && !request.text.trim().is_empty()
        && request.text.len() <= MAX_PROMPT_BYTES
}

fn code(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({ "code": code }))).into_response()
}

pub async fn stream(State(state): State<WebState>, headers: HeaderMap) -> Response {
    if !has_read_access(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(live) = &state.live else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let events = futures_util::stream::unfold(
        (live.events.subscribe(), true),
        |(mut rx, first)| async move {
            let frame = if first {
                ": connected\n\n".to_owned()
            } else {
                match tokio::time::timeout(KEEPALIVE, rx.recv()).await {
                    Err(_) => ": keepalive\n\n".to_owned(),
                    Ok(Ok(event)) => format!(
                        "event: live\ndata: {}\n\n",
                        serde_json::to_string(&event).unwrap_or_default()
                    ),
                    // A slow client never makes the server buffer: it resynchronises from the library.
                    Ok(Err(RecvError::Lagged(_))) => "event: resync\ndata: {}\n\n".to_owned(),
                    Ok(Err(RecvError::Closed)) => return None,
                }
            };
            Some((Ok::<_, Infallible>(Bytes::from(frame)), (rx, false)))
        },
    );
    let mut response = Response::new(Body::from_stream(events));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
}

pub async fn status(State(state): State<WebState>, headers: HeaderMap) -> Response {
    if !has_read_access(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(live) = &state.live else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let name = match live.request("GET", "/status", b"").await {
        Ok((200, body)) => serde_json::from_slice::<Value>(&body)
            .ok()
            .and_then(|value| value.get("live").and_then(Value::as_str).map(str::to_owned))
            .filter(|name| matches!(name.as_str(), "idle" | "busy" | "needs_sign_in" | "error"))
            .unwrap_or_else(|| "error".into()),
        _ => "offline".into(),
    };
    Json(json!({ "state": name })).into_response()
}

pub async fn prompt(State(state): State<WebState>, headers: HeaderMap, body: Bytes) -> Response {
    // Same session/CSRF check as the bridge; the origin middleware already ran.
    if let Err(status) = check_bridge_access(&state, &headers) {
        return status.into_response();
    }
    let Some(live) = &state.live else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(request) = serde_json::from_slice::<PromptRequest>(&body) else {
        return code(StatusCode::BAD_REQUEST, "invalid_request");
    };
    if !valid_prompt(&request) {
        return code(StatusCode::BAD_REQUEST, "invalid_request");
    }
    let payload = json!({ "conversation_id": request.conversation_id, "text": request.text });
    match live
        .request("POST", "/prompt", payload.to_string().as_bytes())
        .await
    {
        Ok((200 | 202, body)) => {
            let job_id = serde_json::from_slice::<Value>(&body)
                .ok()
                .and_then(|value| {
                    value
                        .get("job_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .filter(|id| {
                    !id.is_empty()
                        && id.len() <= 64
                        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                });
            match job_id {
                Some(job_id) => (
                    StatusCode::ACCEPTED,
                    Json(json!({ "accepted": true, "job_id": job_id })),
                )
                    .into_response(),
                None => code(StatusCode::BAD_GATEWAY, "browser_unavailable"),
            }
        }
        Ok((409, _)) => code(StatusCode::CONFLICT, "busy"),
        _ => code(StatusCode::BAD_GATEWAY, "browser_unavailable"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::test_app_state;
    use crate::{authelia_origin, trusted_origin, WebAuth};
    use axum::{body::to_bytes, extract::connect_info::ConnectInfo, http::Request, Router};
    use chat_hub_runtime::live_capture::{LiveEvent, LiveEventKind};
    use futures_util::StreamExt;
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;
    use tower::ServiceExt;

    const SECRET: &str = "synthetic-sidecar-secret";
    const TEXT: &str = "synthetic prompt text";
    const CONVERSATION: &str = "0a1b2c3d-0000-4000-8000-0123456789ab";

    /// Loopback stand-in for the sidecar. Records each raw request it receives.
    async fn fake_sidecar(
        status: &'static str,
        body: &'static str,
    ) -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = seen.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut raw = Vec::new();
                let mut buffer = [0u8; 4096];
                loop {
                    let n = socket.read(&mut buffer).await.unwrap_or(0);
                    raw.extend_from_slice(&buffer[..n]);
                    let text = String::from_utf8_lossy(&raw).into_owned();
                    let complete = text.split_once("\r\n\r\n").is_some_and(|(head, rest)| {
                        let length = head
                            .lines()
                            .find_map(|line| line.strip_prefix("Content-Length: "))
                            .and_then(|value| value.parse::<usize>().ok())
                            .unwrap_or(0);
                        rest.len() >= length
                    });
                    if n == 0 || complete {
                        break;
                    }
                }
                record
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&raw).into_owned());
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        (addr, seen)
    }

    fn live(events: &LiveEvents, addr: SocketAddr) -> Option<Arc<Live>> {
        Some(Arc::new(Live::new(events.clone(), addr, SECRET).unwrap()))
    }

    fn prompt_body() -> String {
        json!({ "conversation_id": CONVERSATION, "text": TEXT }).to_string()
    }

    fn request(
        method: &str,
        path: &str,
        host: &str,
        origin: Option<&str>,
        body: &str,
    ) -> Request<Body> {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, host)
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(origin) = origin {
            builder = builder.header(header::ORIGIN, origin);
        }
        builder.body(Body::from(body.to_owned())).unwrap()
    }

    async fn status_of(app: &Router, request: Request<Body>) -> StatusCode {
        app.clone().oneshot(request).await.unwrap().status()
    }

    #[test]
    fn prompt_conversation_and_endpoint_validation() {
        assert!(valid_conversation_id(CONVERSATION));
        for bad in [
            "",
            "abc",
            "../../etc/passwd",
            "0a1b2c3d-0000-4000-8000-0123456789a?",
            "0a1b2c3d_0000_4000_8000_0123456789ab",
        ] {
            assert!(!valid_conversation_id(bad), "{bad}");
        }
        let prompt = |id: Option<&str>, text: &str| PromptRequest {
            conversation_id: id.map(str::to_owned),
            text: text.to_owned(),
        };
        assert!(valid_prompt(&prompt(None, "hi")));
        assert!(valid_prompt(&prompt(
            Some(CONVERSATION),
            &"x".repeat(MAX_PROMPT_BYTES)
        )));
        assert!(!valid_prompt(&prompt(None, "")));
        assert!(!valid_prompt(&prompt(None, "  \n")));
        assert!(!valid_prompt(&prompt(
            None,
            &"x".repeat(MAX_PROMPT_BYTES + 1)
        )));
        assert!(!valid_prompt(&prompt(Some("not-a-uuid"), "hi")));
        assert_eq!(
            parse_browser_url("http://127.0.0.1:8790").unwrap().port(),
            8790
        );
        assert!(parse_browser_url("http://[::1]:8790/").is_ok());
        for bad in [
            "http://10.0.0.5:8790",
            "https://127.0.0.1:8790",
            "http://localhost:8790",
            "http://127.0.0.1",
        ] {
            assert!(parse_browser_url(bad).is_err(), "{bad}");
        }
        let hub = LiveEvents::default();
        let addr = "127.0.0.1:1".parse().unwrap();
        assert!(Live::new(hub.clone(), addr, "short").is_err());
        assert!(Live::new(hub, addr, "has space in it but long enough").is_err());
    }

    #[tokio::test]
    async fn token_mode_requires_session_and_csrf_and_forwards_with_secret() {
        let (addr, seen) =
            fake_sidecar("202 Accepted", r#"{"accepted":true,"job_id":"job-1"}"#).await;
        let events = LiveEvents::default();
        let app = test_app_state(
            WebAuth::Token(Arc::from("synthetic-access-token")),
            true,
            live(&events, addr),
        );
        let body = prompt_body();
        // Unauthenticated: no session.
        assert_eq!(
            status_of(&app, request("POST", "/api/live/prompt", "x", None, &body)).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status_of(&app, request("GET", "/api/live/stream", "x", None, "")).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status_of(&app, request("GET", "/api/live/status", "x", None, "")).await,
            StatusCode::UNAUTHORIZED
        );
        let login = app
            .clone()
            .oneshot(request(
                "POST",
                "/api/session",
                "x",
                None,
                r#"{"token":"synthetic-access-token"}"#,
            ))
            .await
            .unwrap();
        let cookies: Vec<String> = login
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|v| v.to_str().unwrap().split(';').next().unwrap().to_owned())
            .collect();
        let csrf = cookies[1].split_once('=').unwrap().1.to_owned();
        let cookie = cookies.join("; ");
        let with = |csrf: Option<&str>| {
            let mut req = request("POST", "/api/live/prompt", "x", None, &body);
            req.headers_mut()
                .insert(header::COOKIE, cookie.parse().unwrap());
            if let Some(csrf) = csrf {
                req.headers_mut()
                    .insert("x-csrf-token", csrf.parse().unwrap());
            }
            req
        };
        // Session without CSRF header, or with a wrong one: forbidden, nothing forwarded.
        assert_eq!(status_of(&app, with(None)).await, StatusCode::FORBIDDEN);
        assert_eq!(
            status_of(&app, with(Some("wrong"))).await,
            StatusCode::FORBIDDEN
        );
        assert!(seen.lock().unwrap().is_empty());
        let response = app.clone().oneshot(with(Some(&csrf))).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let json: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(json, json!({"accepted": true, "job_id": "job-1"}));
        let forwarded = seen.lock().unwrap()[0].clone();
        assert!(forwarded.starts_with("POST /prompt HTTP/1.1"));
        assert!(forwarded.contains(&format!("Authorization: Bearer {SECRET}")));
        assert!(forwarded.contains(TEXT) && forwarded.contains(CONVERSATION));
        // Validation failures never reach the sidecar.
        for bad in [
            r#"{"text":""}"#,
            r#"{"conversation_id":"../x","text":"hi"}"#,
            r#"{"text":"hi","extra":1}"#,
            "not json",
        ] {
            let mut req = request("POST", "/api/live/prompt", "x", None, bad);
            req.headers_mut()
                .insert(header::COOKIE, cookie.parse().unwrap());
            req.headers_mut()
                .insert("x-csrf-token", csrf.parse().unwrap());
            assert_eq!(status_of(&app, req).await, StatusCode::BAD_REQUEST, "{bad}");
        }
        assert_eq!(seen.lock().unwrap().len(), 1);
        let mut caps = request("GET", "/api/capabilities", "x", None, "");
        caps.headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        let caps = app.clone().oneshot(caps).await.unwrap();
        let caps: Value =
            serde_json::from_slice(&to_bytes(caps.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(caps["capabilities"]["live"], true);
        assert_eq!(caps["capabilities"]["provider_view"], false);
    }

    #[tokio::test]
    async fn sidecar_busy_and_failures_map_to_distinct_codes() {
        let events = LiveEvents::default();
        let origin = "http://127.0.0.1:8080";
        let auth = || {
            WebAuth::TrustedProxy(trusted_origin("127.0.0.1:8080".parse().unwrap(), None).unwrap())
        };
        let (busy, _) = fake_sidecar("409 Conflict", r#"{"code":"busy"}"#).await;
        let app = test_app_state(auth(), true, live(&events, busy));
        let post = || {
            request(
                "POST",
                "/api/live/prompt",
                "127.0.0.1:8080",
                Some(origin),
                &prompt_body(),
            )
        };
        assert_eq!(status_of(&app, post()).await, StatusCode::CONFLICT);
        let (bad, _) = fake_sidecar("401 Unauthorized", "{}").await;
        let app = test_app_state(auth(), true, live(&events, bad));
        assert_eq!(status_of(&app, post()).await, StatusCode::BAD_GATEWAY);
        // Nothing listens on the port: offline, never a hang.
        let closed = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap();
        let app = test_app_state(auth(), true, live(&events, closed));
        assert_eq!(status_of(&app, post()).await, StatusCode::BAD_GATEWAY);
        let response = app
            .oneshot(request(
                "GET",
                "/api/live/status",
                "127.0.0.1:8080",
                None,
                "",
            ))
            .await
            .unwrap();
        let json: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(json, json!({"state": "offline"}));
    }

    #[tokio::test]
    async fn status_passes_only_the_state_name() {
        let (addr, _) = fake_sidecar(
            "200 OK",
            r#"{"state":"idle","live":"needs_sign_in","counters":{}}"#,
        )
        .await;
        let auth =
            WebAuth::TrustedProxy(trusted_origin("127.0.0.1:8080".parse().unwrap(), None).unwrap());
        let app = test_app_state(auth, true, live(&LiveEvents::default(), addr));
        let response = app
            .oneshot(request(
                "GET",
                "/api/live/status",
                "127.0.0.1:8080",
                None,
                "",
            ))
            .await
            .unwrap();
        let json: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(json, json!({"state": "needs_sign_in"}));
    }

    #[tokio::test]
    async fn trusted_proxy_mode_rejects_other_hosts_and_origins() {
        let (addr, seen) =
            fake_sidecar("202 Accepted", r#"{"accepted":true,"job_id":"job-2"}"#).await;
        let auth =
            WebAuth::TrustedProxy(trusted_origin("127.0.0.1:8080".parse().unwrap(), None).unwrap());
        let app = test_app_state(auth, true, live(&LiveEvents::default(), addr));
        let host = "127.0.0.1:8080";
        let origin = "http://127.0.0.1:8080";
        let body = prompt_body();
        for (method, path, sent_host, sent_origin) in [
            ("POST", "/api/live/prompt", "evil.example", None),
            (
                "POST",
                "/api/live/prompt",
                host,
                Some("http://evil.example"),
            ),
            ("GET", "/api/live/stream", "evil.example", None),
            ("GET", "/api/live/status", host, Some("http://evil.example")),
        ] {
            let sent = request(method, path, sent_host, sent_origin, &body);
            assert_eq!(
                status_of(&app, sent).await,
                StatusCode::FORBIDDEN,
                "{method} {path}"
            );
        }
        assert!(seen.lock().unwrap().is_empty());
        assert_eq!(
            status_of(
                &app,
                request("POST", "/api/live/prompt", host, Some(origin), &body)
            )
            .await,
            StatusCode::ACCEPTED
        );
        assert_eq!(
            status_of(&app, request("GET", "/api/live/stream", host, None, "")).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn live_routes_are_absent_and_capability_false_without_a_sidecar() {
        let auth =
            WebAuth::TrustedProxy(trusted_origin("127.0.0.1:8080".parse().unwrap(), None).unwrap());
        let app = test_app_state(auth, true, None);
        let host = "127.0.0.1:8080";
        let origin = "http://127.0.0.1:8080";
        assert_eq!(
            status_of(
                &app,
                request(
                    "POST",
                    "/api/live/prompt",
                    host,
                    Some(origin),
                    &prompt_body()
                )
            )
            .await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status_of(&app, request("GET", "/api/live/stream", host, None, "")).await,
            StatusCode::NOT_FOUND
        );
        let response = app
            .oneshot(request("GET", "/api/capabilities", host, None, ""))
            .await
            .unwrap();
        let caps: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(caps["capabilities"]["live"], false);
    }

    fn authelia_app(addr: SocketAddr, events: &LiveEvents) -> Router {
        let origin =
            authelia_origin(Some("https://chat-hub-preview.ginnungagaps.xyz".into())).unwrap();
        let auth = WebAuth::Authelia {
            origin,
            expected_user: Arc::from("esben"),
            proxy_ips: Arc::new(vec!["172.21.0.2".parse().unwrap()]),
        };
        test_app_state(auth, true, live(events, addr))
    }

    fn authelia_request(
        method: &str,
        path: &str,
        peer: &str,
        user: Option<&str>,
        origin: Option<&str>,
    ) -> Request<Body> {
        let mut req = request(
            method,
            path,
            "chat-hub-preview.ginnungagaps.xyz",
            origin,
            &prompt_body(),
        );
        req.headers_mut()
            .insert("x-forwarded-proto", "https".parse().unwrap());
        if let Some(user) = user {
            req.headers_mut()
                .insert("remote-user", user.parse().unwrap());
        }
        req.extensions_mut()
            .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
        req
    }

    #[tokio::test]
    async fn authelia_mode_requires_proxy_peer_user_and_exact_origin() {
        let (addr, seen) =
            fake_sidecar("202 Accepted", r#"{"accepted":true,"job_id":"job-3"}"#).await;
        let app = authelia_app(addr, &LiveEvents::default());
        let good = "https://chat-hub-preview.ginnungagaps.xyz";
        let peer = "172.21.0.2:53111";
        for (method, path, peer, user, origin) in [
            (
                "POST",
                "/api/live/prompt",
                "10.9.9.9:1",
                Some("esben"),
                Some(good),
            ),
            ("POST", "/api/live/prompt", peer, None, Some(good)),
            (
                "POST",
                "/api/live/prompt",
                peer,
                Some("mallory"),
                Some(good),
            ),
            ("POST", "/api/live/prompt", peer, Some("esben"), None),
            (
                "POST",
                "/api/live/prompt",
                peer,
                Some("esben"),
                Some("https://evil.example"),
            ),
            ("GET", "/api/live/stream", "10.9.9.9:1", Some("esben"), None),
            ("GET", "/api/live/stream", peer, None, None),
            (
                "GET",
                "/api/live/status",
                peer,
                Some("esben"),
                Some("https://evil.example"),
            ),
        ] {
            let sent = authelia_request(method, path, peer, user, origin);
            assert_eq!(
                status_of(&app, sent).await,
                StatusCode::FORBIDDEN,
                "{method} {path} {peer} {user:?} {origin:?}"
            );
        }
        assert!(seen.lock().unwrap().is_empty());
        let ok = authelia_request("POST", "/api/live/prompt", peer, Some("esben"), Some(good));
        assert_eq!(status_of(&app, ok).await, StatusCode::ACCEPTED);
        let ok = authelia_request("GET", "/api/live/stream", peer, Some("esben"), None);
        let response = app.oneshot(ok).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/event-stream"
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }

    fn event(kind: LiveEventKind, text: &str) -> LiveEvent {
        LiveEvent {
            conversation_id: Some(CONVERSATION.into()),
            message_id: "m1".into(),
            kind,
            text_delta: text.into(),
        }
    }

    async fn read_until(
        body: &mut (impl futures_util::Stream<Item = Result<Bytes, axum::Error>> + Unpin),
        needle: &str,
    ) -> String {
        let mut seen = String::new();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !seen.contains(needle) {
                seen.push_str(&String::from_utf8_lossy(
                    &body.next().await.unwrap().unwrap(),
                ));
            }
        })
        .await
        .expect("frame arrives");
        seen
    }

    #[tokio::test]
    async fn stream_relays_events_and_resyncs_slow_clients_instead_of_buffering() {
        let events = LiveEvents::default();
        let (addr, _) = fake_sidecar("200 OK", "{}").await;
        let auth =
            WebAuth::TrustedProxy(trusted_origin("127.0.0.1:8080".parse().unwrap(), None).unwrap());
        let app = test_app_state(auth, true, live(&events, addr));
        let open = || request("GET", "/api/live/stream", "127.0.0.1:8080", None, "");
        let mut fast = app
            .clone()
            .oneshot(open())
            .await
            .unwrap()
            .into_body()
            .into_data_stream();
        let mut slow = app
            .oneshot(open())
            .await
            .unwrap()
            .into_body()
            .into_data_stream();
        events.publish(event(LiveEventKind::Delta, "hello"));
        let frames = read_until(&mut fast, "event: live").await;
        assert!(frames.starts_with(": connected"));
        assert!(frames.contains(r#""text_delta":"hello""#));
        // The slow client never reads while more events than the hub keeps are published.
        for _ in 0..(chat_hub_runtime::live_capture::EVENT_QUEUE + 8) {
            events.publish(event(LiveEventKind::Delta, "x"));
        }
        assert!(read_until(&mut slow, "event: resync")
            .await
            .contains("event: resync"));
    }
}
