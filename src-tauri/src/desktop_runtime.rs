use chat_hub::{
    config::Config,
    proxy::{CaptureEvent, ProxyConfig},
    service::{self, ServiceHandle, ServiceStatus},
    store::Store,
};
use ring::rand::SecureRandom;
use std::{path::Path, time::Duration};
use tokio::sync::mpsc;

pub struct DesktopRuntime {
    pub api: String,
    pub proxy: String,
    pub token: String,
    pub ca_dir: std::path::PathBuf,
    handle: ServiceHandle,
    #[allow(dead_code)]
    pub database_path: std::path::PathBuf,
    #[allow(dead_code)]
    pub capture_recovery: serde_json::Value,
    capture_status: chat_hub::live_capture::CaptureStatus,
    capture: mpsc::Sender<CaptureEvent>,
}

impl DesktopRuntime {
    pub fn status(&self) -> ServiceStatus {
        self.handle.status()
    }

    #[allow(dead_code)] // Desktop support menu; isolated runtime tests omit the menu.
    pub fn capture_snapshot(&self) -> chat_hub::live_capture::Snapshot {
        self.capture_status.snapshot()
    }

    pub async fn shutdown(self) -> serde_json::Value {
        self.shutdown_outcome().await.0
    }

    // The packaged desktop command calls this; isolated startup tests compile
    // this module without that command in their crate graph.
    #[allow(dead_code)]
    pub async fn shutdown_for_restore(self) -> Result<(), String> {
        if self.shutdown_outcome().await.1 {
            Ok(())
        } else {
            Err("restore_shutdown_incomplete".into())
        }
    }

    async fn shutdown_outcome(self) -> (serde_json::Value, bool) {
        let Self {
            handle,
            capture,
            capture_status,
            ..
        } = self;
        drop(capture);
        let (legacy, stopped) = handle
            .shutdown_timeout_outcome(Duration::from_secs(5))
            .await;
        (
            service::shutdown_report(&legacy, capture_status.snapshot()),
            stopped,
        )
    }
}

pub async fn start(data_dir: &Path, proxy_config: ProxyConfig) -> Result<DesktopRuntime, String> {
    let (capture_enabled, capture_profile_scope) = Config::capture_settings_from_env()?;
    start_with_capture_config(
        data_dir,
        proxy_config,
        capture_enabled,
        capture_profile_scope,
    )
    .await
}

pub async fn start_with_capture_config(
    data_dir: &Path,
    mut proxy_config: ProxyConfig,
    capture_enabled: bool,
    capture_profile_scope: String,
) -> Result<DesktopRuntime, String> {
    std::fs::create_dir_all(data_dir).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(data_dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    let token = persistent_token(data_dir)?;
    let ca_dir = data_dir.join("ca");
    proxy_config.ca_dir = ca_dir.clone();
    proxy_config.bind = "127.0.0.1:0".parse().expect("valid loopback address");
    let database_path = data_dir.join("chat-hub.sqlite3");
    let config = Config {
        bind_addr: "127.0.0.1:0".parse().expect("valid loopback address"),
        bearer_token: token.clone(),
        database_path: database_path
            .to_str()
            .ok_or_else(|| "database path is not valid UTF-8".to_string())?
            .to_owned(),
        ca_dir: ca_dir.clone(),
        capture_enabled,
        capture_profile_scope,
        proxy_addr: "127.0.0.1:0".parse().expect("valid loopback address"),
    };
    // Replay only already-captured records, even when capture has since been
    // disabled. This performs no provider requests and is independent of 429s.
    // The temporary connection is dropped before starting the service writer.
    let recovery_path = config.database_path.clone();
    let recovery = tokio::task::spawn_blocking(move || {
        let mut store = Store::open_service(&recovery_path)?;
        store.recover_pending_captures()
    })
    .await;
    let capture_recovery = match recovery {
        Ok(Ok(status)) => {
            let state = if status.pending_items == 0 && status.failed == 0 {
                "complete"
            } else {
                "pending"
            };
            let mut value = serde_json::to_value(status).unwrap_or(serde_json::Value::Null);
            value["status"] = state.into();
            value
        }
        _ => serde_json::json!({"status":"failed"}),
    };
    // Failed/corrupt records remain on disk and are visible in diagnostics.
    // Do not make the existing library inaccessible merely because replay failed.
    let (handle, capture) = service::start_with_proxy(config, 64, proxy_config).await?;
    let capture_status = handle.capture_status().clone();
    let status = handle.status();
    let api = format!(
        "http://{}",
        status.api_addr.ok_or("API address unavailable")?
    );
    let proxy = status
        .proxy_addr
        .ok_or("proxy address unavailable")?
        .to_string();
    Ok(DesktopRuntime {
        api,
        proxy,
        token,
        ca_dir,
        database_path,
        capture_recovery,
        handle,
        capture_status,
        capture,
    })
}

fn persistent_token(path: &Path) -> Result<String, String> {
    use std::io::{Read, Write};
    let file = path.join("service.token");
    if let Ok(mut input) = std::fs::File::open(&file) {
        let mut token = String::new();
        input
            .read_to_string(&mut token)
            .map_err(|e| e.to_string())?;
        if token.len() >= 64 {
            return Ok(token);
        }
    }
    let mut bytes = [0u8; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "secure random number generation failed".to_string())?;
    let token = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let mut output = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&file)
        .map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    output
        .write_all(token.as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(token)
}
