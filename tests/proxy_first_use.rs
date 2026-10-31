mod common;

use chat_hub::{
    ca::Ca,
    proxy::{self, CaptureEvent, ProxyConfig},
};
use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};
use std::{
    fs,
    net::SocketAddr,
    process::{Child, Command, Stdio},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

const CHILD_MODE: &str = "CHAT_HUB_PROXY_FIRST_TLS_CHILD";

struct ChildGuard(Option<Child>);
impl ChildGuard {
    fn child(&mut self) -> &mut Child {
        self.0.as_mut().expect("child is still running")
    }
    fn wait_with_output(&mut self) -> std::process::Output {
        self.0
            .take()
            .expect("child is still running")
            .wait_with_output()
            .unwrap()
    }
}
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
        }
    }
}

#[tokio::test]
async fn proxy_child_intercepts_tls_after_fresh_process_provider_selection() {
    if std::env::var_os(CHILD_MODE).is_some() {
        run_proxy_child().await;
        return;
    }

    let root = common::temp_dir("proxy-first-rustls-use");
    let upstream_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let upstream_cert = CertificateParams::new(vec!["chatgpt.com".into()])
        .unwrap()
        .self_signed(&upstream_key)
        .unwrap();
    let roots_path = root.join("upstream.pem");
    fs::write(&roots_path, upstream_cert.pem()).unwrap();

    // The TLS upstream lives in the parent process. Rustls state in this process
    // therefore cannot preinstall a provider in the isolated proxy process.
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
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream_listener.local_addr().unwrap();
    let upstream = tokio::spawn(async move {
        let (stream, _) = upstream_listener.accept().await.unwrap();
        let mut tls = tokio_rustls::TlsAcceptor::from(Arc::new(upstream_config))
            .accept(stream)
            .await
            .unwrap();
        let mut request = Vec::new();
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") {
            tls.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
        }
        assert!(String::from_utf8_lossy(&request).starts_with("GET /synthetic HTTP/1.1"));
        tls.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Length: 32\r\nContent-Type: application/json\r\n\r\n{\"ok\":true,\"source\":\"synthetic\"}",
        )
        .await
        .unwrap();
    });

    let ca_dir = root.join("proxy-ca");
    let ca = Ca::load_or_create(&ca_dir).unwrap();
    let ready = root.join("proxy.ready");
    let captured = root.join("capture.json");
    let stop = root.join("stop");
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "proxy_child_intercepts_tls_after_fresh_process_provider_selection",
            "--nocapture",
        ])
        .env(CHILD_MODE, "1")
        .env("CHAT_HUB_TEST_UPSTREAM", upstream_addr.to_string())
        .env("CHAT_HUB_TEST_ROOTS", &roots_path)
        .env("CHAT_HUB_TEST_CA", &ca_dir)
        .env("CHAT_HUB_TEST_READY", &ready)
        .env("CHAT_HUB_TEST_CAPTURED", &captured)
        .env("CHAT_HUB_TEST_STOP", &stop)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("launch isolated proxy process");
    let mut child = ChildGuard(Some(child));

    let proxy_addr = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if ready.exists() {
                break fs::read_to_string(&ready)
                    .unwrap()
                    .trim()
                    .parse::<SocketAddr>()
                    .unwrap();
            }
            if let Some(status) = child.child().try_wait().unwrap() {
                panic!("proxy child exited before readiness: {status}");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("proxy child readiness deadline");

    let response = reqwest::Client::builder()
        .proxy(reqwest::Proxy::https(format!("http://{proxy_addr}")).unwrap())
        .add_root_certificate(reqwest::Certificate::from_pem(ca.cert_pem.as_bytes()).unwrap())
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
        .get("https://chatgpt.com/synthetic")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.text().await.unwrap(),
        r#"{"ok":true,"source":"synthetic"}"#
    );
    upstream.await.unwrap();

    let event: serde_json::Value = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if captured.exists() {
                break serde_json::from_slice(&fs::read(&captured).unwrap()).unwrap();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("captured response deadline");
    assert_eq!(event["host"], "chatgpt.com");
    assert_eq!(event["path"], "/synthetic");
    assert_eq!(event["status"], 200);
    assert_eq!(event["body"], r#"{"ok":true,"source":"synthetic"}"#);

    fs::write(&stop, "stop").unwrap();
    let output = child.wait_with_output();
    assert!(
        output.status.success(),
        "proxy child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = fs::remove_dir_all(root);
}

async fn run_proxy_child() {
    // Proxy startup must select the default before the intercepted TLS handshake.
    assert!(rustls::crypto::CryptoProvider::get_default().is_none());
    let upstream: SocketAddr = std::env::var("CHAT_HUB_TEST_UPSTREAM")
        .unwrap()
        .parse()
        .unwrap();
    let roots = std::env::var("CHAT_HUB_TEST_ROOTS").unwrap();
    let ca = std::env::var("CHAT_HUB_TEST_CA").unwrap();
    let ready = std::env::var("CHAT_HUB_TEST_READY").unwrap();
    let captured = std::env::var("CHAT_HUB_TEST_CAPTURED").unwrap();
    let stop = std::env::var("CHAT_HUB_TEST_STOP").unwrap();
    let mut config = ProxyConfig::loopback(ca);
    config.test_upstream = Some(upstream);
    config.test_roots = Some(roots.into());
    let (events, mut rx) = tokio::sync::mpsc::channel::<CaptureEvent>(2);
    let (running, proxy_task) = proxy::run(config, events).await.unwrap();
    let installed = rustls::crypto::CryptoProvider::get_default().expect("default provider");
    let expected = rustls::crypto::ring::default_provider();
    assert_eq!(
        installed
            .cipher_suites
            .iter()
            .map(|suite| suite.suite())
            .collect::<Vec<_>>(),
        expected
            .cipher_suites
            .iter()
            .map(|suite| suite.suite())
            .collect::<Vec<_>>()
    );

    fs::write(ready, running.addr.to_string()).unwrap();

    loop {
        tokio::select! {
            event = rx.recv() => {
                if let Some(event) = event {
                    let value = serde_json::json!({
                        "host": event.host,
                        "path": event.path,
                        "status": event.status,
                        "body": String::from_utf8_lossy(&event.body),
                    });
                    let temporary = format!("{captured}.tmp");
                    fs::write(&temporary, serde_json::to_vec(&value).unwrap()).unwrap();
                    fs::rename(temporary, &captured).unwrap();
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(20)) => {
                if std::path::Path::new(&stop).exists() { break; }
            }
        }
    }
    proxy_task.abort();
}
