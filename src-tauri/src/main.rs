use chat_hub::{
    config::APP_NAME,
    proxy::{self, CaptureEvent, ProxyConfig},
    server,
    store::Store,
};
use serde::Serialize;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tauri::{Manager, State};
use tokio::sync::mpsc;
mod crash_diagnostics;
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
}
#[derive(Clone, Serialize)]
struct UpdateStatus {
    version: String,
    channel: String,
    status: String,
    message: String,
}
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
    use tauri::{Manager, WebviewWindow};
    fn dir() -> Option<PathBuf> {
        if !cfg!(debug_assertions) || !super::smoke() {
            return None;
        }
        std::env::var_os("MUNINN_E2E_DIR").map(PathBuf::from)
    }
    fn respond(d: &std::path::Path, id: &str, v: &Value) {
        let p = d.join(format!("{id}.res.json"));
        let t = d.join(format!("{id}.res.json.tmp"));
        if std::fs::write(&t, v.to_string()).is_ok() {
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
                let result = run(&app, &c);
                respond(&d, &id, &result);
            }
        });
    }
    fn run(app: &tauri::AppHandle, c: &Value) -> Value {
        match c["op"].as_str().unwrap_or("") {
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
    #[tauri::command]
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
#[tauri::command]
fn update_status(s: State<'_, AppState>) -> UpdateStatus {
    s.updater.lock().unwrap().clone()
}
#[cfg(feature = "native-smoke")]
#[tauri::command]
fn e2e_result(id: String, value: String) -> Result<(), String> {
    e2e::e2e_result(id, value)
}
#[tauri::command]
fn service_info(s: State<'_, AppState>) -> serde_json::Value {
    serde_json::json!({"service":APP_NAME,"version":env!("CARGO_PKG_VERSION"),"api":s.api,"proxy":s.proxy,"process_id":std::process::id(),"executable":std::env::current_exe().ok()})
}
#[tauri::command]
async fn check_for_updates(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<UpdateStatus, String> {
    #[cfg(feature = "native-smoke")]
    if smoke() {
        let mut s = state.updater.lock().map_err(|e| e.to_string())?;
        s.status = "checking".into();
        s.message = "Checking signed synthetic feed".into();
    }
    use tauri_plugin_updater::UpdaterExt;
    #[cfg(feature = "native-smoke")]
    let updater = if smoke() {
        updater_builder(&app)?
    } else {
        app.updater().map_err(|e| e.to_string())?
    };
    #[cfg(not(feature = "native-smoke"))]
    let updater = app.updater().map_err(|e| e.to_string())?;
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
    let current = std::env::var("MUNINN_UPDATER_SMOKE_CURRENT_VERSION")
        .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").into());
    let endpoint = url::Url::parse(&endpoint).map_err(|e| e.to_string())?;
    app.updater_builder()
        .endpoints(vec![endpoint])
        .map_err(|e| e.to_string())?
        .pubkey(pubkey)
        .version_comparator(move |_, release| {
            semver::Version::parse(&release.version)
                .ok()
                .zip(semver::Version::parse(&current).ok())
                .is_some_and(|(r, c)| r > c)
        })
        .build()
        .map_err(|e| e.to_string())
}
#[tauri::command]
async fn install_update(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
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
fn persistent_token(path: &std::path::Path) -> Result<String, String> {
    use std::io::{Read, Write};
    let f = path.join("service.token");
    if let Ok(mut x) = std::fs::File::open(&f) {
        let mut t = String::new();
        x.read_to_string(&mut t).map_err(|e| e.to_string())?;
        if t.len() >= 64 {
            return Ok(t);
        }
    }
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|e| e.to_string())?;
    let token = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let mut o = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&f)
        .or_else(|_| std::fs::File::create(&f))
        .map_err(|e| e.to_string())?;
    o.write_all(token.as_bytes()).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?
    }
    Ok(token)
}
fn main() {
    crash_diagnostics::install();
    let updater_builder = tauri_plugin_updater::Builder::new();
    let builder = tauri::Builder::default().plugin(updater_builder.build());
    let builder = builder.setup(|app| {
        let dir = app.path().app_data_dir()?;
        std::fs::create_dir_all(&dir)?;
        let token = persistent_token(&dir).map_err(std::io::Error::other)?;
        let db = dir.join("chat-hub.sqlite3");
        let ca = dir.join("ca");
        let (tx, mut rx) = mpsc::channel::<CaptureEvent>(128);
        let store = Arc::new(Mutex::new(
            Store::open(db.to_str().unwrap()).map_err(std::io::Error::other)?,
        ));
        let ingest = store.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(e) = rx.recv().await {
                if let Ok(mut s) = ingest.lock() {
                    let _ = s.ingest_capture(e);
                }
            }
        });
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
        let (proxy, _) =
            tauri::async_runtime::block_on(proxy::run(cfg, tx)).map_err(std::io::Error::other)?;
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let api = listener.local_addr()?;
        drop(listener);
        let api_listener = tauri::async_runtime::block_on(tokio::net::TcpListener::bind(api))?;
        let router = server::router(
            Store::open(db.to_str().unwrap()).map_err(std::io::Error::other)?,
            token.clone(),
        );
        tauri::async_runtime::spawn(async move {
            let _ = axum::serve(api_listener, router).await;
        });
        let service_info = serde_json::json!({
            "service": APP_NAME,
            "version": env!("CARGO_PKG_VERSION"),
            "api": format!("http://{api}"),
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
            api: format!("http://{api}"),
            proxy: proxy.addr.to_string(),
        });
        #[cfg(feature = "native-smoke")]
        e2e::start(app.handle().clone());
        #[cfg(target_os = "macos")]
        {
            let proxy_url = format!("http://{}", proxy.addr);
            let ca_der = proxy.ca.cert_der().map_err(std::io::Error::other)?;
            let provider = tauri::WebviewWindowBuilder::new(
                app,
                "provider",
                tauri::WebviewUrl::External("https://chatgpt.com".parse().unwrap()),
            )
            .title("Provider")
            .proxy_url(proxy_url.parse().unwrap())
            .build()?;
            crate::trust_mac::prepare(&ca_der, &provider);
        }
        Ok(())
    });
    #[cfg(feature = "native-smoke")]
    let result = builder
        .invoke_handler(tauri::generate_handler![
            update_status,
            service_info,
            check_for_updates,
            install_update,
            e2e_result
        ])
        .run(tauri::generate_context!());
    #[cfg(not(feature = "native-smoke"))]
    let result = builder
        .invoke_handler(tauri::generate_handler![
            update_status,
            service_info,
            check_for_updates,
            install_update
        ])
        .run(tauri::generate_context!());
    if let Err(e) = result {
        eprintln!("desktop failed: {e}")
    }
}
