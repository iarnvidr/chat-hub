mod common;

use chat_hub::{
    config::Config,
    proxy::{CaptureEvent, ProxyConfig},
    service,
};
use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};
use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn config(db: String, ca: PathBuf) -> Config {
    Config {
        capture_enabled: true,
        capture_profile_scope: "provisional:default".into(),
        bind_addr: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        bearer_token: "test".into(),
        database_path: db,
        ca_dir: ca,
        proxy_addr: "127.0.0.1:0".parse().unwrap(),
    }
}
fn tmp(label: &str) -> PathBuf {
    common::temp_dir(&format!("chat-hub-service-{label}"))
}
fn event(path: &str, body: &[u8]) -> CaptureEvent {
    CaptureEvent {
        host: "chatgpt.com".into(),
        path: path.into(),
        status: 200,
        body: body.to_vec(),
    }
}

#[tokio::test]
async fn bounded_service_ingests_synthetic_capture_and_drains() {
    let root = tmp("ingest");
    let (handle, tx) = service::start(
        config(
            root.join("db.sqlite").display().to_string(),
            root.join("ca"),
        ),
        1,
    )
    .await
    .unwrap();
    tx.send(event("/backend-api/conversation/c1", br#"{"title":"Example","mapping":{"n1":{"message":{"author":{"role":"user"},"content":{"parts":["hello"]}} ,"parent":null}}}"#)).await.unwrap();
    drop(tx);
    let stats = handle.shutdown().await;
    assert_eq!(stats.stored, 1);
    assert_eq!(stats.unprocessed, 0);
    let store =
        chat_hub::store::Store::open(&root.join("db.sqlite").display().to_string()).unwrap();
    let conversation = store.read("c1").unwrap().unwrap();
    assert_eq!(conversation["messages"][0]["content"], "hello");
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn malformed_capture_is_ignored_then_valid_capture_is_stored() {
    let root = tmp("malformed-then-valid");
    let (handle, tx) = service::start(
        config(
            root.join("db.sqlite").display().to_string(),
            root.join("ca"),
        ),
        4,
    )
    .await
    .unwrap();
    tx.send(event("/backend-api/conversation/bad", b"null"))
        .await
        .unwrap();
    tx.send(event(
        "/backend-api/conversation/good",
        br#"{"title":"Good","mapping":{}}"#,
    ))
    .await
    .unwrap();
    drop(tx);
    let stats = handle.shutdown().await;
    assert_eq!(stats.accepted, 2);
    assert_eq!(stats.ignored, 1);
    assert_eq!(stats.stored, 1);
    assert_eq!(stats.failed, 0);
    let store =
        chat_hub::store::Store::open(&root.join("db.sqlite").display().to_string()).unwrap();
    assert!(store.read("good").unwrap().is_some());
    assert!(store.read("bad").unwrap().is_none());
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn start_proxy_failure_releases_api_listener_and_drop_releases_api() {
    let root = tmp("rollback");
    let occupied = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = occupied.local_addr().unwrap();
    let mut cfg = config(
        root.join("db.sqlite").display().to_string(),
        root.join("ca"),
    );
    cfg.proxy_addr = proxy_addr;
    assert!(
        service::start_with_proxy(cfg.clone(), 1, ProxyConfig::loopback(root.join("ca")))
            .await
            .is_err()
    );
    drop(occupied);
    let probe = TcpListener::bind(cfg.bind_addr).await.unwrap();
    drop(probe);
    let (handle, _) = service::start(cfg, 1).await.unwrap();
    let api = handle.status().api_addr.unwrap();
    drop(handle);
    tokio::time::sleep(Duration::from_millis(20)).await;
    let probe = TcpListener::bind(api).await.unwrap();
    drop(probe);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn write_trigger_failure_is_counted_and_shutdown_drains_sender() {
    let root = tmp("write-error");
    let db = root.join("db.sqlite");
    let (handle, tx) = service::start(config(db.display().to_string(), root.join("ca")), 1)
        .await
        .unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_conversations BEFORE INSERT ON conversations BEGIN SELECT RAISE(FAIL, 'test write failure'); END;").unwrap();
    drop(conn);
    tx.send(event(
        "/backend-api/conversation/fail",
        br#"{"title":"fail","mapping":{}}"#,
    ))
    .await
    .unwrap();
    drop(tx);
    let stats = handle.shutdown().await;
    assert_eq!(stats.failed, 1);
    assert_eq!(stats.unprocessed, 0);
    let store = chat_hub::store::Store::open(&db.display().to_string()).unwrap();
    assert!(store.health().unwrap());
    assert!(store.read("fail").unwrap().is_none());
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn shutdown_with_live_sender_counts_queued_events_as_unprocessed() {
    let root = tmp("live-sender");
    let (handle, tx) = service::start(
        config(
            root.join("db.sqlite").display().to_string(),
            root.join("ca"),
        ),
        4,
    )
    .await
    .unwrap();
    tx.send(event("/ignored", b"x")).await.unwrap();
    let stats = handle.shutdown_timeout(Duration::from_secs(2)).await;
    assert_eq!(stats.accepted, 1);
    assert_eq!(stats.ignored, 1);
    assert_eq!(stats.unprocessed, 0);
    assert!(tx.send(event("/ignored", b"late")).await.is_err());
    drop(tx);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn bounded_queue_accepts_only_capacity_and_drains_after_sqlite_lock_release() {
    let root = tmp("queue-lock");
    let db = root.join("db.sqlite");
    let (handle, tx) = service::start(config(db.display().to_string(), root.join("ca")), 1)
        .await
        .unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch("BEGIN EXCLUSIVE").unwrap();
    tx.send(event(
        "/backend-api/conversation/one",
        br#"{"title":"one","mapping":{}}"#,
    ))
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while handle.status().ingest.accepted == 0 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    tx.try_send(event(
        "/backend-api/conversation/two",
        br#"{"title":"two","mapping":{}}"#,
    ))
    .unwrap();
    assert!(service::try_submit(&tx, event("/backend-api/conversation/three", b"{}")).is_err());
    conn.execute_batch("ROLLBACK").unwrap();
    drop(conn);
    drop(tx);
    let before_shutdown = handle.status();
    let stats = tokio::time::timeout(Duration::from_secs(3), handle.shutdown())
        .await
        .unwrap_or_else(|_| panic!(
            "shutdown timed out after releasing SQLite lock; status state={:?}, ingest accepted={} stored={} ignored={} failed={} unprocessed={}, sender_closed={}",
            before_shutdown.state,
            before_shutdown.ingest.accepted,
            before_shutdown.ingest.stored,
            before_shutdown.ingest.ignored,
            before_shutdown.ingest.failed,
            before_shutdown.ingest.unprocessed,
            true, // sender was dropped before shutdown
        ));
    assert_eq!(stats.accepted, 2);
    assert_eq!(stats.stored + stats.failed, 2);
    assert_eq!(stats.unprocessed, 0);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn dropping_sender_before_deadline_preserves_exact_work_conservation() {
    let root = tmp("deadline-lock");
    let db = root.join("db.sqlite");
    let (handle, tx) = service::start(config(db.display().to_string(), root.join("ca")), 1)
        .await
        .unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch("BEGIN EXCLUSIVE").unwrap();
    tx.send(event(
        "/backend-api/conversation/one",
        br#"{"title":"one","mapping":{}}"#,
    ))
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while handle.status().ingest.accepted == 0 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    tx.try_send(event("/ignored", b"queued")).unwrap();
    drop(tx); // Close the sender while one accepted write is blocked and one item is queued.
    let stats = tokio::time::timeout(
        Duration::from_secs(3),
        handle.shutdown_timeout(Duration::from_millis(100)),
    )
    .await
    .unwrap();
    assert!(stats.accepted <= 2, "{stats:?}");
    assert_eq!(
        stats.stored + stats.ignored + stats.failed + stats.unprocessed,
        2,
        "{stats:?}"
    );
    conn.execute_batch("ROLLBACK").unwrap();
    drop(conn);
    tokio::time::sleep(Duration::from_millis(150)).await;
    let _ = std::fs::remove_dir_all(root);
}

// Exercise the real CONNECT/TLS proxy feeding the service's bounded writer and SQLite store.
#[tokio::test]
async fn tls_capture_flows_through_service_into_conversation_store() {
    exercise_live_tls(true, None).await;
}

#[tokio::test]
async fn disabled_capture_forwards_tls_without_retention_or_archival() {
    exercise_live_tls(false, None).await;
}

#[tokio::test]
async fn live_tls_identity_validation_and_raw_path_fallback_are_lossless() {
    for case in [
        "fallback",
        "mismatch",
        "malformed_id",
        "malformed_conversation_id",
        "conflicting_ids",
        "null_empty",
        "empty_identity",
    ] {
        exercise_live_tls(true, Some(case)).await;
    }
}

#[tokio::test]
async fn live_tls_malformed_oversize_encoding_and_store_failure_preserve_traffic() {
    for case in [
        "invalid_snapshot",
        "oversized",
        "unsupported_encoding",
        "store_failed",
    ] {
        exercise_live_tls(true, Some(case)).await;
    }
}

#[tokio::test]
async fn live_tls_gzip_and_deflate_snapshots_persist_without_changing_wire_response() {
    exercise_live_tls(true, Some("gzip")).await;
    exercise_live_tls(true, Some("deflate")).await;
    exercise_live_tls(true, Some("bad_gzip")).await;
    exercise_live_tls(true, Some("sse")).await;
    exercise_live_tls(true, Some("modern_sse")).await;
}

async fn exercise_live_tls(enabled: bool, case: Option<&str>) {
    let is_sse = matches!(case, Some("sse" | "modern_sse"));
    let request_path = if case == Some("modern_sse") {
        "/backend-api/f/conversation"
    } else {
        "/backend-api/conversation"
    };
    let root = tmp("tls");
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let cert = CertificateParams::new(vec!["chatgpt.com".into()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let roots = root.join("upstream.pem");
    std::fs::write(&roots, cert.pem()).unwrap();
    let upstream_cfg = rustls::ServerConfig::builder_with_provider(
        rustls::crypto::ring::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        rustls_pemfile::certs(&mut cert.pem().as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .unwrap(),
        rustls_pemfile::private_key(&mut key.serialize_pem().as_bytes())
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = listener.local_addr().unwrap();
    let mut doc = serde_json::from_slice::<serde_json::Value>(include_bytes!(
        "fixtures/chatgpt-export/array.json"
    ))
    .unwrap()[0]
        .clone();
    match case {
        Some("fallback") => {
            doc.as_object_mut().unwrap().remove("id");
        }
        Some("mismatch") => doc["id"] = serde_json::json!("other"),
        Some("malformed_id") => doc["id"] = serde_json::json!([]),
        Some("malformed_conversation_id") => doc["conversation_id"] = serde_json::json!(23),
        Some("conflicting_ids") => {
            doc["conversation_id"] = serde_json::json!("other");
        }
        Some("empty_identity") => {
            doc["id"] = serde_json::json!("");
            doc["conversation_id"] = serde_json::json!("");
        }
        Some("null_empty") => doc["conversation_id"] = serde_json::Value::Null,
        _ => {}
    }
    let request_payload = if is_sse {
        serde_json::json!({
            "conversation_id": null,
            "parent_message_id": "root-node",
            "messages": [{
                "id": "user-node",
                "author": {"role":"user"},
                "content": {"content_type":"text","parts":["streamed question"]}
            }]
        })
        .to_string()
        .into_bytes()
    } else {
        b"request".to_vec()
    };
    let body = if case == Some("modern_sse") {
        let mut stream = String::from("data: \"v1\"\r\n\r\n");
        let values = [
            serde_json::json!({"type":"resume_conversation_token","conversation_id":"sse-conversation","token":"PRIVATE_RESUME_CANARY"}),
            serde_json::json!({"conversation_id":"sse-conversation","title":"SSE Test"}),
            serde_json::json!({"p":"","o":"add","c":0,"v":{"message":{"id":"user-node","parent_id":"root-node","author":{"role":"user"},"content":{"content_type":"text","parts":["streamed question"]}}}}),
            serde_json::json!({"p":"","o":"add","c":1,"v":{"message":{"id":"assistant-node","parent_id":"user-node","author":{"role":"assistant"},"content":{"content_type":"text","parts":[""]},"status":"in_progress","end_turn":null}}}),
            serde_json::json!({"p":"/message/content/parts/0","o":"append","c":1,"v":"live answer"}),
            serde_json::json!({"o":"patch","p":"","c":1,"v":[{"p":"/message/status","o":"replace","v":"finished_successfully"},{"p":"/message/end_turn","o":"replace","v":true}]}),
        ];
        for value in values {
            stream.push_str("data: ");
            stream.push_str(&value.to_string());
            stream.push_str("\r\n\r\n");
        }
        stream.push_str("data: [DONE]\r\n\r\n");
        stream.into_bytes()
    } else if is_sse {
        let mut stream = String::new();
        for value in [
            serde_json::json!({"conversation_id":"sse-conversation","title":"SSE Test"}),
            serde_json::json!({"conversation_id":"sse-conversation","message":{"id":"assistant-node","parent_id":"user-node","author":{"role":"assistant"},"content":{"content_type":"text","parts":["live answer"]}}}),
        ] {
            stream.push_str("data: ");
            stream.push_str(&value.to_string());
            stream.push_str("\r\n\r\n");
        }
        stream.push_str("data: [DONE]\r\n\r\n");
        stream.into_bytes()
    } else {
        match case {
            Some("invalid_snapshot") => b"not-json PRIVATE_BODY_SENTINEL".to_vec(),
            Some("oversized") => vec![b'x'; chat_hub::live_capture::ITEM_BYTES + 1],
            _ => serde_json::to_vec(&doc).unwrap(),
        }
    };
    let terminal = match case {
        Some(
            "mismatch"
            | "malformed_id"
            | "malformed_conversation_id"
            | "conflicting_ids"
            | "invalid_snapshot",
        ) => "invalid_snapshot",
        Some("oversized") => "oversized",
        Some("unsupported_encoding") => "unsupported_encoding",
        Some("bad_gzip") => "decode_failed",
        Some("store_failed") => "store_failed",
        _ => "committed_imported",
    };
    let successful = enabled && terminal == "committed_imported";
    let encoding_header = if case == Some("unsupported_encoding") {
        "Content-Encoding: br\r\n"
    } else if case == Some("gzip") || case == Some("bad_gzip") {
        "Content-Encoding: gzip\r\n"
    } else if case == Some("deflate") {
        "Content-Encoding: deflate\r\n"
    } else {
        ""
    };
    let response_body = match case {
        Some("gzip") => {
            use flate2::{write::GzEncoder, Compression};
            use std::io::Write;
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&body).unwrap();
            encoder.finish().unwrap()
        }
        Some("bad_gzip") => b"not a gzip stream".to_vec(),
        Some("deflate") => {
            use flate2::{write::ZlibEncoder, Compression};
            use std::io::Write;
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&body).unwrap();
            encoder.finish().unwrap()
        }
        _ => body.clone(),
    };
    let response_content_type = if is_sse {
        "Content-Type: text/event-stream\r\n"
    } else {
        "Content-Type: application/json\r\n"
    };
    let expected_request_payload = request_payload.clone();
    let expected_response_body = response_body.clone();
    let upstream_task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut tls = tokio_rustls::TlsAcceptor::from(Arc::new(upstream_cfg))
            .accept(stream)
            .await
            .unwrap();
        let mut request = Vec::new();
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") {
            tls.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
        }
        let text = String::from_utf8_lossy(&request);
        if is_sse {
            assert!(text.starts_with(&format!("POST {request_path} HTTP/1.1")));
        }
        let length: usize = text
            .lines()
            .find_map(|l| {
                l.to_ascii_lowercase()
                    .strip_prefix("content-length: ")
                    .map(str::to_owned)
            })
            .unwrap()
            .parse()
            .unwrap();
        let end = request.len();
        let mut payload = vec![0; length];
        tls.read_exact(&mut payload).await.unwrap();
        assert!(end > 0);
        assert_eq!(payload, expected_request_payload);
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n{response_content_type}{encoding_header}\r\n",
            response_body.len()
        );
        tls.write_all(head.as_bytes()).await.unwrap();
        tls.write_all(&response_body).await.unwrap();
        tls.shutdown().await.unwrap();
    });
    let mut proxy = ProxyConfig::loopback(root.join("unused"));
    proxy.test_upstream = Some(upstream_addr);
    proxy.test_roots = Some(roots);
    let mut service_config = config(
        root.join("db.sqlite").display().to_string(),
        root.join("ca"),
    );
    service_config.capture_enabled = enabled;
    service_config.capture_profile_scope = "profile:tls-test".into();
    let (handle, tx) = service::start_with_proxy(service_config, 1, proxy)
        .await
        .unwrap();
    if case == Some("store_failed") {
        let injector = rusqlite::Connection::open(root.join("db.sqlite")).unwrap();
        injector.execute_batch("CREATE TRIGGER synthetic_live_abort BEFORE INSERT ON observations BEGIN SELECT RAISE(ABORT,'PRIVATE_SQL_SENTINEL'); END;").unwrap();
    }
    let status = handle.status();
    let proxy_addr = status.proxy_addr.unwrap();
    let ca = chat_hub::ca::Ca::load_or_create(root.join("ca")).unwrap();
    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::https(format!("http://{proxy_addr}")).unwrap())
        .add_root_certificate(reqwest::Certificate::from_pem(ca.cert_pem.as_bytes()).unwrap())
        .timeout(Duration::from_secs(8))
        .build()
        .unwrap();
    let response = if is_sse {
        client
            .post(format!("https://chatgpt.com{request_path}"))
            .header("content-type", "application/json")
            .body(request_payload)
            .send()
            .await
            .unwrap()
    } else {
        client
            .get("https://chatgpt.com/backend-api/conversation/synthetic-export-one")
            .body("request")
            .send()
            .await
            .unwrap()
    };
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.bytes().await.unwrap().as_ref(),
        expected_response_body.as_slice()
    );
    upstream_task.await.unwrap();
    if !enabled {
        tx.send(event(
            "/backend-api/conversation/direct-must-not-archive",
            &body,
        ))
        .await
        .unwrap();
    }
    let capture = handle.capture_status();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let api = reqwest::Client::builder().no_proxy().build().unwrap();
    let api_url = format!("http://{}", status.api_addr.unwrap());
    let detail = loop {
        let response = api
            .get(format!("{api_url}/api/v1/status"))
            .bearer_auth("test")
            .send()
            .await
            .unwrap();
        let http_status = response.status();
        let bytes = response.bytes().await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        // A concurrent capture write can briefly occupy the read adapter. Retry
        // only that explicit admission failure, within the original deadline.
        if http_status == reqwest::StatusCode::SERVICE_UNAVAILABLE
            && v["error"]["code"] == "service_unavailable"
        {
            assert!(
                tokio::time::Instant::now() < deadline,
                "status polling timed out: HTTP {http_status}, service_unavailable"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
            continue;
        }
        assert_eq!(http_status, reqwest::StatusCode::OK);
        assert_eq!(v["capture"]["enabled"], enabled);
        assert_eq!(v["service"]["status"], "ready");
        assert_eq!(v["database"]["status"], "healthy");
        assert!(!v.to_string().contains("PRIVATE_SQL_SENTINEL"));
        assert!(!v.to_string().contains("PRIVATE_BODY_SENTINEL"));
        if enabled && v["capture"]["counters"][terminal] == 1 {
            break v;
        }
        if !enabled && handle.status().ingest.ignored == 1 {
            break v;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "capture polling timed out: HTTP {http_status}, terminal counter {}",
            v["capture"]["counters"][terminal]
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    if !enabled {
        assert_eq!(detail["capture"]["counters"]["started"], 0);
        assert_eq!(detail["capture"]["counters"]["retained_bytes"], 0);
    } else if successful {
        let bytes = api
            .get(format!("{api_url}/api/v1/conversations"))
            .bearer_auth("test")
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        let list: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(list["items"].as_array().unwrap().len(), 1);
        let id = list["items"][0]["id"].as_str().unwrap();
        let mut url = reqwest::Url::parse(&format!("{api_url}/api/v1/conversations/")).unwrap();
        url.path_segments_mut().unwrap().pop_if_empty().push(id);
        let bytes = api
            .get(url)
            .bearer_auth("test")
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        let graph: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        if is_sse {
            assert_eq!(graph["conversation"]["title"], "SSE Test");
            assert_eq!(graph["nodes"].as_array().unwrap().len(), 2);
            assert_eq!(graph["selection"]["selected_node_id"], "assistant-node");
            assert!(graph["messages"].to_string().contains("streamed question"));
            assert!(graph["messages"].to_string().contains("live answer"));
            assert_eq!(graph["conversation"]["account_scope"], "profile:tls-test");
        } else {
            assert_eq!(graph["nodes"].as_array().unwrap().len(), 3);
            assert_eq!(graph["selection"]["selected_node_id"], "right");
            assert!(graph["messages"].to_string().contains("future_part"));
        }
    }
    let stats = tokio::time::timeout(Duration::from_secs(5), handle.shutdown())
        .await
        .unwrap();
    // Compatibility ingress accounting does not count typed live attempts or
    // turn their pending commits into legacy unprocessed outcomes at deadlines.
    assert_eq!(stats.stored, 0);
    assert_eq!(stats.failed, 0);
    assert_eq!(stats.unprocessed, 0);
    let store =
        chat_hub::store::Store::open(&root.join("db.sqlite").display().to_string()).unwrap();
    assert_eq!(
        store.library_snapshot().unwrap().conversations.len(),
        usize::from(successful)
    );
    if successful {
        let snapshot = store.library_snapshot().unwrap();
        assert_eq!(
            snapshot.conversations[0]["account_scope"],
            "profile:tls-test"
        );
        let saved = store
            .read_normalized(snapshot.conversations[0]["id"].as_str().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(saved["messages"].as_array().unwrap().len(), 2);
        if is_sse {
            assert!(saved.to_string().contains("streamed question"));
            assert!(saved.to_string().contains("live answer"));
        } else {
            assert!(saved.to_string().contains("Selected synthetic answer"));
        }
        let inspector = rusqlite::Connection::open(root.join("db.sqlite")).unwrap();
        let raw: String = inspector
            .query_row("SELECT raw_payload FROM observations", [], |r| r.get(0))
            .unwrap();
        if is_sse {
            let retained_stream = serde_json::from_str::<serde_json::Value>(&raw)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned();
            assert!(retained_stream.contains("[DONE]"));
            if case == Some("modern_sse") {
                assert!(!retained_stream.contains("PRIVATE_RESUME_CANARY"));
                assert!(retained_stream.contains("sse-conversation"));
                assert!(retained_stream.contains("live answer"));
            }
        } else {
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&raw).unwrap(),
                doc
            );
        }
    }
    let counters = serde_json::to_value(capture.snapshot()).unwrap();
    assert_eq!(counters["counters"]["active"], 0);
    assert_eq!(counters["counters"]["retained_bytes"], 0);
    let _ = std::fs::remove_dir_all(root);
}
