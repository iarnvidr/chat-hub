use chat_hub::{
    config::Config,
    proxy::{CaptureEvent, ProxyConfig},
    service::{self, ServiceHandle, ServiceStatus},
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
    capture_status: chat_hub::live_capture::CaptureStatus,
    capture: mpsc::Sender<CaptureEvent>,
}

impl DesktopRuntime {
    pub fn status(&self) -> ServiceStatus {
        self.handle.status()
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

pub async fn start(
    data_dir: &Path,
    mut proxy_config: ProxyConfig,
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
    let (capture_enabled, capture_profile_scope) = Config::capture_settings_from_env()?;
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
