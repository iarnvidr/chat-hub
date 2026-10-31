//! On-demand provider WebView embedded in the native main window.
use serde::Serialize;
use std::sync::Mutex;
use tauri::{Manager, Webview, WebviewBuilder, WebviewWindow, Wry};

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
            .on_navigation(|url| url.as_str() == "about:blank" || url.scheme() == "https")
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
            inner
                .webview
                .navigate(tauri::Url::parse("https://chatgpt.com/").map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
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
