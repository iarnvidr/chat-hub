mod common;

use chat_hub::{
    ca::Ca,
    proxy::{self, CaptureEvent, ProxyConfig},
};
use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};
use std::{fs, net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::mpsc,
};

#[tokio::test]
async fn connect_mitm_forwards_tls_response_and_emits_capture() {
    // Run the actual CONNECT/TLS exercise in a new test-binary process. This is
    // deliberately the first Rustls use there, proving proxy production builders
    // do not rely on a process-global provider being installed by unrelated code.
    const CHILD: &str = "CHAT_HUB_FRESH_TLS_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "connect_mitm_forwards_tls_response_and_emits_capture",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .status()
            .expect("launch fresh TLS test process");
        assert!(
            status.success(),
            "fresh-process TLS exercise failed: {status}"
        );
        return;
    }
    exercise_connect_mitm().await;
}

async fn exercise_connect_mitm() {
    let dir = common::temp_dir("chat-hub-e2e");
    let upstream_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let upstream_params = CertificateParams::new(vec!["chatgpt.com".into()]).unwrap();
    let upstream_cert = upstream_params.self_signed(&upstream_key).unwrap();
    let roots = dir.join("upstream.pem");
    fs::write(&roots, upstream_cert.pem()).unwrap();
    let upstream_config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        rustls_pemfile::certs(&mut upstream_cert.pem().as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .unwrap(),
        rustls_pemfile::private_key(&mut upstream_key.serialize_pem().as_bytes())
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream.local_addr().unwrap();
    let upstream_task = tokio::spawn(async move {
        let (stream, _) = upstream.accept().await.unwrap();
        let mut tls = tokio_rustls::TlsAcceptor::from(Arc::new(upstream_config))
            .accept(stream)
            .await
            .unwrap();
        let mut request = Vec::new();
        let mut byte = [0; 1];
        while !request.ends_with(b"\r\n\r\n") {
            tls.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
        }
        assert!(String::from_utf8_lossy(&request).starts_with("POST /synthetic HTTP/1.1"));
        let headers = String::from_utf8_lossy(&request);
        let content_length: usize = headers
            .lines()
            .find_map(|line| line.strip_prefix("content-length: "))
            .or_else(|| {
                headers
                    .lines()
                    .find_map(|line| line.strip_prefix("Content-Length: "))
            })
            .unwrap()
            .parse()
            .unwrap();
        let header_end = request.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        let mut full_request = request;
        full_request.resize(header_end + content_length, 0);
        tls.read_exact(&mut full_request[header_end..])
            .await
            .unwrap();
        assert_eq!(content_length, 9000);
        assert!(full_request[header_end..].iter().all(|&b| b == b'x'));
        tls.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Length: 32\r\n\r\n{\"ok\":true,\"source\":\"synthetic\"}",
        )
        .await
        .unwrap();
    });
    let ca = Ca::load_or_create(dir.join("proxy-ca")).unwrap();
    let (tx, mut rx) = mpsc::channel::<CaptureEvent>(4);
    let mut config = ProxyConfig::loopback(dir.join("proxy-ca"));
    config.test_upstream = Some(upstream_addr);
    config.test_roots = Some(roots);
    let (running, proxy_task) = proxy::run(config, tx).await.unwrap();
    let proxy = reqwest::Proxy::https(format!("http://{}", running.addr)).unwrap();
    let client = reqwest::Client::builder()
        .proxy(proxy)
        .add_root_certificate(reqwest::Certificate::from_pem(ca.cert_pem.as_bytes()).unwrap())
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let payload = "x".repeat(9000);
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        client
            .post("https://chatgpt.com/synthetic")
            .body(payload)
            .send(),
    )
    .await
    .expect("length-delimited response should complete without upstream EOF")
    .unwrap();
    assert_eq!(response.status(), 200);
    let expected = r#"{"ok":true,"source":"synthetic"}"#;
    assert_eq!(response.text().await.unwrap(), expected);
    let event = tokio::time::timeout(Duration::from_secs(3), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event.host, "chatgpt.com");
    assert_eq!(event.path, "/synthetic");
    assert_eq!(event.status, 200);
    assert_eq!(event.body, expected.as_bytes());
    upstream_task.await.unwrap();
    proxy_task.abort();
    fs::remove_dir_all(dir).unwrap();
}

#[allow(dead_code)]
fn _addr(_: SocketAddr) {}
