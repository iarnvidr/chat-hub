//! On-demand provider WebView embedded in the native main window.
use serde::Serialize;
use std::sync::Mutex;
use tauri::{Manager, Webview, WebviewBuilder, WebviewWindow, Wry};

/// Build only the provider's own URL. Conversation IDs are canonical UUIDs;
/// callers cannot supply a URL, path, or other navigation target.
pub fn target_url(conversation_id: Option<&str>) -> Result<tauri::Url, String> {
    let path = match conversation_id {
        None => "/".to_string(),
        Some(id) if valid_conversation_id(id) => format!("/c/{id}"),
        Some(_) => return Err("invalid_conversation_id".into()),
    };
    tauri::Url::parse(&format!("https://chatgpt.com{path}"))
        .map_err(|_| "invalid_conversation_id".into())
}

fn valid_conversation_id(id: &str) -> bool {
    let parts: Vec<_> = id.split('-').collect();
    parts.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(parts)
            .all(|(len, part)| part.len() == *len && part.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn allowed_provider_navigation(url: &tauri::Url) -> bool {
    // Federated login may visit other HTTPS origins. Privileged IPC remains
    // restricted to the native main webview regardless of the provider URL.
    url.scheme() == "https" || url.as_str() == "about:blank"
}

#[derive(Clone, Serialize)]
pub struct ProviderViewStatus {
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

struct Inner {
    webview: Webview<Wry>,
    navigated: bool,
    loaded: bool,
    visible: bool,
}

pub struct ProviderViewState {
    inner: Mutex<Inner>,
    proxy_enabled: bool,
    direct_smoke_enabled: bool,
}

impl ProviderViewState {
    pub fn attach(
        app: &tauri::AppHandle,
        main: &WebviewWindow,
        proxy_url: &str,
        proxy_enabled: bool,
        direct_smoke_enabled: bool,
    ) -> Result<Webview<Wry>, String> {
        let initial_url = tauri::Url::parse("about:blank").map_err(|e| e.to_string())?;
        let app_handle = app.clone();
        let builder = WebviewBuilder::new("provider", tauri::WebviewUrl::External(initial_url))
            .on_navigation(allowed_provider_navigation)
            .on_page_load(move |_webview, payload| {
                if payload.url().scheme() != "https" {
                    return;
                }
                if let Some(state) = app_handle.try_state::<ProviderViewState>() {
                    if let Ok(mut inner) = state.inner.lock() {
                        match payload.event() {
                            tauri::webview::PageLoadEvent::Started => inner.loaded = false,
                            tauri::webview::PageLoadEvent::Finished => inner.loaded = true,
                        }
                    }
                }
            });

        #[cfg(target_os = "macos")]
        let builder = if proxy_enabled {
            builder.proxy_url(tauri::Url::parse(proxy_url).map_err(|e| e.to_string())?)
        } else {
            builder
        };
        #[cfg(not(target_os = "macos"))]
        let builder = {
            let _ = (proxy_url, proxy_enabled);
            builder
        };

        #[cfg(all(target_os = "macos", feature = "native-smoke"))]
        let builder = if crate::smoke() {
            // Never reuse the owner's persistent WKWebsiteDataStore for synthetic
            // or direct smoke navigation. This random identifier is isolated to
            // the smoke process; production keeps WebKit's normal persistent store.
            let mut identifier = [0u8; 16];
            getrandom::getrandom(&mut identifier).map_err(|e| e.to_string())?;
            builder.data_store_identifier(identifier)
        } else {
            builder
        };
        #[cfg(not(all(target_os = "macos", feature = "native-smoke")))]
        let builder = builder;

        let webview = main
            .as_ref()
            .window()
            .add_child(
                builder,
                tauri::LogicalPosition::new(0.0, 0.0),
                tauri::LogicalSize::new(1.0, 1.0),
            )
            .map_err(|e| e.to_string())?;
        webview.hide().map_err(|e| e.to_string())?;
        app.manage(Self {
            inner: Mutex::new(Inner {
                webview: webview.clone(),
                navigated: false,
                loaded: false,
                visible: false,
            }),
            proxy_enabled,
            direct_smoke_enabled,
        });
        Ok(webview)
    }

    pub fn set_view(
        &self,
        visible: bool,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        reload: bool,
    ) -> Result<ProviderViewStatus, String> {
        if ![x, y, width, height].iter().all(|n| n.is_finite())
            || (visible
                && (x < 0.0
                    || y < 0.0
                    || width < 1.0
                    || height < 1.0
                    || width > 20_000.0
                    || height > 20_000.0))
        {
            return Err("invalid provider view bounds".into());
        }

        let mut inner = self.inner.lock().map_err(|e| e.to_string())?;
        if !visible {
            inner.webview.hide().map_err(|e| e.to_string())?;
            inner.visible = false;
            return Ok(ProviderViewStatus {
                state: "hidden".into(),
                message: None,
            });
        }

        inner
            .webview
            .set_position(tauri::LogicalPosition::new(x, y))
            .map_err(|e| e.to_string())?;
        inner
            .webview
            .set_size(tauri::LogicalSize::new(width, height))
            .map_err(|e| e.to_string())?;
        if reload {
            inner.loaded = false;
            inner.navigated = true;
            inner.webview.reload().map_err(|e| e.to_string())?;
        } else if !inner.navigated {
            // The app-scoped CA delegate is installed before this first navigation.
            inner.navigated = true;
            inner
                .webview
                .navigate(tauri::Url::parse("https://chatgpt.com/").map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        }
        inner.webview.show().map_err(|e| e.to_string())?;
        inner.visible = true;
        let ready = inner.loaded;
        Ok(ProviderViewStatus {
            state: if ready { "ready" } else { "loading" }.into(),
            message: if self.proxy_enabled {
                None
            } else {
                Some("Live capture is off; ChatGPT is connected directly.".into())
            },
        })
    }

    /// Navigate only after an explicit main-window command. Calling this does
    /// not change visibility or bounds, so the UI can keep its existing layout.
    pub fn open_conversation(&self, conversation_id: Option<&str>) -> Result<(), String> {
        let target = target_url(conversation_id)?;
        let mut inner = self.inner.lock().map_err(|e| e.to_string())?;
        inner.loaded = false;
        inner.navigated = true;
        inner.webview.navigate(target).map_err(|e| e.to_string())
    }

    #[cfg(feature = "native-smoke")]
    pub fn set_smoke_view(&self, main: &Webview<Wry>) -> Result<(), String> {
        if !self.proxy_enabled && !self.direct_smoke_enabled {
            return Err("provider fixture proxy is not enabled".into());
        }
        let size = main.window().inner_size().map_err(|e| e.to_string())?;
        self.set_view(true, 0.0, 0.0, size.width as f64, size.height as f64, false)?;
        Ok(())
    }
}

#[cfg(test)]
mod target_url_tests {
    use super::{allowed_provider_navigation, target_url};

    #[test]
    fn accepts_only_chatgpt_root_or_uuid_conversation() {
        assert_eq!(target_url(None).unwrap().as_str(), "https://chatgpt.com/");
        let id = "123e4567-e89b-12d3-a456-426614174000";
        assert_eq!(
            target_url(Some(id)).unwrap().as_str(),
            format!("https://chatgpt.com/c/{id}")
        );
        for bad in ["not-a-uuid", "https://example.com", "../settings", ""] {
            assert!(target_url(Some(bad)).is_err());
        }
    }

    #[test]
    fn allows_https_login_redirects_but_rejects_local_or_script_navigation() {
        for url in [
            "about:blank",
            "https://chatgpt.com/",
            "https://login.microsoftonline.com/",
        ] {
            assert!(allowed_provider_navigation(
                &tauri::Url::parse(url).unwrap()
            ));
        }
        for url in [
            "http://chatgpt.com/",
            "file:///etc/hosts",
            "javascript:alert(1)",
            "data:text/html,hello",
        ] {
            assert!(!allowed_provider_navigation(
                &tauri::Url::parse(url).unwrap()
            ));
        }
    }
}
