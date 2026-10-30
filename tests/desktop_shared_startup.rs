#[path = "../src-tauri/src/desktop_runtime.rs"]
mod desktop_runtime;
#[path = "../src-tauri/src/service_health.rs"]
mod service_health;

use chat_hub::proxy::ProxyConfig;
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{fs, net::SocketAddr, time::Duration};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct TestDir(std::path::PathBuf);
impl TestDir {
    fn new() -> Self {
        loop {
            let path = std::env::temp_dir().join(format!(
                "chat-hub-desktop-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("create temp directory: {e}"),
            }
        }
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

async fn json(response: reqwest::Response) -> Value {
    let bytes = response.bytes().await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn get(client: &reqwest::Client, url: &str, token: &str) -> Value {
    let response = client
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let bytes = response.bytes().await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn production_helper_serves_real_http_and_persists_across_restart() {
    let dir = TestDir::new();
    let fixture = include_bytes!("fixtures/chatgpt-export/array.json");
    let client = reqwest::Client::new();
    let runtime = desktop_runtime::start(dir.path(), ProxyConfig::loopback(dir.path().join("ca")))
        .await
        .unwrap();
    let token = runtime.token.clone();
    let api = runtime.api.clone();
    let api_addr: SocketAddr = api.strip_prefix("http://").unwrap().parse().unwrap();
    let proxy_addr: SocketAddr = runtime.proxy.parse().unwrap();
    let ca_dir = dir.path().join("ca");
    assert_eq!(runtime.ca_dir, ca_dir);
    assert!(proxy_addr.ip().is_loopback() && proxy_addr.port() != 0);
    let status = runtime.status();
    assert_eq!(status.state, chat_hub::service::ServiceState::Ready);
    let response = client
        .get(format!("{api}/api/v1/status"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let payload = json(response).await;
    assert_eq!(payload["status"], "healthy");
    assert_eq!(payload["service"]["status"], "ready");
    assert_eq!(payload["database"]["status"], "healthy");
    assert!(service_health::healthy_response(200, &payload));
    assert!(!client
        .get(format!("{api}/api/v1/status"))
        .send()
        .await
        .unwrap()
        .status()
        .is_success());

    let imported = client
        .post(format!("{api}/api/v1/imports/chatgpt"))
        .bearer_auth(&token)
        .header("content-type", "application/json")
        .body(fixture.to_vec())
        .send()
        .await
        .unwrap();
    assert!(
        imported.status().is_success(),
        "{}",
        imported.text().await.unwrap()
    );
    let list = get(&client, &format!("{api}/api/v1/conversations"), &token).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    let id = list["items"][0]["id"].as_str().unwrap();
    let search = client
        .get(format!("{api}/api/v1/search?q=Alternate"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let search = json(search).await;
    assert!(!search["items"].as_array().unwrap().is_empty());
    let detail = get(&client, &format!("{api}/api/v1/conversations/{id}"), &token).await;
    assert!(detail.to_string().contains("Selected synthetic answer"));
    assert!(detail["content"].is_object() || detail["messages"].is_array());
    let cert = std::fs::read(ca_dir.join("ca.pem")).unwrap();
    let _ = runtime.shutdown().await;
    async fn assert_listener_closed(addr: SocketAddr) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            match tokio::time::timeout(
                Duration::from_millis(200),
                tokio::net::TcpStream::connect(addr),
            )
            .await
            {
                Ok(Err(error)) if error.kind() == std::io::ErrorKind::ConnectionRefused => return,
                Ok(Ok(stream)) => drop(stream),
                Ok(Err(_)) | Err(_) => {}
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "listener {addr} remained open"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
    assert_listener_closed(api_addr).await;
    assert_listener_closed(proxy_addr).await;

    let restarted =
        desktop_runtime::start(dir.path(), ProxyConfig::loopback(dir.path().join("ca")))
            .await
            .unwrap();
    assert_eq!(restarted.token, token);
    assert_eq!(restarted.ca_dir, ca_dir);
    let restarted_proxy: SocketAddr = restarted.proxy.parse().unwrap();
    assert!(restarted_proxy.ip().is_loopback() && restarted_proxy.port() != 0);
    assert_eq!(std::fs::read(ca_dir.join("ca.pem")).unwrap(), cert);
    let list2 = get(
        &client,
        &format!("{}/api/v1/conversations", restarted.api),
        &token,
    )
    .await;
    assert_eq!(list2, list);
    let search2 = client
        .get(format!("{}/api/v1/search?q=Alternate", restarted.api))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let search2 = json(search2).await;
    assert_eq!(search2, search);
    let detail2 = get(
        &client,
        &format!("{}/api/v1/conversations/{id}", restarted.api),
        &token,
    )
    .await;
    assert_eq!(detail2, detail);
    restarted.shutdown().await;
}
