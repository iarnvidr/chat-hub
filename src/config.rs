use std::{env, net::SocketAddr, path::PathBuf};

/// Placeholder product name; keep branding centralized here.
pub const APP_NAME: &str = "chat-hub";
/// Must match `src-tauri/tauri.conf.json`'s application identifier.
pub const APP_IDENTIFIER: &str = "io.github.iarnvidr.chat-hub";

/// Resolve Tauri's app-data directory from platform-specific base directories.
/// Inputs are explicit to keep path selection testable without process-global env.
pub fn app_data_dir_for(
    os: &str,
    home: Option<&std::path::Path>,
    xdg_data_home: Option<&std::path::Path>,
    appdata: Option<&std::path::Path>,
) -> Result<PathBuf, String> {
    match os {
        "macos" => home
            .map(|p| p.join("Library/Application Support").join(APP_IDENTIFIER))
            .ok_or_else(|| "home directory is unavailable".into()),
        "linux" => match xdg_data_home {
            Some(p) => Ok(p.join(APP_IDENTIFIER)),
            None => home
                .map(|p| p.join(".local/share").join(APP_IDENTIFIER))
                .ok_or_else(|| "home directory is unavailable".into()),
        },
        "windows" => appdata
            .map(|p| p.join(APP_IDENTIFIER))
            .ok_or_else(|| "APPDATA directory is unavailable".into()),
        _ => Err(format!("unsupported operating system: {os}")),
    }
}

pub fn default_data_dir() -> Result<PathBuf, String> {
    let home = env::var_os("HOME").map(PathBuf::from);
    let xdg = env::var_os("XDG_DATA_HOME").map(PathBuf::from);
    let appdata = env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let os = "macos";
    #[cfg(target_os = "linux")]
    let os = "linux";
    #[cfg(target_os = "windows")]
    let os = "windows";
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let os = "unsupported";
    app_data_dir_for(os, home.as_deref(), xdg.as_deref(), appdata.as_deref())
}

#[derive(Clone)]
pub struct Config {
    pub bind_addr: SocketAddr,
    pub bearer_token: String,
    pub database_path: String,
    pub ca_dir: PathBuf,
    pub proxy_addr: SocketAddr,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("bind_addr", &self.bind_addr)
            .field("bearer_token", &"[REDACTED]")
            .field("database_path", &self.database_path)
            .field("ca_dir", &self.ca_dir)
            .field("proxy_addr", &self.proxy_addr)
            .finish()
    }
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let bind_addr: SocketAddr = env::var("CHAT_HUB_BIND")
            .unwrap_or_else(|_| "127.0.0.1:0".into())
            .parse()
            .map_err(|e| format!("invalid CHAT_HUB_BIND: {e}"))?;
        if !bind_addr.ip().is_loopback() {
            return Err("CHAT_HUB_BIND must use a loopback address".into());
        }
        let proxy_addr: SocketAddr = env::var("CHAT_HUB_PROXY_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:8788".into())
            .parse()
            .map_err(|e| format!("invalid CHAT_HUB_PROXY_ADDR: {e}"))?;
        if !proxy_addr.ip().is_loopback() {
            return Err("CHAT_HUB_PROXY_ADDR must use a loopback address".into());
        }
        let bearer_token =
            env::var("CHAT_HUB_TOKEN").map_err(|_| "CHAT_HUB_TOKEN is required".to_string())?;
        if bearer_token.is_empty() {
            return Err("CHAT_HUB_TOKEN must not be empty".into());
        }
        let data_dir = env::var_os("CHAT_HUB_DATA_DIR")
            .map(PathBuf::from)
            .map(Ok)
            .unwrap_or_else(default_data_dir)?;
        Ok(Self {
            bind_addr,
            bearer_token,
            database_path: env::var("CHAT_HUB_DATABASE").unwrap_or_else(|_| {
                data_dir
                    .join("chat-hub.sqlite3")
                    .to_string_lossy()
                    .into_owned()
            }),
            ca_dir: env::var_os("CHAT_HUB_CA_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| data_dir.join("ca")),
            proxy_addr,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn clear() {
        for key in [
            "CHAT_HUB_BIND",
            "CHAT_HUB_PROXY_ADDR",
            "CHAT_HUB_TOKEN",
            "CHAT_HUB_DATABASE",
            "CHAT_HUB_CA_DIR",
            "CHAT_HUB_DATA_DIR",
        ] {
            env::remove_var(key);
        }
    }

    #[test]
    fn debug_redacts_bearer_token_and_preserves_other_fields() {
        let c = Config {
            bind_addr: "127.0.0.1:1".parse().unwrap(),
            bearer_token: "sensitive-marker".into(),
            database_path: "/tmp/db".into(),
            ca_dir: PathBuf::from("/tmp/ca"),
            proxy_addr: "127.0.0.1:2".parse().unwrap(),
        };
        let debug = format!("{c:?}");
        assert!(!debug.contains("sensitive-marker"));
        assert!(debug.contains("[REDACTED]"));
        assert!(debug.contains("127.0.0.1:1"));
    }

    #[test]
    fn app_data_paths_follow_platform_conventions() {
        let home = PathBuf::from("/home/test");
        assert_eq!(
            app_data_dir_for("macos", Some(&home), None, None).unwrap(),
            PathBuf::from("/home/test/Library/Application Support/io.github.iarnvidr.chat-hub")
        );
        assert_eq!(
            app_data_dir_for("linux", Some(&home), None, None).unwrap(),
            PathBuf::from("/home/test/.local/share/io.github.iarnvidr.chat-hub")
        );
        assert_eq!(
            app_data_dir_for(
                "linux",
                Some(&home),
                Some(std::path::Path::new("/data")),
                None
            )
            .unwrap(),
            PathBuf::from("/data/io.github.iarnvidr.chat-hub")
        );
        assert_eq!(
            app_data_dir_for(
                "windows",
                None,
                None,
                Some(std::path::Path::new("C:/Users/test/AppData/Roaming"))
            )
            .unwrap(),
            PathBuf::from("C:/Users/test/AppData/Roaming/io.github.iarnvidr.chat-hub")
        );
        assert!(app_data_dir_for("linux", None, None, None).is_err());
    }

    #[test]
    fn requires_nonempty_token_and_defaults_loopback_addresses() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear();
        assert!(Config::from_env()
            .unwrap_err()
            .contains("CHAT_HUB_TOKEN is required"));
        env::set_var("CHAT_HUB_TOKEN", "test-token");
        let c = Config::from_env().unwrap();
        assert_eq!(c.bind_addr, "127.0.0.1:0".parse::<SocketAddr>().unwrap());
        assert_eq!(
            c.proxy_addr,
            "127.0.0.1:8788".parse::<SocketAddr>().unwrap()
        );
        clear();
    }

    #[test]
    fn rejects_non_loopback_addresses_and_empty_token() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear();
        env::set_var("CHAT_HUB_TOKEN", "test-token");
        env::set_var("CHAT_HUB_BIND", "0.0.0.0:8080");
        assert!(Config::from_env()
            .unwrap_err()
            .contains("CHAT_HUB_BIND must use a loopback address"));
        env::remove_var("CHAT_HUB_BIND");
        env::set_var("CHAT_HUB_PROXY_ADDR", "0.0.0.0:8788");
        assert!(Config::from_env()
            .unwrap_err()
            .contains("CHAT_HUB_PROXY_ADDR must use a loopback address"));
        env::remove_var("CHAT_HUB_PROXY_ADDR");
        env::set_var("CHAT_HUB_TOKEN", "");
        assert!(Config::from_env()
            .unwrap_err()
            .contains("CHAT_HUB_TOKEN must not be empty"));
        clear();
    }
}
