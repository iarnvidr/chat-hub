use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

pub fn allows_origin(
    scheme: &str,
    host: Option<&str>,
    port: Option<u16>,
    username: &str,
    password: Option<&str>,
) -> bool {
    username.is_empty()
        && password.is_none()
        && port.is_none()
        && ((scheme == "tauri" && host == Some("localhost"))
            || (scheme == "http" && host == Some("tauri.localhost")))
}

#[derive(Clone)]
pub struct MainWindowOwnership(Arc<AtomicBool>);
impl MainWindowOwnership {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }
    pub fn register_after_build(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn revoke(&self) {
        self.0.store(false, Ordering::Release);
    }
    pub fn is_authorized(&self, native_label: &str) -> bool {
        native_label == "main" && self.0.load(Ordering::Acquire)
    }
}
impl Default for MainWindowOwnership {
    fn default() -> Self {
        Self::new()
    }
}

/// Provider smoke readiness accepts only the exact HTTPS provider origin.
pub fn is_provider_origin(
    scheme: &str,
    host: Option<&str>,
    port: Option<u16>,
    username: &str,
    password: Option<&str>,
) -> bool {
    scheme == "https"
        && host == Some("chatgpt.com")
        && port.is_none()
        && username.is_empty()
        && password.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_origin_table() {
        let cases = [
            ("tauri", Some("localhost"), None, "", None, true),
            ("http", Some("tauri.localhost"), None, "", None, true),
            ("https", Some("tauri.localhost"), None, "", None, false),
            ("http", Some("localhost"), None, "", None, false),
            ("tauri", Some("tauri.localhost"), None, "", None, false),
            ("http", Some("evil.localhost"), None, "", None, false),
            ("http", Some("tauri.localhost.evil"), None, "", None, false),
            ("http", Some("evil-tauri.localhost"), None, "", None, false),
            ("HTTP", Some("tauri.localhost"), None, "", None, false),
            ("http", Some("TAURI.LOCALHOST"), None, "", None, false),
            ("tauri", Some("LOCALHOST"), None, "", None, false),
            ("http", Some("tauri.localhost"), Some(80), "", None, false),
            ("http", Some("tauri.localhost"), Some(443), "", None, false),
            ("http", Some("tauri.localhost"), Some(8080), "", None, false),
            (
                "http",
                Some("tauri.localhost"),
                None,
                "attacker",
                None,
                false,
            ),
            ("http", Some("tauri.localhost"), None, "", Some("pw"), false),
            ("tauri", None, None, "", None, false),
            ("file", None, None, "", None, false),
        ];
        for (scheme, host, port, user, password, expected) in cases {
            assert_eq!(
                allows_origin(scheme, host, port, user, password),
                expected,
                "{scheme}://{host:?}:{port:?}"
            );
        }
    }
    #[test]
    fn ownership_lifecycle_fails_closed() {
        let state = MainWindowOwnership::new();
        assert!(!state.is_authorized("main"));
        assert!(!state.is_authorized("provider"));
        state.register_after_build();
        assert!(state.is_authorized("main"));
        assert!(!state.is_authorized("provider"));
        state.revoke();
        assert!(!state.is_authorized("main"));
        assert!(!state.is_authorized("provider"));
        let failed_build = MainWindowOwnership::new();
        assert!(!failed_build.is_authorized("main"));
    }
}
