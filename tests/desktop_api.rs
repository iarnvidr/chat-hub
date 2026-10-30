#![allow(dead_code)]
#[path = "../src-tauri/src/desktop_api.rs"]
mod desktop_api;

use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
};

fn fixture(response: &'static [u8]) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut req = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            let n = stream.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            req.extend_from_slice(&buf[..n]);
            if req.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        stream.write_all(response).unwrap();
        String::from_utf8_lossy(&req).into_owned()
    });
    (format!("http://{addr}/"), handle)
}

#[test]
fn builders_escape_query_and_detail_path_segments() {
    let u = desktop_api::list_url("http://127.0.0.1/", " hello&x ", Some("a/b ?")).unwrap();
    assert_eq!(u.path(), "/api/v1/search");
    let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
    assert_eq!(q["q"], "hello&x");
    assert_eq!(q["cursor"], "a/b ?");
    assert_eq!(q["limit"], "50");
    let u = desktop_api::detail_url("http://127.0.0.1/", "a/b?c#d", Some("x&y")).unwrap();
    assert_eq!(u.path(), "/api/v1/conversations/a%2Fb%3Fc%23d");
    assert_eq!(u.query_pairs().next().unwrap().1, "x&y");
}

#[tokio::test]
async fn list_uses_auth_and_maps_conflict_without_leaking_body() {
    let (base, server) = fixture(
        b"HTTP/1.1 409 Conflict\r\nContent-Length: 15\r\nConnection: close\r\n\r\nsecret sentinel",
    );
    let e = desktop_api::list(&base, "private-token", "", None)
        .await
        .unwrap_err();
    assert_eq!(e, "stale_cursor");
    let request = server.join().unwrap().to_ascii_lowercase();
    assert!(request.contains("authorization: bearer private-token"));
    assert!(request.contains("get /api/v1/conversations?limit=50"));
}

#[test]
fn import_validation_and_receipt_bounds() {
    assert!(desktop_api::validate_import(br#"{"conversations":[]}"#).is_ok());
    assert_eq!(
        desktop_api::validate_import(b"bad").unwrap_err(),
        "invalid_json"
    );
    assert_eq!(
        desktop_api::validate_import(b"{}").unwrap_err(),
        "invalid_export"
    );
    let receipt = desktop_api::ImportReceipt {
        imported: u64::MAX,
        updated: 1,
        unchanged: 0,
        failed: 0,
        errors: vec![],
        errors_truncated: false,
    };
    assert_eq!(
        desktop_api::validate_receipt(&receipt, 1).unwrap_err(),
        "invalid_response"
    );
}

#[test]
fn all_official_conversation_details_roundtrip_without_losing_fields() {
    for name in ["selected", "alternate", "fallback"] {
        let path = format!("tests/fixtures/api-v1/conversation-{name}.json");
        let original: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let parsed: desktop_api::Detail = serde_json::from_value(original.clone()).unwrap();
        let roundtrip = serde_json::to_value(parsed).unwrap();
        assert_eq!(roundtrip, original, "{name} detail lost or changed fields");
    }
}

#[tokio::test]
async fn detail_http_200_preserves_full_official_fixture() {
    let body = std::fs::read("tests/fixtures/api-v1/conversation-selected.json").unwrap();
    let body = Box::leak(body.into_boxed_slice());
    let response = [
        b"HTTP/1.1 200 OK\r\nContent-Length: ",
        body.len().to_string().as_bytes(),
        b"\r\nConnection: close\r\n\r\n",
        body,
    ]
    .concat();
    let response = Box::leak(response.into_boxed_slice());
    let (base, server) = fixture(response);
    let actual = desktop_api::detail(&base, "t", "cv-a", None).await.unwrap();
    let expected: serde_json::Value = serde_json::from_slice(body).unwrap();
    assert_eq!(serde_json::to_value(actual).unwrap(), expected);
    server.join().unwrap();
}

#[tokio::test]
async fn service_unavailable_response_too_large_maps_exact_code() {
    let (base, server) = fixture(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 39\r\nConnection: close\r\n\r\n{\"error\":{\"code\":\"response_too_large\"}}");
    let error = desktop_api::list(&base, "t", "", None).await.unwrap_err();
    assert_eq!(error, "response_too_large");
    server.join().unwrap();
}

#[test]
fn import_validation_enforces_transport_entry_limit() {
    let conversations = vec![serde_json::json!({}); desktop_api::MAX_IMPORT_CONVERSATIONS + 1];
    let data = serde_json::to_vec(&conversations).unwrap();
    assert_eq!(
        desktop_api::validate_import(&data).unwrap_err(),
        "too_many_conversations"
    );
}

#[tokio::test]
async fn get_rejects_chunked_body_over_limit_before_accumulating_it() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut req = [0u8; 2048];
        let _ = stream.read(&mut req);
        stream.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n800001\r\n").unwrap();
        let block = vec![b'x'; 0x800001];
        stream.write_all(&block).unwrap();
        stream.write_all(b"\r\n0\r\n\r\n").unwrap();
    });
    let e = desktop_api::list(&format!("http://{addr}/"), "t", "", None)
        .await
        .unwrap_err();
    assert_eq!(e, "response_too_large");
    server.join().unwrap();
}
