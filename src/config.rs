use std::{env, net::SocketAddr, path::PathBuf};

/// Placeholder product name; keep branding centralized here.
pub const APP_NAME: &str = "chat-hub";

#[derive(Clone, Debug)]
pub struct Config {
    pub bind_addr: SocketAddr,
    pub bearer_token: String,
    pub database_path: String,
    pub ca_dir: PathBuf,
    pub proxy_addr: SocketAddr,
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
        Ok(Self {
            bind_addr,
            bearer_token,
            database_path: env::var("CHAT_HUB_DATABASE")
                .unwrap_or_else(|_| "chat-hub.sqlite3".into()),
            ca_dir: env::var_os("CHAT_HUB_CA_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("chat-hub-ca")),
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
        ] {
            env::remove_var(key);
        }
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
