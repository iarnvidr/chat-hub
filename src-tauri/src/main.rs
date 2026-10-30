use chat_hub::{config::APP_NAME, proxy::ProxyConfig};
use serde::Serialize;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;
mod crash_diagnostics;
mod desktop_api;
mod desktop_runtime;
mod service_health;
use service_health::healthy_response;
mod window_policy;
use window_policy::MainWindowOwnership;
#[cfg(target_os = "macos")]
mod trust_mac;

#[cfg(target_os = "macos")]
struct ProviderProxy {
    url: String,
    ca_der: Vec<u8>,
}
#[derive(Clone)]
struct AppState {
    updater: Arc<Mutex<UpdateStatus>>,
    pending_update: Arc<Mutex<Option<(tauri_plugin_updater::Update, Vec<u8>)>>>,
    token: String,
    api: String,
    proxy: String,
    runtime: Arc<Mutex<Option<desktop_runtime::DesktopRuntime>>>,
    shutdown_started: Arc<AtomicBool>,
    shutdown_finished: Arc<AtomicBool>,
}
#[derive(Clone, Serialize)]
struct UpdateStatus {
    version: String,
    channel: String,
    status: String,
    message: String,
}
#[allow(dead_code)] // Used only when the native-smoke feature is enabled.
fn smoke() -> bool {
    cfg!(all(feature = "native-smoke", debug_assertions))
        && std::env::args().any(|a| a == "--native-ui-smoke")
        && std::env::args().any(|a| a == "--updater-smoke")
        && std::env::var_os("MUNINN_NATIVE_UI_SMOKE").is_some()
}
#[cfg(feature = "native-smoke")]
mod e2e {
    use serde_json::{json, Value};
    use std::{path::PathBuf, time::Duration};
    use tauri::Manager;
    fn dir() -> Option<PathBuf> {
        if !cfg!(debug_assertions) || !super::smoke() {
            return None;
        }
        std::env::var_os("MUNINN_E2E_DIR").map(PathBuf::from)
    }
    fn respond(d: &std::path::Path, id: &str, v: &Value) {
        let p = d.join(format!("{id}.res.json"));
        let t = d.join(format!("{id}.res.json.tmp"));
        if let Err(e) = std::fs::write(&t, v.to_string()) {
            eprintln!("native E2E response write failed id={id}: {e}");
            return;
        }
        if let Err(e) = std::fs::rename(&t, &p) {
            eprintln!("native E2E response publish failed id={id}: {e}");
            let _ = std::fs::remove_file(t);
        }
    }
    fn progress(d: &std::path::Path, id: &str, stage: &str, detail: Value) {
        let value = json!({"id":id,"stage":stage,"detail":detail});
        let p = d.join(format!("{id}.diag.json"));
        let t = d.join(format!("{id}.diag.json.tmp"));
        if std::fs::write(&t, value.to_string()).is_ok() {
            let _ = std::fs::rename(t, p);
        }
    }
    pub fn start(app: tauri::AppHandle) {
        let Some(d) = dir() else { return };
        let _ = std::fs::create_dir_all(&d);
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(200));
            let Ok(entries) = std::fs::read_dir(&d) else {
                continue;
            };
            let mut paths: Vec<_> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.to_string_lossy().ends_with(".cmd.json"))
                .collect();
            paths.sort();
            for p in paths {
                let text = std::fs::read_to_string(&p).unwrap_or_default();
                let _ = std::fs::remove_file(&p);
                let Ok(c) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                let id = c["id"].as_str().unwrap_or("").to_string();
                if id.is_empty()
                    || !id
                        .chars()
                        .all(|x| x.is_ascii_alphanumeric() || x == '-' || x == '_')
                {
                    continue;
                }
                progress(&d, &id, "dispatch", json!({"op":c["op"]}));
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run(&app, &c, &d, &id)
                }))
                .unwrap_or_else(|_| json!({"ok":false,"error":"native E2E operation panicked"}));
                progress(&d, &id, "response", json!({"ok":result["ok"]}));
                respond(&d, &id, &result);
            }
        });
    }
    fn run(app: &tauri::AppHandle, c: &Value, d: &std::path::Path, id: &str) -> Value {
        match c["op"].as_str().unwrap_or("") {
            "provider_denial" => {
                let w = app.get_webview_window("provider");
                let Some(w) = w else {
                    return json!({"ok":false,"error":"provider webview missing"});
                };
                if !super::smoke() {
                    return json!({"ok":false,"error":"native smoke automation disabled"});
                }
                // The provider webview is created during setup and may not yet
                // have committed its initial navigation when the driver asks.
                // Wait for the real expected origin and a live JS context, bounded
                // so an actual navigation failure has useful diagnostics.
                // Keep the entire two-phase operation inside the Python driver's 40s IPC
                // deadline, leaving time to publish a response even on a stalled WebView.
                let operation_deadline = std::time::Instant::now() + Duration::from_secs(28);
                let ready_deadline =
                    operation_deadline.min(std::time::Instant::now() + Duration::from_secs(10));
                let mut readiness = json!({"href":null,"readyState":null,"windowLabel":w.label()});
                let mut readiness_polls = 0u64;
                loop {
                    readiness_polls += 1;
                    progress(
                        d,
                        id,
                        "provider_readiness",
                        json!({"polls":readiness_polls,"href":readiness["href"],"readyState":readiness["readyState"]}),
                    );
                    let (tx, rx) = std::sync::mpsc::channel();
                    if w.eval_with_callback("JSON.stringify({href:location.href,readyState:document.readyState,label:window.__TAURI_INTERNALS__?'tauri':'no-tauri'})", move |v| { let _ = tx.send(v); }).is_ok() {
                        if let Ok(Ok(value)) = rx.recv_timeout(Duration::from_secs(1)).map(|v| serde_json::from_str::<Value>(&v)) {
                            if let Some(parsed) = value.as_str().and_then(|s| serde_json::from_str::<Value>(s).ok()) {
                                readiness = parsed;
                                if readiness["href"].as_str().is_some_and(|h| tauri::Url::parse(h).ok().is_some_and(|u| crate::window_policy::is_provider_origin(u.scheme(), u.host_str(), u.port(), u.username(), u.password()))) && readiness["readyState"].as_str().is_some() {
                                    break;
                                }
                            }
                        }
                    }
                    if std::time::Instant::now() >= ready_deadline {
                        return json!({"ok":false,"error":"provider webview readiness timed out","readiness":readiness,"polls":readiness_polls,"deadline_ms":28000});
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
                progress(
                    d,
                    id,
                    "provider_probe_started",
                    json!({"href":readiness["href"],"readyState":readiness["readyState"]}),
                );
                let nonce = format!(
                    "{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos()
                );
                let key = format!("__chatHubDenialProbe_{}", nonce);
                let escaped_key =
                    serde_json::to_string(&key).unwrap_or_else(|_| "\"invalid\"".into());
                let start = format!("(()=>{{const k={escaped_key};const invoke=window.__TAURI_INTERNALS__?.invoke;if(typeof invoke!==\"function\"){{window[k]={{nonce:{escaped_key},attempted:false,error:\"invoke unavailable\"}};return}};window[k]={{nonce:{escaped_key},pending:true}};Promise.resolve().then(()=>invoke(\"service_info\")).then(()=>{{window[k]={{nonce:{escaped_key},attempted:true,denied:false}}}},e=>{{window[k]={{nonce:{escaped_key},attempted:true,denied:true,error:String(e).slice(0,120)}}}})}})()");
                if let Err(e) = w.eval(start) {
                    return json!({"ok":false,"error":format!("provider probe start failed: {e}")});
                }
                let deadline = operation_deadline;
                let mut probe_polls = 0u64;
                loop {
                    probe_polls += 1;
                    progress(
                        d,
                        id,
                        "provider_probe_poll",
                        json!({"polls":probe_polls,"remaining_ms":deadline.saturating_duration_since(std::time::Instant::now()).as_millis()}),
                    );
                    let (tx, rx) = std::sync::mpsc::channel();
                    let poll = format!("window[{escaped_key}]||null");
                    if let Err(e) = w.eval_with_callback(poll, move |value| {
                        let _ = tx.send(value);
                    }) {
                        return json!({"ok":false,"error":format!("provider probe poll failed: {e}")});
                    }
                    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                    match rx.recv_timeout(remaining.min(Duration::from_millis(500))) {
                        Ok(value) => {
                            let decoded = serde_json::from_str::<Value>(&value).ok();
                            let result = decoded.as_ref().and_then(|v| {
                                v.as_str()
                                    .and_then(|s| serde_json::from_str::<Value>(s).ok())
                                    .or_else(|| Some(v.clone()))
                            });
                            if let Some(result) = result {
                                if result["nonce"].as_str() != Some(&key) {
                                    continue;
                                }
                                if result["attempted"].as_bool() == Some(false) {
                                    return json!({"ok":false,"error":result["error"].as_str().unwrap_or("invoke unavailable")});
                                }
                                if result["attempted"].as_bool() == Some(true) {
                                    let denied = result["denied"].as_bool() == Some(true);
                                    return json!({"ok":denied,"denied":denied,"category":result["error"].as_str().unwrap_or("")});
                                }
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        Err(_) => {
                            return json!({"ok":false,"error":"provider probe callback disconnected"})
                        }
                    }
                    if std::time::Instant::now() >= deadline {
                        return json!({"ok":false,"error":"timed out waiting for provider service_info result","href":readiness["href"],"readyState":readiness["readyState"],"polls":probe_polls});
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            "state" => {
                let window = app.get_webview_window("main");
                json!({"ok":true,"process_id":std::process::id(),"version":env!("CARGO_PKG_VERSION"),"executable_path":std::env::current_exe().ok().map(|p|p.to_string_lossy().into_owned()),"window_visible":window.and_then(|w|w.is_visible().ok()).unwrap_or(false)})
            }
            "eval" => {
                let w = app.get_webview_window("main");
                let Some(w) = w else {
                    return json!({"ok":false,"error":"main webview missing"});
                };
                let js = c["js"].as_str().unwrap_or("null");
                let id = c["id"].as_str().unwrap_or("");
                let code=format!("(async()=>{{let r;try{{r=await (async()=>{{{js}}})()}}catch(e){{r={{error:String(e)}}}}window.__TAURI_INTERNALS__.invoke('e2e_result',{{id:'{id}',value:JSON.stringify(r??null)}})}})()");
                match w.eval(&code) {
                    Ok(()) => json!({"ok":true}),
                    Err(e) => json!({"ok":false,"error":e.to_string()}),
                }
            }
            _ => json!({"ok":false,"error":"unknown operation"}),
        }
    }
    pub fn e2e_result(id: String, value: String) -> Result<(), String> {
        let Some(d) = dir() else {
            return Err("automation disabled".into());
        };
        if !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err("bad id".into());
        }
        let v: Value = serde_json::from_str(&value).map_err(|e| e.to_string())?;
        respond(&d, &id, &json!({"ok":true,"value":v}));
        Ok(())
    }
}
fn authorize_local_main(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() != "main" {
        return Err("privileged command is restricted to the main application window".into());
    }
    // The label alone is not authority: only the native packaged-main builder
    // can register this process-local ownership token, and destruction revokes it.
    // Do not call WebviewWindow::url(): WKURL can be nil during macOS startup.
    let ownership = window.app_handle().state::<MainWindowOwnership>();
    // Authorization performs an Ordering::Acquire load paired with build registration.
    if !ownership.is_authorized(window.label()) {
        return Err("privileged command requires the live native main window".into());
    }
    Ok(())
}

#[tauri::command]
async fn list_conversations(
    window: tauri::WebviewWindow,
    s: State<'_, AppState>,
    query: String,
    cursor: Option<String>,
) -> Result<desktop_api::Page, String> {
    authorize_local_main(&window)?;
    desktop_api::list(&s.api, &s.token, &query, cursor.as_deref()).await
}
#[tauri::command]
async fn get_conversation(
    window: tauri::WebviewWindow,
    s: State<'_, AppState>,
    id: String,
    leaf_node_id: Option<String>,
) -> Result<desktop_api::Detail, String> {
    authorize_local_main(&window)?;
    desktop_api::detail(&s.api, &s.token, &id, leaf_node_id.as_deref()).await
}
#[tauri::command]
async fn import_chatgpt_export(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    s: State<'_, AppState>,
) -> Result<desktop_api::ImportResult, String> {
    authorize_local_main(&window)?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("ChatGPT conversations JSON", &["json"])
        .pick_file(move |file| {
            let _ = tx.send(file);
        });
    let Some(file) = rx.await.map_err(|_| "file picker unavailable")? else {
        return Ok(desktop_api::ImportResult {
            cancelled: true,
            result: None,
        });
    };
    let path = file.into_path().map_err(|_| "invalid_file")?;
    let metadata = std::fs::symlink_metadata(&path).map_err(|_| "invalid_file")?;
    if !metadata.file_type().is_file() || metadata.len() > desktop_api::MAX_IMPORT_BYTES as u64 {
        return Err("file_too_large_or_invalid".into());
    }
    let bytes = tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let f = std::fs::File::open(path).map_err(|_| "invalid_file")?;
        let mut b = Vec::with_capacity(metadata.len() as usize);
        f.take((desktop_api::MAX_IMPORT_BYTES + 1) as u64)
            .read_to_end(&mut b)
            .map_err(|_| "invalid_file")?;
        Ok::<_, &'static str>(b)
    })
    .await
    .map_err(|_| "invalid_file")??;
    desktop_api::validate_import(&bytes)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| "invalid_json")?;
    let count = if let Some(a) = value.as_array() {
        a.len()
    } else {
        value["conversations"]
            .as_array()
            .ok_or("invalid_export")?
            .len()
    } as u64;
    let result = desktop_api::import(&s.api, &s.token, bytes, count).await?;
    Ok(desktop_api::ImportResult {
        cancelled: false,
        result: Some(result),
    })
}

#[tauri::command]
async fn service_health(
    window: tauri::WebviewWindow,
    s: State<'_, AppState>,
) -> Result<ServiceHealth, String> {
    authorize_local_main(&window)?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(1))
        .build()
        .map_err(|_| "health check unavailable".to_string())?;
    let response = client
        .get(format!("{}/api/v1/status", s.api))
        .bearer_auth(&s.token)
        .send()
        .await
        .map_err(|_| "health check unavailable".to_string())?;
    let status = response.status().as_u16();
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|_| "health check unavailable".to_string())?;
    if !healthy_response(status, &body) {
        return Err("health check unavailable".into());
    }
    Ok(ServiceHealth { healthy: true })
}
#[derive(Serialize)]
struct ServiceHealth {
    healthy: bool,
}

#[tauri::command]
fn update_status(
    window: tauri::WebviewWindow,
    s: State<'_, AppState>,
) -> Result<UpdateStatus, String> {
    authorize_local_main(&window)?;
    Ok(s.updater.lock().map_err(|e| e.to_string())?.clone())
}
#[cfg(feature = "native-smoke")]
#[tauri::command]
fn e2e_result(window: tauri::WebviewWindow, id: String, value: String) -> Result<(), String> {
    authorize_local_main(&window)?;
    e2e::e2e_result(id, value)
}
#[tauri::command]
fn service_info(
    window: tauri::WebviewWindow,
    s: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    authorize_local_main(&window)?;
    Ok(
        serde_json::json!({"service":APP_NAME,"version":env!("CARGO_PKG_VERSION"),"api":s.api,"proxy":s.proxy,"process_id":std::process::id(),"executable":std::env::current_exe().ok()}),
    )
}
#[tauri::command]
async fn check_for_updates(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<UpdateStatus, String> {
    authorize_local_main(&window)?;
    #[cfg(feature = "native-smoke")]
    if smoke() {
        let mut s = state.updater.lock().map_err(|e| e.to_string())?;
        s.status = "checking".into();
        s.message = "Checking signed synthetic feed".into();
    }
    use tauri_plugin_updater::UpdaterExt;
    #[cfg(feature = "native-smoke")]
    let updater_result = if smoke() {
        updater_builder(&app)
    } else {
        app.updater().map_err(|e| e.to_string())
    };
    #[cfg(not(feature = "native-smoke"))]
    let updater_result = app.updater().map_err(|e| e.to_string());
    let updater = match updater_result {
        Ok(updater) => updater,
        Err(message) => {
            let status = UpdateStatus {
                version: env!("CARGO_PKG_VERSION").into(),
                channel: "stable".into(),
                status: "error".into(),
                message: message.clone(),
            };
            *state.updater.lock().map_err(|e| e.to_string())? = status;
            return Err(message);
        }
    };
    let result = updater.check().await;
    match result {
        Ok(Some(update)) => {
            let status = UpdateStatus {
                version: update.version.clone(),
                channel: "stable".into(),
                status: "available".into(),
                message: "Update available".into(),
            };
            *state.updater.lock().map_err(|e| e.to_string())? = status.clone();
            *state.pending_update.lock().map_err(|e| e.to_string())? = Some((update, Vec::new()));
            Ok(status)
        }
        Ok(None) => {
            let status = UpdateStatus {
                version: env!("CARGO_PKG_VERSION").into(),
                channel: "stable".into(),
                status: "current".into(),
                message: "No update available".into(),
            };
            *state.updater.lock().map_err(|e| e.to_string())? = status.clone();
            *state.pending_update.lock().map_err(|e| e.to_string())? = None;
            Ok(status)
        }
        Err(error) => {
            let status = UpdateStatus {
                version: env!("CARGO_PKG_VERSION").into(),
                channel: "stable".into(),
                status: "error".into(),
                message: error.to_string(),
            };
            *state.updater.lock().map_err(|e| e.to_string())? = status.clone();
            Err(status.message)
        }
    }
}
#[cfg(feature = "native-smoke")]
fn updater_builder(app: &tauri::AppHandle) -> Result<tauri_plugin_updater::Updater, String> {
    use tauri_plugin_updater::UpdaterExt;
    let endpoint =
        std::env::var("MUNINN_UPDATER_SMOKE_ENDPOINT").map_err(|_| "missing smoke endpoint")?;
    let pubkey =
        std::env::var("MUNINN_UPDATER_SMOKE_PUBKEY").map_err(|_| "missing smoke public key")?;
    let endpoint = tauri::Url::parse(&endpoint).map_err(|e| e.to_string())?;
    app.updater_builder()
        .endpoints(vec![endpoint])
        .map_err(|e| e.to_string())?
        .pubkey(pubkey)
        .version_comparator(|current, release| release.version > current)
        .build()
        .map_err(|e| e.to_string())
}
#[tauri::command]
async fn install_update(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    authorize_local_main(&window)?;
    let pending = state
        .pending_update
        .lock()
        .map_err(|e| e.to_string())?
        .take()
        .ok_or("No checked signed update available")?;
    let (update, bytes) = pending;
    let bytes = if bytes.is_empty() {
        match update.download(|_, _| {}, || {}).await {
            Ok(bytes) => bytes,
            Err(error) => {
                let signature = matches!(
                    error,
                    tauri_plugin_updater::Error::Minisign(_)
                        | tauri_plugin_updater::Error::SignatureUtf8(_)
                        | tauri_plugin_updater::Error::Base64(_)
                        | tauri_plugin_updater::Error::SignedVersionMismatch { .. }
                        | tauri_plugin_updater::Error::MissingSignedVersion
                );
                if signature {
                    let mut status = state.updater.lock().map_err(|e| e.to_string())?;
                    status.status = "signature_rejected".into();
                    status.message = "Update signature verification failed".into();
                }
                return Err(error.to_string());
            }
        }
    } else {
        bytes
    };
    if let Err(error) = update.verify(&bytes) {
        let signature = matches!(
            error,
            tauri_plugin_updater::Error::Minisign(_)
                | tauri_plugin_updater::Error::SignatureUtf8(_)
                | tauri_plugin_updater::Error::Base64(_)
                | tauri_plugin_updater::Error::SignedVersionMismatch { .. }
                | tauri_plugin_updater::Error::MissingSignedVersion
        );
        if signature {
            let mut status = state.updater.lock().map_err(|e| e.to_string())?;
            status.status = "signature_rejected".into();
            status.message = "Update signature verification failed".into();
        }
        return Err(error.to_string());
    }
    update.install(&bytes).map_err(|e| e.to_string())?;
    app.restart();
}
fn main() {
    crash_diagnostics::install();
    let updater_builder = tauri_plugin_updater::Builder::new();
    let builder = tauri::Builder::default()
        .plugin(updater_builder.build())
        .plugin(tauri_plugin_dialog::init());
    let builder = builder.setup(|app| {
        let dir = {
            #[cfg(feature = "native-smoke")]
            {
                if smoke() {
                    if let Some(path) = std::env::var_os("MUNINN_E2E_DATA_DIR") {
                        PathBuf::from(path)
                    } else {
                        app.path().app_data_dir()?
                    }
                } else {
                    app.path().app_data_dir()?
                }
            }
            #[cfg(not(feature = "native-smoke"))]
            {
                app.path().app_data_dir()?
            }
        };
        std::fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let ca = dir.join("ca");
        let mut cfg = ProxyConfig::loopback(&ca);
        #[cfg(feature = "native-smoke")]
        if smoke() {
            if let Ok(upstream) = std::env::var("MUNINN_PROVIDER_SMOKE_UPSTREAM") {
                cfg.test_upstream = Some(upstream.parse().map_err(std::io::Error::other)?);
            }
            if let Ok(roots) = std::env::var("MUNINN_PROVIDER_SMOKE_ROOTS") {
                cfg.test_roots = Some(PathBuf::from(roots));
            }
        }
        let runtime = tauri::async_runtime::block_on(desktop_runtime::start(&dir, cfg))
            .map_err(std::io::Error::other)?;
        let api = runtime.api.clone();
        let token = runtime.token.clone();
        let proxy = runtime.proxy.clone();
        let ca_dir = runtime.ca_dir.clone();
        let service_info = serde_json::json!({
            "service": APP_NAME,
            "version": env!("CARGO_PKG_VERSION"),
            "api": api,
            "process_id": std::process::id(),
        });
        let info_tmp = dir.join("service.json.tmp");
        std::fs::write(&info_tmp, serde_json::to_vec(&service_info)?)?;
        std::fs::rename(info_tmp, dir.join("service.json"))?;
        app.manage(AppState {
            pending_update: Arc::new(Mutex::new(None)),
            updater: Arc::new(Mutex::new(UpdateStatus {
                version: env!("CARGO_PKG_VERSION").into(),
                channel: "stable".into(),
                status: "ready".into(),
                message: "Ready".into(),
            })),
            token,
            api,
            proxy,
            runtime: Arc::new(Mutex::new(Some(runtime))),
            shutdown_started: Arc::new(AtomicBool::new(false)),
            shutdown_finished: Arc::new(AtomicBool::new(false)),
        });
        #[cfg(feature = "native-smoke")]
        e2e::start(app.handle().clone());
        // Disable Tauri's automatic window creation: the configured main window
        // must be created only through this validated native builder.
        let mut main_config = app
            .config()
            .app
            .windows
            .iter()
            .find(|w| w.label == "main")
            .ok_or_else(|| std::io::Error::other("missing configured main window"))?
            .clone();
        if !matches!(main_config.url, tauri::utils::config::WebviewUrl::App(_)) {
            return Err(std::io::Error::other("main window must use packaged app URL").into());
        }
        main_config.create = false;
        let main_ownership = MainWindowOwnership::new();
        app.manage(main_ownership.clone());
        let main = tauri::WebviewWindowBuilder::from_config(app, &main_config)?
            .on_navigation(|url| {
                window_policy::allows_origin(
                    url.scheme(),
                    url.host_str(),
                    url.port(),
                    url.username(),
                    url.password(),
                )
            })
            .build()
            .map_err(|error| {
                eprintln!("main webview setup failed: {error}");
                error
            })?;
        main.on_window_event({
            let ownership = main_ownership.clone();
            move |event| {
                if matches!(event, tauri::WindowEvent::Destroyed) {
                    ownership.revoke();
                }
            }
        });
        // Registration follows successful construction only; JS cannot create or
        // impersonate this native process-local ownership token.
        main_ownership.register_after_build();
        #[cfg(target_os = "macos")]
        {
            let proxy_addr = proxy.clone();
            let proxy_url = format!("http://{proxy_addr}");
            let ca_der = chat_hub::ca::Ca::load_or_create(&ca_dir)
                .and_then(|ca| ca.cert_der())
                .map_err(std::io::Error::other)?;
            let provider = tauri::WebviewWindowBuilder::new(
                app,
                "provider",
                tauri::WebviewUrl::External(
                    "https://chatgpt.com"
                        .parse()
                        .map_err(std::io::Error::other)?,
                ),
            )
            .title("Provider")
            .proxy_url(proxy_url.parse().map_err(std::io::Error::other)?)
            .build()?;
            crate::trust_mac::prepare(&ca_der, &provider);
        }
        Ok(())
    });
    #[cfg(feature = "native-smoke")]
    let run = builder
        .invoke_handler(tauri::generate_handler![
            update_status,
            list_conversations,
            get_conversation,
            import_chatgpt_export,
            service_health,
            service_info,
            check_for_updates,
            install_update,
            e2e_result
        ])
        .build(tauri::generate_context!())
        .map(|app| {
            app.run(|app, event| {
                if let tauri::RunEvent::ExitRequested { code, api, .. } = event {
                    let state = app.state::<AppState>();
                    if state.shutdown_finished.load(Ordering::Acquire) {
                        return;
                    }
                    api.prevent_exit();
                    if state.shutdown_started.swap(true, Ordering::AcqRel) {
                        return;
                    }
                    let runtime = state.runtime.lock().ok().and_then(|mut slot| slot.take());
                    let finished = state.shutdown_finished.clone();
                    let handle = app.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Some(runtime) = runtime {
                            let _ = runtime.shutdown().await;
                        }
                        finished.store(true, Ordering::Release);
                        handle.exit(code.unwrap_or(0));
                    });
                }
            })
        });
    #[cfg(not(feature = "native-smoke"))]
    let run = builder
        .invoke_handler(tauri::generate_handler![
            update_status,
            list_conversations,
            get_conversation,
            import_chatgpt_export,
            service_health,
            service_info,
            check_for_updates,
            install_update
        ])
        .build(tauri::generate_context!())
        .map(|app| {
            app.run(|app, event| {
                if let tauri::RunEvent::ExitRequested { code, api, .. } = event {
                    let state = app.state::<AppState>();
                    if state.shutdown_finished.load(Ordering::Acquire) {
                        return;
                    }
                    api.prevent_exit();
                    if state.shutdown_started.swap(true, Ordering::AcqRel) {
                        return;
                    }
                    let runtime = state.runtime.lock().ok().and_then(|mut slot| slot.take());
                    let finished = state.shutdown_finished.clone();
                    let handle = app.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Some(runtime) = runtime {
                            let _ = runtime.shutdown().await;
                        }
                        finished.store(true, Ordering::Release);
                        handle.exit(code.unwrap_or(0));
                    });
                }
            })
        });
    match run {
        Ok(()) => {}
        Err(error) => eprintln!("desktop failed: {error}"),
    }
}
