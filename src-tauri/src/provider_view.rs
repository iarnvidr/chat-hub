//! On-demand provider WebView embedded in the native main window.
use ring::rand::SecureRandom;
use serde::Serialize;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatGptConnectionState {
    NotOpen,
    Loading,
    Connected,
    SignedOut,
    Unavailable,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ChatGptConnectionStatus {
    pub state: ChatGptConnectionState,
    pub message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_scope: Option<String>,
}

impl ChatGptConnectionStatus {
    fn new(state: ChatGptConnectionState) -> Self {
        let message = match state {
            ChatGptConnectionState::NotOpen => "Open ChatGPT to check your connection.",
            ChatGptConnectionState::Loading => "ChatGPT is still loading.",
            ChatGptConnectionState::Connected => "Connected to ChatGPT.",
            ChatGptConnectionState::SignedOut => "Sign in to ChatGPT to continue.",
            ChatGptConnectionState::Unavailable => {
                "ChatGPT connection could not be checked. Try again."
            }
        };
        Self {
            state,
            message,
            account_scope: None,
        }
    }

    fn connected(account_scope: Option<String>) -> Self {
        let mut status = Self::new(ChatGptConnectionState::Connected);
        status.account_scope = account_scope;
        status
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ProbePoll {
    Pending,
    Complete(ChatGptConnectionState, Option<String>),
}

fn valid_observed_scope(scope: &str) -> bool {
    scope
        .strip_prefix("chatgpt:observed:")
        .is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
}

fn exact_chatgpt_origin(url: &tauri::Url) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some("chatgpt.com")
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
}

fn current_chatgpt_conversation_id(url: &tauri::Url) -> Option<String> {
    if !exact_chatgpt_origin(url) || url.query().is_some() || url.fragment().is_some() {
        return None;
    }
    let id = url.path().strip_prefix("/c/")?;
    if !valid_conversation_id(id) || id.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return None;
    }
    Some(id.to_owned())
}

#[derive(Clone, Debug, Default)]
struct PageMetadata {
    exact_origin: bool,
    conversation_id: Option<String>,
    account_scope: Option<String>,
    navigation_generation: u64,
    next_sequence: u64,
    applied_sequence: u64,
}

fn parse_page_metadata_callback(raw: &str, expected_nonce: &str) -> Option<(bool, Option<String>)> {
    let mut value: serde_json::Value = serde_json::from_str(raw).ok()?;
    for _ in 0..3 {
        let Some(nested) = value.as_str() else { break };
        value = serde_json::from_str(nested).ok()?;
    }
    let object = value.as_object()?;
    if object.len() != 3 || object.get("nonce")?.as_str()? != expected_nonce {
        return None;
    }
    let exact_origin = object.get("exact_origin")?.as_bool()?;
    let conversation_id = match object.get("conversation_id")? {
        serde_json::Value::Null => None,
        serde_json::Value::String(id)
            if exact_origin
                && valid_conversation_id(id)
                && id.bytes().all(|byte| !byte.is_ascii_uppercase()) =>
        {
            Some(id.clone())
        }
        _ => return None,
    };
    Some((exact_origin, conversation_id))
}

fn page_metadata_script(nonce: &str) -> Option<String> {
    let nonce_json = serde_json::to_string(nonce).ok()?;
    Some(format!(
        "({})({nonce_json})",
        include_str!("provider_page_metadata.js")
    ))
}

fn request_page_metadata(webview: &Webview<Wry>, page: &Arc<Mutex<PageMetadata>>) {
    let Some(nonce) = fresh_probe_nonce() else {
        return;
    };
    let (generation, sequence) = match page.lock() {
        Ok(mut metadata) => {
            metadata.next_sequence = metadata.next_sequence.saturating_add(1);
            (metadata.navigation_generation, metadata.next_sequence)
        }
        Err(_) => return,
    };
    let Some(script) = page_metadata_script(&nonce) else {
        return;
    };
    let page = page.clone();
    let _ = webview.eval_with_callback(script, move |value| {
        let Some((exact_origin, conversation_id)) = parse_page_metadata_callback(&value, &nonce)
        else {
            return;
        };
        if let Ok(mut metadata) = page.lock() {
            if metadata.navigation_generation == generation && sequence >= metadata.applied_sequence
            {
                metadata.exact_origin = exact_origin;
                metadata.conversation_id = conversation_id;
                if !exact_origin {
                    metadata.account_scope = None;
                }
                metadata.applied_sequence = sequence;
            }
        }
    });
}

fn parse_probe_callback(raw: &str, expected_nonce: &str) -> Option<ProbePoll> {
    let mut value: serde_json::Value = serde_json::from_str(raw).ok()?;
    for _ in 0..3 {
        let Some(nested) = value.as_str() else {
            break;
        };
        value = serde_json::from_str(nested).ok()?;
    }
    let object = value.as_object()?;
    if !(2..=3).contains(&object.len()) {
        return None;
    }
    if object.get("nonce")?.as_str()? != expected_nonce {
        return None;
    }
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "nonce" | "state" | "account_scope"))
    {
        return None;
    }
    match object.get("state")?.as_str()? {
        "checking" if object.len() == 2 => Some(ProbePoll::Pending),
        "connected" => {
            if object.len() == 3 && !object.contains_key("account_scope") {
                return None;
            }
            let scope = match object.get("account_scope") {
                None | Some(serde_json::Value::Null) => None,
                Some(serde_json::Value::String(scope)) if valid_observed_scope(scope) => {
                    Some(scope.clone())
                }
                _ => return None,
            };
            Some(ProbePoll::Complete(
                ChatGptConnectionState::Connected,
                scope,
            ))
        }
        "signed_out" if object.len() == 2 => {
            Some(ProbePoll::Complete(ChatGptConnectionState::SignedOut, None))
        }
        "unavailable" if object.len() == 2 => Some(ProbePoll::Complete(
            ChatGptConnectionState::Unavailable,
            None,
        )),
        _ => None,
    }
}

fn fresh_probe_nonce() -> Option<String> {
    let mut bytes = [0u8; 16];
    ring::rand::SystemRandom::new().fill(&mut bytes).ok()?;
    Some(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn cleanup_probe(webview: &Webview<Wry>, key_json: &str) {
    let _ = webview.eval(&format!("delete window[{key_json}]"));
}

struct Inner {
    webview: Webview<Wry>,
    navigated: bool,
    loaded: bool,
    visible: bool,
}

pub struct ProviderViewState {
    inner: Mutex<Inner>,
    page: Arc<Mutex<PageMetadata>>,
    proxy_enabled: bool,
    direct_smoke_enabled: bool,
}

impl ProviderViewState {
    pub(crate) fn history_webview(&self) -> Result<Webview<Wry>, String> {
        let (webview, loaded) = {
            let inner = self
                .inner
                .lock()
                .map_err(|_| "provider_unavailable".to_string())?;
            (inner.webview.clone(), inner.loaded)
        };
        let page = self
            .page
            .lock()
            .map_err(|_| "provider_unavailable".to_string())?;
        if !page.exact_origin || !loaded {
            return Err("provider_not_ready".into());
        }
        Ok(webview)
    }

    pub fn attach(
        app: &tauri::AppHandle,
        main: &WebviewWindow,
        proxy_url: &str,
        proxy_enabled: bool,
        direct_smoke_enabled: bool,
    ) -> Result<Webview<Wry>, String> {
        let initial_url = tauri::Url::parse("about:blank").map_err(|e| e.to_string())?;
        let app_handle = app.clone();
        let page = Arc::new(Mutex::new(PageMetadata::default()));
        let page_for_load = page.clone();
        let builder = WebviewBuilder::new("provider", tauri::WebviewUrl::External(initial_url))
            .on_navigation(allowed_provider_navigation)
            .on_page_load(move |_webview, payload| {
                let event = payload.event();
                if payload.url().scheme() != "https" {
                    if let Ok(mut metadata) = page_for_load.lock() {
                        if event == tauri::webview::PageLoadEvent::Started {
                            metadata.navigation_generation =
                                metadata.navigation_generation.saturating_add(1);
                            metadata.account_scope = None;
                            metadata.next_sequence = 0;
                            metadata.applied_sequence = 0;
                        }
                        metadata.exact_origin = false;
                        metadata.conversation_id = None;
                        metadata.account_scope = None;
                    }
                    if let Some(state) = app_handle.try_state::<ProviderViewState>() {
                        if let Ok(mut inner) = state.inner.lock() {
                            inner.loaded = event == tauri::webview::PageLoadEvent::Finished;
                        }
                    }
                    return;
                }
                if let Ok(mut metadata) = page_for_load.lock() {
                    if event == tauri::webview::PageLoadEvent::Started {
                        metadata.navigation_generation =
                            metadata.navigation_generation.saturating_add(1);
                        metadata.next_sequence = 0;
                        metadata.applied_sequence = 0;
                    }
                    metadata.exact_origin = exact_chatgpt_origin(payload.url());
                    metadata.conversation_id = current_chatgpt_conversation_id(payload.url());
                }
                if let Some(state) = app_handle.try_state::<ProviderViewState>() {
                    if let Ok(mut inner) = state.inner.lock() {
                        match event {
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
            page,
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
            let conversation_id = self
                .page
                .lock()
                .ok()
                .and_then(|page| page.conversation_id.clone());
            return Ok(ProviderViewStatus {
                state: "hidden".into(),
                message: None,
                conversation_id,
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
        let status = ProviderViewStatus {
            state: if ready { "ready" } else { "loading" }.into(),
            message: if self.proxy_enabled {
                None
            } else {
                Some("Live capture is off; ChatGPT is connected directly.".into())
            },
            conversation_id: self
                .page
                .lock()
                .ok()
                .and_then(|page| page.conversation_id.clone()),
        };
        let webview = inner.webview.clone();
        drop(inner);
        request_page_metadata(&webview, &self.page);
        Ok(status)
    }

    /// Navigate only after an explicit main-window command. Calling this does
    /// not change visibility or bounds, so the UI can keep its existing layout.
    pub fn open_conversation(&self, conversation_id: Option<&str>) -> Result<(), String> {
        let target = target_url(conversation_id)?;
        if let Ok(mut page) = self.page.lock() {
            page.navigation_generation = page.navigation_generation.saturating_add(1);
            page.account_scope = None;
            page.next_sequence = 0;
            page.applied_sequence = 0;
            page.exact_origin = true;
            page.conversation_id = current_chatgpt_conversation_id(&target);
        }
        let mut inner = self.inner.lock().map_err(|e| e.to_string())?;
        inner.loaded = false;
        inner.navigated = true;
        inner.webview.navigate(target).map_err(|e| e.to_string())
    }

    /// Check the current ChatGPT web session without navigating or returning
    /// credentials, account details, or provider response data to native code.
    pub async fn chatgpt_connection(&self) -> ChatGptConnectionStatus {
        let (webview, loaded) = match self.inner.lock() {
            Ok(inner) => (inner.webview.clone(), inner.loaded),
            Err(_) => return ChatGptConnectionStatus::new(ChatGptConnectionState::Unavailable),
        };
        let (page_is_chatgpt, page_generation) = match self.page.lock() {
            Ok(page) => (page.exact_origin, page.navigation_generation),
            Err(_) => return ChatGptConnectionStatus::new(ChatGptConnectionState::Unavailable),
        };
        if !page_is_chatgpt {
            return ChatGptConnectionStatus::new(ChatGptConnectionState::NotOpen);
        }
        if !loaded {
            return ChatGptConnectionStatus::new(ChatGptConnectionState::Loading);
        }

        let Some(nonce) = fresh_probe_nonce() else {
            return ChatGptConnectionStatus::new(ChatGptConnectionState::Unavailable);
        };
        let key = format!("__chatHubSessionProbe_{nonce}");
        let Ok(key_json) = serde_json::to_string(&key) else {
            return ChatGptConnectionStatus::new(ChatGptConnectionState::Unavailable);
        };
        let Ok(nonce_json) = serde_json::to_string(&nonce) else {
            return ChatGptConnectionStatus::new(ChatGptConnectionState::Unavailable);
        };
        let start_probe = format!(
            r#"(()=>{{const k={key_json};const n={nonce_json};window[k]={{nonce:n,state:"checking"}};(async()=>{{try{{const response=await fetch("/api/auth/session",{{credentials:"include",cache:"no-store"}});if(!response.ok){{window[k]={{nonce:n,state:response.status===401||response.status===403?"signed_out":"unavailable"}};return}}const session=await response.json();const token=session&&session.accessToken;if(typeof token!=="string"||!session.user){{window[k]={{nonce:n,state:"signed_out"}};return}}let scope=null;try{{const part=token.split(".")[1];if(!part||part.length>16000)throw 0;const claims=JSON.parse(atob(part.replace(/-/g,"+").replace(/_/g,"/")+"=".repeat((4-part.length%4)%4)));const auth=claims["https://api.openai.com/auth"];const account=auth?.chatgpt_account_id;let user=auth?.chatgpt_user_id;if(user==null)user=auth?.user_id;const valid=v=>typeof v==="string"&&v.length>0&&v.length<=256&&/^[A-Za-z0-9_:-]+$/.test(v);if(valid(account)&&valid(user)){{const bytes=new TextEncoder().encode(JSON.stringify(["chatgpt-request-identity-v1",account,user]));const digest=await crypto.subtle.digest("SHA-256",bytes);scope="chatgpt:observed:"+Array.from(new Uint8Array(digest),b=>b.toString(16).padStart(2,"0")).join("")}}}}catch(_){{scope=null}}window[k]={{nonce:n,state:"connected",account_scope:scope}}}}catch(_){{window[k]={{nonce:n,state:"unavailable"}}}}}})()}})()"#
        );
        if webview.eval(&start_probe).is_err() {
            return ChatGptConnectionStatus::new(ChatGptConnectionState::Unavailable);
        }

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                cleanup_probe(&webview, &key_json);
                return ChatGptConnectionStatus::new(ChatGptConnectionState::Unavailable);
            }
            let (tx, rx) = tokio::sync::oneshot::channel();
            let tx = Mutex::new(Some(tx));
            let poll = format!("JSON.stringify(window[{key_json}]||null)");
            if webview
                .eval_with_callback(poll, move |value| {
                    if let Ok(mut slot) = tx.lock() {
                        if let Some(tx) = slot.take() {
                            let _ = tx.send(value);
                        }
                    }
                })
                .is_err()
            {
                cleanup_probe(&webview, &key_json);
                return ChatGptConnectionStatus::new(ChatGptConnectionState::Unavailable);
            }
            let wait = remaining.min(Duration::from_secs(2));
            match tokio::time::timeout(wait, rx).await {
                Ok(Ok(value)) => match parse_probe_callback(&value, &nonce) {
                    Some(ProbePoll::Pending) => {}
                    Some(ProbePoll::Complete(state, account_scope)) => {
                        cleanup_probe(&webview, &key_json);
                        return if state == ChatGptConnectionState::Connected {
                            let scoped = self.page.lock().ok().and_then(|mut page| {
                                if page.navigation_generation != page_generation
                                    || !page.exact_origin
                                {
                                    return None;
                                }
                                page.account_scope = account_scope.clone();
                                page.account_scope.clone()
                            });
                            ChatGptConnectionStatus::connected(scoped)
                        } else {
                            ChatGptConnectionStatus::new(state)
                        };
                    }
                    None => {
                        cleanup_probe(&webview, &key_json);
                        return ChatGptConnectionStatus::new(ChatGptConnectionState::Unavailable);
                    }
                },
                Ok(Err(_)) => {
                    cleanup_probe(&webview, &key_json);
                    return ChatGptConnectionStatus::new(ChatGptConnectionState::Unavailable);
                }
                Err(_) => {}
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
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
    use super::{
        allowed_provider_navigation, current_chatgpt_conversation_id, exact_chatgpt_origin,
        page_metadata_script, parse_page_metadata_callback, parse_probe_callback, target_url,
        valid_observed_scope, ChatGptConnectionState, ProbePoll,
    };

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

    #[test]
    fn probe_requires_exact_chatgpt_https_origin() {
        for url in ["https://chatgpt.com/", "https://chatgpt.com/c/abc"] {
            assert!(exact_chatgpt_origin(&tauri::Url::parse(url).unwrap()));
        }
        for url in [
            "about:blank",
            "http://chatgpt.com/",
            "https://chatgpt.com.evil.test/",
            "https://sub.chatgpt.com/",
            "https://user@chatgpt.com/",
            "https://login.microsoftonline.com/",
        ] {
            assert!(
                !exact_chatgpt_origin(&tauri::Url::parse(url).unwrap()),
                "{url}"
            );
        }
    }

    #[test]
    fn current_conversation_id_is_only_the_exact_canonical_chatgpt_uuid_path() {
        let id = "123e4567-e89b-12d3-a456-426614174000";
        assert_eq!(
            current_chatgpt_conversation_id(
                &tauri::Url::parse(&format!("https://chatgpt.com/c/{id}")).unwrap()
            ),
            Some(id.to_owned())
        );
        for url in [
            "https://chatgpt.com/",
            "https://chatgpt.com/c/not-a-uuid",
            "https://chatgpt.com/c/123E4567-E89B-12D3-A456-426614174000",
            "https://chatgpt.com/c/123e4567-e89b-12d3-a456-426614174000/extra",
            "https://chatgpt.com/c/123e4567-e89b-12d3-a456-426614174000?query=private",
            "https://chatgpt.com/c/123e4567-e89b-12d3-a456-426614174000#fragment",
            "https://chatgpt.com.evil.test/c/123e4567-e89b-12d3-a456-426614174000",
        ] {
            assert_eq!(
                current_chatgpt_conversation_id(&tauri::Url::parse(url).unwrap()),
                None,
                "{url}"
            );
        }
    }

    #[test]
    fn page_metadata_callback_returns_only_nonce_bound_origin_and_uuid() {
        let id = "123e4567-e89b-12d3-a456-426614174000";
        let safe = serde_json::json!({
            "nonce": "test-nonce", "exact_origin": true, "conversation_id": id
        })
        .to_string();
        assert_eq!(
            parse_page_metadata_callback(&safe, "test-nonce"),
            Some((true, Some(id.to_owned())))
        );
        let nested = serde_json::to_string(&safe).unwrap();
        assert_eq!(
            parse_page_metadata_callback(&nested, "test-nonce"),
            Some((true, Some(id.to_owned())))
        );
        let root = serde_json::json!({
            "nonce": "test-nonce", "exact_origin": true, "conversation_id": null
        })
        .to_string();
        assert_eq!(
            parse_page_metadata_callback(&root, "test-nonce"),
            Some((true, None))
        );
        for raw in [
            r#"{"nonce":"wrong","exact_origin":true,"conversation_id":null}"#,
            r#"{"nonce":"test-nonce","exact_origin":false,"conversation_id":"123e4567-e89b-12d3-a456-426614174000"}"#,
            r#"{"nonce":"test-nonce","exact_origin":true,"conversation_id":"invalid"}"#,
            r#"{"nonce":"test-nonce","exact_origin":true,"conversation_id":null,"url":"https://chatgpt.com/c/private"}"#,
        ] {
            assert_eq!(parse_page_metadata_callback(raw, "test-nonce"), None);
        }
    }

    #[test]
    fn page_location_probe_returns_only_safe_metadata_and_checks_exact_uuid_route() {
        let script = page_metadata_script("safe-nonce").unwrap();
        assert!(script.contains("new URL(location.href)"));
        assert!(script.contains("hostname === 'chatgpt.com'"));
        assert!(script.contains("conversation_id: conversationId"));
        assert!(!script.contains("accessToken"));
        assert!(!script.contains("title"));
    }

    #[test]
    fn parses_only_nonce_bound_status_from_callback_shapes() {
        let nonce = "001122";
        let signed_out = serde_json::json!({"nonce": nonce, "state": "signed_out"}).to_string();
        let signed_out_nested = serde_json::to_string(&signed_out).unwrap();
        let signed_out_deep_nested = serde_json::to_string(&signed_out_nested).unwrap();
        for (raw, expected) in [
            (
                r#"{"nonce":"001122","state":"checking"}"#,
                ProbePoll::Pending,
            ),
            (
                r#""{\"nonce\":\"001122\",\"state\":\"connected\"}""#,
                ProbePoll::Complete(ChatGptConnectionState::Connected, None),
            ),
            (
                r#"{"nonce":"001122","state":"unavailable"}"#,
                ProbePoll::Complete(ChatGptConnectionState::Unavailable, None),
            ),
        ] {
            assert_eq!(parse_probe_callback(raw, nonce), Some(expected));
        }
        assert_eq!(
            parse_probe_callback(&signed_out_deep_nested, nonce),
            Some(ProbePoll::Complete(ChatGptConnectionState::SignedOut, None))
        );
        for raw in [
            r#"{"nonce":"wrong","state":"connected"}"#,
            r#"{"nonce":"001122","state":"connected","accessToken":"secret"}"#,
            r#"{"nonce":"001122","state":"unknown"}"#,
            "not-json",
        ] {
            assert_eq!(parse_probe_callback(raw, nonce), None);
        }
    }

    #[test]
    fn connection_probe_accepts_only_opaque_observed_scope() {
        let scope = format!("chatgpt:observed:{}", "a".repeat(64));
        let raw = serde_json::json!({"nonce":"001122", "state":"connected", "account_scope":scope})
            .to_string();
        assert_eq!(
            parse_probe_callback(&raw, "001122"),
            Some(ProbePoll::Complete(
                ChatGptConnectionState::Connected,
                Some(format!("chatgpt:observed:{}", "a".repeat(64))),
            ))
        );
        assert!(!valid_observed_scope("chatgpt:observed:secret"));
        assert!(parse_probe_callback(
            r#"{"nonce":"001122","state":"connected","account_scope":"raw-account-id"}"#,
            "001122"
        )
        .is_none());
        assert!(parse_probe_callback(
            r#"{"nonce":"001122","state":"signed_out","account_scope":"chatgpt:observed:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
            "001122"
        ).is_none());
    }
}
