//! Main-window-only access to local entities, tags and saved file bytes.
use super::{authorize_local_main, AppState};
use serde_json::{json, Value};
use std::{path::Path, time::Duration};
use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(|_| "service_unavailable".into())
}
fn endpoint(api: &str, parts: &[&str], query: &[(&str, String)]) -> Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(api).map_err(|_| "service_unavailable")?;
    let loopback = url
        .host_str()
        .and_then(|h| h.trim_matches(['[', ']']).parse::<std::net::IpAddr>().ok())
        .is_some_and(|ip| ip.is_loopback());
    if url.scheme() != "http" || !loopback || !url.username().is_empty() || url.password().is_some()
    {
        return Err("service_unavailable".into());
    }
    url.path_segments_mut()
        .map_err(|_| "service_unavailable")?
        .pop_if_empty()
        .extend(["api", "v1"])
        .extend(parts.iter().copied());
    url.query_pairs_mut()
        .extend_pairs(query.iter().map(|(k, v)| (*k, v.as_str())));
    Ok(url)
}
async fn bounded(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, String> {
    if !response.status().is_success() {
        return Err(match response.status().as_u16() {
            404 => "not_found",
            413 => "file_too_large",
            503 => "busy",
            _ => "invalid_request",
        }
        .into());
    }
    if response.content_length().is_some_and(|n| n > limit as u64) {
        return Err("response_too_large".into());
    }
    let mut out = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "service_unavailable")? {
        if out.len().saturating_add(chunk.len()) > limit {
            return Err("response_too_large".into());
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}
async fn get(s: &AppState, parts: &[&str], query: &[(&str, String)]) -> Result<Value, String> {
    let response = client()?
        .get(endpoint(&s.api, parts, query)?)
        .bearer_auth(&s.token)
        .send()
        .await
        .map_err(|_| "service_unavailable")?;
    serde_json::from_slice(&bounded(response, 8 * 1024 * 1024).await?)
        .map_err(|_| "invalid_response".into())
}
fn scope(source: Option<String>, project_key: Option<String>) -> Vec<(&'static str, String)> {
    let mut query = vec![("source", source.unwrap_or_else(|| "chatgpt".into()))];
    if let Some(v) = project_key {
        query.push(("project_key", v));
    }
    query
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri exposes independently optional catalog filters as command arguments.
pub async fn catalog_entities(
    window: tauri::Webview,
    s: State<'_, AppState>,
    source: Option<String>,
    project_key: Option<String>,
    kind: Option<String>,
    artifact_state: Option<String>,
    conversation_id: Option<String>,
    tag: Option<String>,
    query: Option<String>,
    cursor: Option<String>,
    limit: Option<usize>,
) -> Result<Value, String> {
    authorize_local_main(&window)?;
    let mut q = scope(source, project_key);
    for (k, v) in [
        ("kind", kind),
        ("artifact_state", artifact_state),
        ("conversation_id", conversation_id),
        ("tag", tag),
        ("query", query),
        ("cursor", cursor),
    ] {
        if let Some(v) = v {
            q.push((k, v));
        }
    }
    q.push(("limit", limit.unwrap_or(40).to_string()));
    get(&s, &["catalog", "entities"], &q).await
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri exposes the category graph's explicit query filters.
pub async fn catalog_entity_graph(
    window: tauri::Webview,
    s: State<'_, AppState>,
    source: Option<String>,
    project_key: Option<String>,
    conversation_id: Option<String>,
    tag: Option<String>,
    query: Option<String>,
    kind: Option<String>,
    artifact_state: Option<String>,
    limit: Option<usize>,
) -> Result<Value, String> {
    authorize_local_main(&window)?;
    let mut q = scope(source, project_key);
    for (k, v) in [
        ("conversation_id", conversation_id),
        ("tag", tag),
        ("query", query),
        ("kind", kind),
        ("artifact_state", artifact_state),
    ] {
        if let Some(v) = v {
            q.push((k, v));
        }
    }
    q.push(("limit", limit.unwrap_or(40).clamp(1, 40).to_string()));
    get(&s, &["catalog", "entity-graph"], &q).await
}

#[tauri::command]
pub async fn catalog_tag_suggestions(
    window: tauri::Webview,
    s: State<'_, AppState>,
    conversation_id: String,
    limit: Option<usize>,
) -> Result<Value, String> {
    authorize_local_main(&window)?;
    get(
        &s,
        &["catalog", "tag-suggestions"],
        &[
            ("conversation_id", conversation_id),
            ("limit", limit.unwrap_or(12).clamp(1, 12).to_string()),
        ],
    )
    .await
}

#[tauri::command]
pub async fn catalog_entity(
    window: tauri::Webview,
    s: State<'_, AppState>,
    id: String,
) -> Result<Value, String> {
    authorize_local_main(&window)?;
    get(&s, &["catalog", "entities", &id], &[]).await
}
#[tauri::command]
pub async fn list_local_tags(
    window: tauri::Webview,
    s: State<'_, AppState>,
    target_kind: Option<String>,
    query: Option<String>,
) -> Result<Value, String> {
    authorize_local_main(&window)?;
    let mut q = vec![("limit", 200.to_string())];
    if let Some(v) = target_kind {
        q.push(("target_kind", v));
    }
    if let Some(v) = query {
        q.push(("query", v));
    }
    get(&s, &["tags"], &q).await
}
#[tauri::command]
pub async fn get_target_tags(
    window: tauri::Webview,
    s: State<'_, AppState>,
    target_kind: String,
    target_id: String,
) -> Result<Value, String> {
    authorize_local_main(&window)?;
    get(
        &s,
        &["tags", "assignments"],
        &[("target_kind", target_kind), ("target_id", target_id)],
    )
    .await
}
#[tauri::command]
pub async fn set_target_tags(
    window: tauri::Webview,
    s: State<'_, AppState>,
    target_kind: String,
    target_id: String,
    tags: Vec<String>,
) -> Result<Value, String> {
    authorize_local_main(&window)?;
    let response = client()?
        .put(endpoint(&s.api, &["tags", "assignments"], &[])?)
        .bearer_auth(&s.token)
        .header("Content-Type", "application/json")
        .body(json!({"target_kind":target_kind,"target_id":target_id,"tags":tags}).to_string())
        .send()
        .await
        .map_err(|_| "service_unavailable")?;
    serde_json::from_slice(&bounded(response, 1024 * 1024).await?)
        .map_err(|_| "invalid_response".into())
}
#[tauri::command]
pub async fn tagged_targets(
    window: tauri::Webview,
    s: State<'_, AppState>,
    tag: String,
    target_kind: Option<String>,
    after_kind: Option<String>,
    after_id: Option<String>,
    tag_revision: Option<i64>,
) -> Result<Value, String> {
    authorize_local_main(&window)?;
    let mut q = vec![("tag", tag), ("limit", 100.to_string())];
    if let Some(revision) = tag_revision {
        q.push(("tag_revision", revision.to_string()));
    }
    for (k, v) in [
        ("target_kind", target_kind),
        ("after_kind", after_kind),
        ("after_id", after_id),
    ] {
        if let Some(v) = v {
            q.push((k, v));
        }
    }
    get(&s, &["tags", "targets"], &q).await
}
#[tauri::command]
pub async fn saved_artifacts(
    window: tauri::Webview,
    s: State<'_, AppState>,
    conversation_id: Option<String>,
    cursor: Option<String>,
) -> Result<Value, String> {
    authorize_local_main(&window)?;
    let mut q = vec![("limit", 100.to_string())];
    if let Some(v) = conversation_id {
        q.push(("conversation_id", v));
    }
    if let Some(v) = cursor {
        q.push(("after", v));
    }
    get(&s, &["artifacts"], &q).await
}

#[tauri::command]
pub async fn attach_artifact_file(
    window: tauri::Webview,
    app: tauri::AppHandle,
    s: State<'_, AppState>,
    id: String,
    conversation_id: String,
    mime_type: Option<String>,
) -> Result<Value, String> {
    authorize_local_main(&window)?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_file(move |v| {
        let _ = tx.send(v);
    });
    let Some(file) = rx.await.map_err(|_| "file_picker_unavailable")? else {
        return Ok(json!({"cancelled":true}));
    };
    let path = file.into_path().map_err(|_| "invalid_file")?;
    let (filename, bytes) = tauri::async_runtime::spawn_blocking(move || read_selected_file(&path))
        .await
        .map_err(|_| "invalid_file")??;
    let mut q = vec![("conversation_id", conversation_id), ("filename", filename)];
    if let Some(v) = mime_type {
        q.push(("mime_type", v));
    }
    let response = client()?
        .post(endpoint(&s.api, &["artifacts", &id, "content"], &q)?)
        .bearer_auth(&s.token)
        .header("Content-Type", "application/octet-stream")
        .body(bytes)
        .send()
        .await
        .map_err(|_| "service_unavailable")?;
    serde_json::from_slice(&bounded(response, 1024 * 1024).await?)
        .map_err(|_| "invalid_response".into())
}
fn read_selected_file(path: &Path) -> Result<(String, Vec<u8>), String> {
    use std::io::Read;
    let metadata = std::fs::symlink_metadata(path).map_err(|_| "invalid_file")?;
    if !metadata.file_type().is_file() {
        return Err("invalid_file".into());
    }
    if metadata.len() > chat_hub::artifacts::MAX_ARTIFACT_BYTES as u64 {
        return Err("file_too_large".into());
    }
    let name = path
        .file_name()
        .and_then(|v| v.to_str())
        .filter(|s| chat_hub::artifacts::valid_filename(s))
        .ok_or("invalid_file")?
        .to_owned();
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|_| "invalid_file")?
        .take((chat_hub::artifacts::MAX_ARTIFACT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "invalid_file")?;
    if bytes.len() > chat_hub::artifacts::MAX_ARTIFACT_BYTES {
        return Err("file_too_large".into());
    }
    Ok((name, bytes))
}
async fn file_bytes(s: &AppState, id: &str) -> Result<Vec<u8>, String> {
    let response = client()?
        .get(endpoint(&s.api, &["artifacts", id, "content"], &[])?)
        .bearer_auth(&s.token)
        .send()
        .await
        .map_err(|_| "service_unavailable")?;
    bounded(response, chat_hub::artifacts::MAX_ARTIFACT_BYTES).await
}
fn saved_filename(metadata: &Value) -> Result<&str, String> {
    metadata["filename"]
        .as_str()
        .filter(|v| chat_hub::artifacts::valid_filename(v))
        .ok_or_else(|| "invalid_response".into())
}
fn write_selected_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "invalid_destination")?
        .as_nanos();
    write_selected_file_with_nonce(path, bytes, nonce)
}
fn write_selected_file_with_nonce(path: &Path, bytes: &[u8], nonce: u128) -> Result<(), String> {
    use std::io::Write;
    let parent = path.parent().ok_or("invalid_destination")?;
    if !path
        .file_name()
        .and_then(|v| v.to_str())
        .is_some_and(chat_hub::artifacts::valid_filename)
    {
        return Err("invalid_destination".into());
    }
    if std::fs::symlink_metadata(path).is_ok_and(|v| !v.file_type().is_file()) {
        return Err("invalid_destination".into());
    }
    let temporary = parent.join(format!(".chat-hub-file-{}-{nonce}", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut created_temporary = false;
    let result = (|| {
        let mut f = options.open(&temporary).map_err(|_| "file_write_failed")?;
        created_temporary = true;
        f.write_all(bytes)
            .and_then(|_| f.sync_all())
            .map_err(|_| "file_write_failed")?;
        std::fs::rename(&temporary, path).map_err(|_| "file_write_failed")?;
        Ok(())
    })();
    if result.is_err() && created_temporary {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map_err(str::to_owned)
}
#[tauri::command]
pub async fn export_artifact_file(
    window: tauri::Webview,
    app: tauri::AppHandle,
    s: State<'_, AppState>,
    id: String,
) -> Result<Value, String> {
    authorize_local_main(&window)?;
    let metadata = get(&s, &["artifacts", &id], &[]).await?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_file_name(saved_filename(&metadata)?)
        .save_file(move |v| {
            let _ = tx.send(v);
        });
    let Some(file) = rx.await.map_err(|_| "file_picker_unavailable")? else {
        return Ok(json!({"cancelled":true}));
    };
    let path = file.into_path().map_err(|_| "invalid_destination")?;
    let bytes = file_bytes(&s, &id).await?;
    tauri::async_runtime::spawn_blocking(move || write_selected_file(&path, &bytes))
        .await
        .map_err(|_| "file_write_failed")??;
    Ok(json!({"saved":true}))
}
#[tauri::command]
pub async fn open_artifact_file(
    window: tauri::Webview,
    app: tauri::AppHandle,
    s: State<'_, AppState>,
    id: String,
    reveal: Option<bool>,
) -> Result<Value, String> {
    authorize_local_main(&window)?;
    let metadata = get(&s, &["artifacts", &id], &[]).await?;
    let name = saved_filename(&metadata)?.to_owned();
    let sha = metadata["sha256"]
        .as_str()
        .filter(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or("invalid_response")?;
    let directory = app
        .path()
        .app_cache_dir()
        .map_err(|_| "file_write_failed")?
        .join("saved-files")
        .join(sha);
    let bytes = file_bytes(&s, &id).await?;
    let show = reveal.unwrap_or(false) || !can_open_data_file(&name);
    tauri::async_runtime::spawn_blocking(move || {
        std::fs::create_dir_all(&directory).map_err(|_| "file_write_failed")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| "file_write_failed")?;
        }
        let path = directory.join(name);
        write_selected_file(&path, &bytes)?;
        open_data_file(&path, show)
    })
    .await
    .map_err(|_| "file_open_failed")??;
    Ok(json!({"opened":true,"revealed":show}))
}
fn can_open_data_file(name: &str) -> bool {
    matches!(
        Path::new(name)
            .extension()
            .and_then(|v| v.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some(
            "pdf"
                | "txt"
                | "md"
                | "json"
                | "csv"
                | "tsv"
                | "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "webp"
                | "xlsx"
                | "docx"
                | "pptx"
                | "odt"
                | "ods"
        )
    )
}

#[tauri::command]
pub async fn open_catalog_link(
    window: tauri::Webview,
    s: State<'_, AppState>,
    id: String,
) -> Result<Value, String> {
    authorize_local_main(&window)?;
    let item = get(&s, &["catalog", "entities", &id], &[]).await?;
    let address = item["entity"]["canonical_url"]
        .as_str()
        .ok_or("link_unavailable")?;
    let url = reqwest::Url::parse(address).map_err(|_| "link_unavailable")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("link_unavailable".into());
    }
    let address = url.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        #[cfg(target_os = "macos")]
        let result = std::process::Command::new("open").arg(&address).spawn();
        #[cfg(target_os = "linux")]
        let result = std::process::Command::new("xdg-open").arg(&address).spawn();
        #[cfg(target_os = "windows")]
        let result = std::process::Command::new("explorer").arg(&address).spawn();
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        return Err("link_unavailable".to_string());
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        result
            .map(|_| ())
            .map_err(|_| "link_unavailable".to_string())
    })
    .await
    .map_err(|_| "link_unavailable")??;
    Ok(json!({"opened":true}))
}
fn open_data_file(path: &Path, reveal: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let mut command = std::process::Command::new("open");
        if reveal {
            command.arg("-R");
        }
        command.arg(path).spawn().map_err(|_| "file_open_failed")?;
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        let target = if reveal {
            path.parent().unwrap_or(path)
        } else {
            path
        };
        std::process::Command::new("xdg-open")
            .arg(target)
            .spawn()
            .map_err(|_| "file_open_failed")?;
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (path, reveal);
        Err("file_open_unavailable".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn urls_encode_entity_ids_and_keep_credentials_off_urls() {
        let url = endpoint(
            "http://127.0.0.1:5000",
            &["catalog", "entities", "entity:a/b"],
            &[("tag", "two words".into())],
        )
        .unwrap();
        assert!(url.as_str().contains("entity:a%2Fb"));
        assert_eq!(url.query_pairs().next().unwrap().1, "two words");
        assert!(endpoint("https://example.com", &[], &[]).is_err());
    }
    #[test]
    fn executable_and_browser_content_is_revealed_instead_of_executed() {
        for name in [
            "run.command",
            "tool.exe",
            "index.html",
            "image.svg",
            "script.sh",
        ] {
            assert!(!can_open_data_file(name));
        }
        assert!(can_open_data_file("Report.PDF"));
        for name in ["../escape.pdf", "C:escape.pdf", "bad\\name.pdf"] {
            assert!(saved_filename(&json!({"filename":name})).is_err());
        }
        assert_eq!(
            saved_filename(&json!({"filename":"safe.pdf"})).unwrap(),
            "safe.pdf"
        );
    }
    #[test]
    fn selected_file_read_is_bounded_and_rejects_symlinks() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!(
            "chat-hub-selected-file-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let small = dir.join("small.csv");
        std::fs::File::create(&small)
            .unwrap()
            .write_all(b"a,b\n")
            .unwrap();
        assert_eq!(
            read_selected_file(&small).unwrap(),
            ("small.csv".into(), b"a,b\n".to_vec())
        );
        let large = dir.join("large.bin");
        let file = std::fs::File::create(&large).unwrap();
        file.set_len((chat_hub::artifacts::MAX_ARTIFACT_BYTES + 1) as u64)
            .unwrap();
        assert_eq!(read_selected_file(&large).unwrap_err(), "file_too_large");
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let link = dir.join("link.csv");
            symlink(&small, &link).unwrap();
            assert_eq!(read_selected_file(&link).unwrap_err(), "invalid_file");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn selected_file_write_is_atomic_and_collision_never_removes_an_unowned_temp() {
        let dir = std::env::temp_dir().join(format!(
            "chat-hub-write-file-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let target = dir.join("report.csv");
        std::fs::write(&target, b"old").unwrap();
        write_selected_file_with_nonce(&target, b"complete new bytes", 17).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"complete new bytes");
        let temporary = dir.join(format!(".chat-hub-file-{}-18", std::process::id()));
        std::fs::write(&temporary, b"unowned collision file").unwrap();
        assert_eq!(
            write_selected_file_with_nonce(&target, b"replacement", 18).unwrap_err(),
            "file_write_failed"
        );
        assert_eq!(
            std::fs::read(&temporary).unwrap(),
            b"unowned collision file"
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"complete new bytes");
        assert_eq!(
            write_selected_file_with_nonce(&dir.join(".."), b"x", 19).unwrap_err(),
            "invalid_destination"
        );
        assert_eq!(
            write_selected_file_with_nonce(&dir.join("bad:name.csv"), b"x", 20).unwrap_err(),
            "invalid_destination"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let link = dir.join("target-link.csv");
            symlink(&target, &link).unwrap();
            assert_eq!(
                write_selected_file_with_nonce(&link, b"x", 21).unwrap_err(),
                "invalid_destination"
            );
            assert_eq!(std::fs::read(&target).unwrap(), b"complete new bytes");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
