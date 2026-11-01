use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

const MAX_RESPONSE: usize = 8 * 1024 * 1024;
pub const MAX_IMPORT_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_IMPORT_CONVERSATIONS: usize = 10_000;
pub const MAX_IMPORT_ERRORS: usize = 100;
#[derive(Debug, Serialize, Deserialize)]
pub struct ImportError {
    pub item_index: u64,
    pub code: String,
    pub message: String,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct ImportReceipt {
    pub imported: u64,
    pub updated: u64,
    pub unchanged: u64,
    pub failed: u64,
    pub errors: Vec<ImportError>,
    pub errors_truncated: bool,
}
#[derive(Debug, Serialize)]
pub struct ImportResult {
    pub cancelled: bool,
    pub result: Option<ImportReceipt>,
}
pub fn validate_import(data: &[u8]) -> Result<(), String> {
    if data.len() > MAX_IMPORT_BYTES {
        return Err("file_too_large".into());
    }
    let value: serde_json::Value = serde_json::from_slice(data).map_err(|_| "invalid_json")?;
    let conversations = if value.is_array() {
        value.as_array()
    } else {
        value
            .get("conversations")
            .and_then(serde_json::Value::as_array)
    }
    .ok_or("invalid_export")?;
    if conversations.len() > MAX_IMPORT_CONVERSATIONS {
        return Err("too_many_conversations".into());
    }
    Ok(())
}
pub fn validate_receipt(r: &ImportReceipt, count: u64) -> Result<(), String> {
    let sum = r
        .imported
        .checked_add(r.updated)
        .and_then(|x| x.checked_add(r.unchanged))
        .and_then(|x| x.checked_add(r.failed))
        .ok_or("invalid_response")?;
    if sum != count || r.errors.len() > MAX_IMPORT_ERRORS || r.errors.len() as u64 > r.failed {
        return Err("invalid_response".into());
    }
    Ok(())
}
pub async fn import(
    base: &str,
    token: &str,
    data: Vec<u8>,
    count: u64,
) -> Result<ImportReceipt, String> {
    import_from(base, token, data, count, "chatgpt").await
}

pub async fn import_paseo_history(
    base: &str,
    token: &str,
    data: Vec<u8>,
    count: u64,
) -> Result<ImportReceipt, String> {
    import_from(base, token, data, count, "paseo-history").await
}

pub async fn import_chatgpt_history(
    base: &str,
    token: &str,
    data: Vec<u8>,
    count: u64,
) -> Result<ImportReceipt, String> {
    import_from(base, token, data, count, "chatgpt-history").await
}

async fn import_from(
    base: &str,
    token: &str,
    data: Vec<u8>,
    count: u64,
    source: &str,
) -> Result<ImportReceipt, String> {
    if data.len() > MAX_IMPORT_BYTES {
        return Err("file_too_large".into());
    }
    let c = Client::builder()
        .timeout(Duration::from_secs(
            if matches!(source, "paseo-history" | "chatgpt-history") {
                300
            } else {
                60
            },
        ))
        .build()
        .map_err(|_| "Desktop API unavailable")?;
    let u = url(base, &["api", "v1", "imports", source], &[])?;
    let r = c
        .post(u)
        .bearer_auth(token)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(data)
        .send()
        .await
        .map_err(|_| "Import may have partially completed. Retry the same file safely.")?;
    if !r.status().is_success() {
        return Err("Desktop API unavailable".into());
    }
    let mut body = Vec::new();
    let mut response = r;
    while let Some(chunk) = response.chunk().await.map_err(|_| "invalid_response")? {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE {
            return Err("response_too_large".into());
        }
        body.extend_from_slice(&chunk);
    }
    let receipt: ImportReceipt = serde_json::from_slice(&body).map_err(|_| "invalid_response")?;
    validate_receipt(&receipt, count)?;
    Ok(receipt)
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Page {
    pub items: Vec<serde_json::Value>,
    pub next_cursor: Option<String>,
    pub library_revision: Option<serde_json::Value>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Detail {
    pub conversation: serde_json::Value,
    pub selection: serde_json::Value,
    pub nodes: Vec<serde_json::Value>,
    pub messages: Vec<serde_json::Value>,
    pub thread_node_ids: Vec<String>,
    pub branches: Vec<Branch>,
    pub library_revision: Option<serde_json::Value>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Branch {
    pub leaf_node_id: String,
    pub selected: bool,
}
fn url(base: &str, segments: &[&str], query: &[(&str, &str)]) -> Result<Url, String> {
    let mut u = Url::parse(base).map_err(|_| "Desktop API unavailable")?;
    {
        let mut p = u
            .path_segments_mut()
            .map_err(|_| "Desktop API unavailable")?;
        p.pop_if_empty();
        for s in segments {
            p.push(s);
        }
    }
    u.query_pairs_mut().extend_pairs(query.iter().copied());
    Ok(u)
}
pub fn list_url(base: &str, query: &str, cursor: Option<&str>) -> Result<Url, String> {
    list_url_for_source(base, query, cursor, None)
}

pub fn list_url_for_source(
    base: &str,
    query: &str,
    cursor: Option<&str>,
    source: Option<&str>,
) -> Result<Url, String> {
    list_url_for_scope(base, query, cursor, source, None)
}
pub fn list_url_for_scope(
    base: &str,
    query: &str,
    cursor: Option<&str>,
    source: Option<&str>,
    project_key: Option<&str>,
) -> Result<Url, String> {
    list_url_for_scope_and_tag(base, query, cursor, source, project_key, None)
}
pub fn list_url_for_scope_and_tag(
    base: &str,
    query: &str,
    cursor: Option<&str>,
    source: Option<&str>,
    project_key: Option<&str>,
    tag: Option<&str>,
) -> Result<Url, String> {
    list_url_for_scope_tag_archive(base, query, cursor, source, project_key, tag, None)
}
pub fn list_url_for_scope_tag_archive(
    base: &str,
    query: &str,
    cursor: Option<&str>,
    source: Option<&str>,
    project_key: Option<&str>,
    tag: Option<&str>,
    archive_state: Option<&str>,
) -> Result<Url, String> {
    let mut q = vec![("limit", "50")];
    if !query.trim().is_empty() {
        q.push(("q", query.trim()));
    }
    if let Some(c) = cursor {
        q.push(("cursor", c));
    }
    if let Some(source) = source {
        if !matches!(source, "all" | "chatgpt" | "codex") {
            return Err("invalid_source".into());
        }
        q.push(("source", source));
    }
    if let Some(project_key) = project_key {
        if project_key.is_empty() || project_key.len() > 1024 {
            return Err("invalid_project".into());
        }
        q.push(("project_key", project_key));
    }
    if let Some(tag) = tag {
        if tag.trim().is_empty() || tag.len() > 256 {
            return Err("invalid_tag".into());
        }
        q.push(("tag", tag));
    }
    if let Some(archive_state) = archive_state {
        if !matches!(archive_state, "active" | "archived" | "all") {
            return Err("invalid_archive_state".into());
        }
        q.push(("archive_state", archive_state));
    }
    let endpoint = if query.trim().is_empty() {
        "conversations"
    } else {
        "search"
    };
    url(base, &["api", "v1", endpoint], &q)
}
pub fn detail_url(base: &str, id: &str, leaf: Option<&str>) -> Result<Url, String> {
    let mut u = url(base, &["api", "v1", "conversations", id], &[])?;
    if let Some(n) = leaf {
        u.query_pairs_mut().append_pair("leaf_node_id", n);
    }
    Ok(u)
}
fn map_http_error(status: StatusCode, code: Option<&str>) -> &'static str {
    match status {
        StatusCode::BAD_REQUEST if code == Some("invalid_cursor") => "invalid_cursor",
        StatusCode::BAD_REQUEST => "invalid_request",
        StatusCode::CONFLICT => "stale_cursor",
        StatusCode::UNAUTHORIZED => "unauthorized",
        StatusCode::NOT_FOUND => "not_found",
        StatusCode::SERVICE_UNAVAILABLE if code == Some("busy") => "busy",
        StatusCode::SERVICE_UNAVAILABLE if code == Some("response_too_large") => {
            "response_too_large"
        }
        StatusCode::SERVICE_UNAVAILABLE => "service_unavailable",
        _ => "service_unavailable",
    }
}
async fn get<T: for<'de> Deserialize<'de>>(c: &Client, u: Url, token: &str) -> Result<T, String> {
    let r = c
        .get(u)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| "Desktop API unavailable")?;
    if !r.status().is_success() {
        let status = r.status();
        let mut response = r;
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| "service_unavailable")? {
            let remaining = 4096usize.saturating_sub(body.len());
            if chunk.len() > remaining {
                body.extend_from_slice(&chunk[..remaining]);
                break;
            }
            body.extend_from_slice(&chunk);
        }
        let parsed: Option<Value> = serde_json::from_slice(&body).ok();
        let code = parsed
            .as_ref()
            .and_then(|v| {
                v.get("error")
                    .and_then(|e| e.get("code"))
                    .or_else(|| v.get("code"))
            })
            .and_then(Value::as_str);
        return Err(map_http_error(status, code).into());
    }
    let mut response = r;
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Desktop API unavailable")?
    {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE {
            return Err("response_too_large".into());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| "invalid_response".into())
}
pub async fn list(base: &str, token: &str, q: &str, cursor: Option<&str>) -> Result<Page, String> {
    list_in_source(base, token, q, cursor, None).await
}

pub async fn list_in_source(
    base: &str,
    token: &str,
    q: &str,
    cursor: Option<&str>,
    source: Option<&str>,
) -> Result<Page, String> {
    list_in_scope(base, token, q, cursor, source, None).await
}
pub async fn list_in_scope(
    base: &str,
    token: &str,
    q: &str,
    cursor: Option<&str>,
    source: Option<&str>,
    project_key: Option<&str>,
) -> Result<Page, String> {
    list_in_scope_with_tag(base, token, q, cursor, source, project_key, None).await
}
pub async fn list_in_scope_with_tag(
    base: &str,
    token: &str,
    q: &str,
    cursor: Option<&str>,
    source: Option<&str>,
    project_key: Option<&str>,
    tag: Option<&str>,
) -> Result<Page, String> {
    list_in_scope_with_tag_and_archive(base, token, q, cursor, source, project_key, tag, None).await
}
// Keep the server filter arguments explicit to mirror the native list command.
#[allow(clippy::too_many_arguments)]
pub async fn list_in_scope_with_tag_and_archive(
    base: &str,
    token: &str,
    q: &str,
    cursor: Option<&str>,
    source: Option<&str>,
    project_key: Option<&str>,
    tag: Option<&str>,
    archive_state: Option<&str>,
) -> Result<Page, String> {
    let c = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| "Desktop API unavailable")?;
    let u =
        list_url_for_scope_tag_archive(base, q, cursor, source, project_key, tag, archive_state)?;
    get(&c, u, token).await
}
pub async fn projects(base: &str, token: &str, source: &str) -> Result<Value, String> {
    if !matches!(source, "all" | "chatgpt" | "codex") {
        return Err("invalid_source".into());
    }
    let c = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| "Desktop API unavailable")?;
    let u = url(base, &["api", "v1", "projects"], &[("source", source)])?;
    get(&c, u, token).await
}
pub async fn rename_project(
    base: &str,
    token: &str,
    project_key: &str,
    name: Option<&str>,
) -> Result<Value, String> {
    if project_key.is_empty() || project_key.len() > 1024 {
        return Err("invalid_project".into());
    }
    let c = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| "Desktop API unavailable")?;
    let u = url(base, &["api", "v1", "projects", "name"], &[])?;
    let body = serde_json::to_vec(&serde_json::json!({"project_key":project_key,"name":name}))
        .map_err(|_| "invalid_request")?;
    let response = c
        .patch(u)
        .bearer_auth(token)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
        .map_err(|_| "Desktop API unavailable")?;
    if !response.status().is_success() {
        if response.status() == StatusCode::CONFLICT {
            return Err("scope_mismatch".into());
        }
        return Err(map_http_error(response.status(), None).into());
    }
    let mut response = response;
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Desktop API unavailable")?
    {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE {
            return Err("response_too_large".into());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| "invalid_response".into())
}

pub async fn commit_verified_project_title(
    base: &str,
    token: &str,
    project_key: &str,
    expected_scope: &str,
    title: &str,
    legacy_target: Option<(&str, &str)>,
) -> Result<Value, String> {
    if project_key.is_empty()
        || project_key.len() > 1024
        || expected_scope.len() > 128
        || title.trim().is_empty()
        || title.chars().count() > 120
        || title.chars().any(char::is_control)
    {
        return Err("invalid_project".into());
    }
    let c = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| "Desktop API unavailable")?;
    let u = url(base, &["api", "v1", "projects", "title"], &[])?;
    let body = serde_json::to_vec(&serde_json::json!({
        "project_key":project_key,
        "expected_scope":expected_scope,
        "title":title.trim(),
        "legacy_target":legacy_target.map(|(account_scope,source_id)| serde_json::json!({
            "account_scope":account_scope,
            "source_id":source_id,
        })),
    }))
    .map_err(|_| "invalid_request")?;
    let response = c
        .patch(u)
        .bearer_auth(token)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
        .map_err(|_| "Desktop API unavailable")?;
    if !response.status().is_success() {
        return Err(map_http_error(response.status(), None).into());
    }
    let mut response = response;
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Desktop API unavailable")?
    {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE {
            return Err("response_too_large".into());
        }
        body.extend_from_slice(&chunk);
    }
    let result: Value = serde_json::from_slice(&body).map_err(|_| "invalid_response")?;
    if result.get("verified").and_then(Value::as_bool) != Some(true)
        || !result
            .get("library_revision")
            .and_then(Value::as_i64)
            .is_some_and(|revision| revision >= 0)
    {
        return Err("update_unconfirmed".into());
    }
    Ok(result)
}

pub async fn project_assignment_context(
    base: &str,
    token: &str,
    conversation_id: &str,
    project_key: Option<&str>,
) -> Result<Value, String> {
    if conversation_id.is_empty()
        || conversation_id.len() > 256
        || project_key.is_some_and(|key| key.is_empty() || key.len() > 1024)
    {
        return Err("invalid_request".into());
    }
    let c = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| "Desktop API unavailable")?;
    let mut query = vec![("conversation_id", conversation_id)];
    if let Some(project_key) = project_key {
        query.push(("project_key", project_key));
    }
    let u = url(base, &["api", "v1", "project-assignment"], &query)?;
    get(&c, u, token).await
}
pub async fn learned_project_operations(base: &str, token: &str) -> Result<Value, String> {
    let c = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| "Desktop API unavailable")?;
    let u = url(base, &["api", "v1", "learned-project-operations"], &[])?;
    get(&c, u, token).await
}

pub async fn commit_project_assignment(
    base: &str,
    token: &str,
    conversation_id: &str,
    expected_conversation_scope: &str,
    project_key: Option<&str>,
    verified_project_title: Option<&str>,
) -> Result<Value, String> {
    if conversation_id.is_empty()
        || conversation_id.len() > 256
        || expected_conversation_scope.is_empty()
        || expected_conversation_scope.len() > 256
        || project_key.is_some_and(|key| key.is_empty() || key.len() > 1024)
    {
        return Err("invalid_request".into());
    }
    let c = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|_| "Desktop API unavailable")?;
    let u = url(base, &["api", "v1", "project-assignment"], &[])?;
    let body = serde_json::to_vec(&serde_json::json!({
        "conversation_id":conversation_id,
        "expected_conversation_scope":expected_conversation_scope,
        "project_key":project_key,
        "verified_project_title":verified_project_title,
    }))
    .map_err(|_| "invalid_request")?;
    let response = c
        .patch(u)
        .bearer_auth(token)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
        .map_err(|_| "Desktop API unavailable")?;
    if !response.status().is_success() {
        return Err(map_http_error(response.status(), None).into());
    }
    let mut response = response;
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Desktop API unavailable")?
    {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE {
            return Err("response_too_large".into());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| "invalid_response".into())
}

pub async fn commit_conversation_state(
    base: &str,
    token: &str,
    conversation_id: &str,
    receipt: &serde_json::Value,
    archived: Option<bool>,
    pinned: Option<bool>,
) -> Result<Value, String> {
    if conversation_id.is_empty()
        || conversation_id.len() > 1024
        || (!archived.is_some() && !pinned.is_some())
        || !receipt.is_object()
    {
        return Err("invalid_request".into());
    }
    let c = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|_| "Desktop API unavailable")?;
    let u = url(
        base,
        &["api", "v1", "conversations", conversation_id, "state"],
        &[],
    )?;
    let body = serde_json::to_vec(&serde_json::json!({
        "receipt": receipt,
        "archived": archived,
        "pinned": pinned,
    }))
    .map_err(|_| "invalid_request")?;
    let response = c
        .patch(u)
        .bearer_auth(token)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
        .map_err(|_| "Desktop API unavailable")?;
    if !response.status().is_success() {
        return Err(map_http_error(response.status(), None).into());
    }
    let mut response = response;
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Desktop API unavailable")?
    {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE {
            return Err("response_too_large".into());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| "invalid_response".into())
}

pub async fn sync_chatgpt_pin_snapshot(
    base: &str,
    token: &str,
    receipt: &serde_json::Value,
) -> Result<Value, String> {
    if !receipt.is_object() {
        return Err("invalid_request".into());
    }
    let c = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|_| "Desktop API unavailable")?;
    let u = url(base, &["api", "v1", "chatgpt", "pins", "snapshot"], &[])?;
    let body = serde_json::to_vec(&serde_json::json!({ "receipt": receipt }))
        .map_err(|_| "invalid_request")?;
    if body.len() > 128 * 1024 {
        return Err("invalid_request".into());
    }
    let response = c
        .post(u)
        .bearer_auth(token)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
        .map_err(|_| "Desktop API unavailable")?;
    if !response.status().is_success() {
        return Err(map_http_error(response.status(), None).into());
    }
    let mut response = response;
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Desktop API unavailable")?
    {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE {
            return Err("response_too_large".into());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| "invalid_response".into())
}

pub async fn conversation_state_identity(
    base: &str,
    token: &str,
    conversation_id: &str,
) -> Result<Value, String> {
    if conversation_id.is_empty() || conversation_id.len() > 1024 {
        return Err("invalid_request".into());
    }
    let c = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| "Desktop API unavailable")?;
    let u = conversation_state_url(base, conversation_id)?;
    get(&c, u, token).await
}

fn conversation_state_url(base: &str, conversation_id: &str) -> Result<Url, String> {
    if conversation_id.is_empty() || conversation_id.len() > 1024 {
        return Err("invalid_request".into());
    }
    url(
        base,
        &["api", "v1", "conversations", conversation_id, "state"],
        &[],
    )
}

fn catalog_url(
    base: &str,
    endpoint: &str,
    source: &str,
    project_key: Option<&str>,
    limit: Option<usize>,
    focus_id: Option<&str>,
) -> Result<Url, String> {
    if !matches!(source, "all" | "chatgpt" | "codex")
        || project_key.is_some_and(|key| key.is_empty() || key.len() > 1024)
        || limit.is_some_and(|value| !(1..=200).contains(&value))
        || focus_id.is_some_and(|value| value.trim().is_empty() || value.len() > 256)
    {
        return Err("invalid_request".into());
    }
    let mut query = vec![("source", source.to_owned())];
    if let Some(project_key) = project_key {
        query.push(("project_key", project_key.to_owned()));
    }
    let limit_text = limit.map(|value| value.to_string());
    if let Some(value) = limit_text.as_deref() {
        query.push(("limit", value.to_owned()));
    }
    if let Some(focus_id) = focus_id {
        query.push(("focus_id", focus_id.to_owned()));
    }
    let pairs = query
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect::<Vec<_>>();
    url(base, &["api", "v1", "catalog", endpoint], &pairs)
}
pub async fn catalog_overview(
    base: &str,
    token: &str,
    source: &str,
    project_key: Option<&str>,
) -> Result<Value, String> {
    let c = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|_| "Desktop API unavailable")?;
    get(
        &c,
        catalog_url(base, "overview", source, project_key, None, None)?,
        token,
    )
    .await
}
pub async fn catalog_graph(
    base: &str,
    token: &str,
    source: &str,
    project_key: Option<&str>,
    limit: usize,
    focus_id: Option<&str>,
) -> Result<Value, String> {
    catalog_graph_with_tag(base, token, source, project_key, limit, focus_id, None).await
}
pub async fn catalog_graph_with_tag(
    base: &str,
    token: &str,
    source: &str,
    project_key: Option<&str>,
    limit: usize,
    focus_id: Option<&str>,
    tag: Option<&str>,
) -> Result<Value, String> {
    let c = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|_| "Desktop API unavailable")?;
    let mut address = catalog_url(base, "graph", source, project_key, Some(limit), focus_id)?;
    if let Some(tag) = tag {
        address.query_pairs_mut().append_pair("tag", tag);
    }
    get(&c, address, token).await
}
pub async fn rebuild_catalog_index(base: &str, token: &str) -> Result<Value, String> {
    let c = Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|_| "Desktop API unavailable")?;
    let u = url(base, &["api", "v1", "catalog", "rebuild"], &[])?;
    let response = c
        .post(u)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| "Desktop API unavailable")?;
    if !response.status().is_success() {
        return Err(map_http_error(response.status(), None).into());
    }
    let mut response = response;
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Desktop API unavailable")?
    {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE {
            return Err("response_too_large".into());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| "invalid_response".into())
}
pub async fn detail(
    base: &str,
    token: &str,
    id: &str,
    leaf: Option<&str>,
) -> Result<Detail, String> {
    let c = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| "Desktop API unavailable")?;
    let u = detail_url(base, id, leaf)?;
    get(&c, u, token).await
}

#[cfg(test)]
mod source_list_tests {
    use super::{
        catalog_url, conversation_state_url, list_url_for_scope, list_url_for_scope_and_tag,
        list_url_for_scope_tag_archive, list_url_for_source,
    };

    #[test]
    fn source_and_project_filters_are_sent_to_server_before_pagination() {
        let base = "http://127.0.0.1:8000";
        assert_eq!(
            list_url_for_source(base, "", None, None).unwrap().path(),
            "/api/v1/conversations"
        );
        assert_eq!(
            list_url_for_source(base, "", None, Some("all"))
                .unwrap()
                .path(),
            "/api/v1/conversations"
        );
        assert_eq!(
            list_url_for_source(base, " term ", None, Some("chatgpt"))
                .unwrap()
                .path(),
            "/api/v1/search"
        );
        assert_eq!(
            list_url_for_source(base, "", Some("cursor"), Some("chatgpt"))
                .unwrap()
                .path(),
            "/api/v1/conversations"
        );
        let codex = list_url_for_source(base, "", None, Some("codex")).unwrap();
        assert_eq!(
            codex.query_pairs().find(|(k, _)| k == "source").unwrap().1,
            "codex"
        );
        let project =
            list_url_for_scope(base, "", None, Some("chatgpt"), Some("project:key")).unwrap();
        assert_eq!(
            project
                .query_pairs()
                .find(|(k, _)| k == "project_key")
                .unwrap()
                .1,
            "project:key"
        );
    }

    #[test]
    fn catalog_requests_keep_source_project_and_node_limit_scoped() {
        let base = "http://127.0.0.1:8000";
        let overview = catalog_url(
            base,
            "overview",
            "chatgpt",
            Some("project:[id]"),
            None,
            None,
        )
        .unwrap();
        assert_eq!(overview.path(), "/api/v1/catalog/overview");
        assert_eq!(
            overview
                .query_pairs()
                .find(|(k, _)| k == "source")
                .unwrap()
                .1,
            "chatgpt"
        );
        assert_eq!(
            overview
                .query_pairs()
                .find(|(k, _)| k == "project_key")
                .unwrap()
                .1,
            "project:[id]"
        );
        let graph = catalog_url(
            base,
            "graph",
            "codex",
            None,
            Some(80),
            Some("local-chat-id"),
        )
        .unwrap();
        assert_eq!(graph.path(), "/api/v1/catalog/graph");
        assert_eq!(
            graph.query_pairs().find(|(k, _)| k == "limit").unwrap().1,
            "80"
        );
        assert_eq!(
            graph
                .query_pairs()
                .find(|(k, _)| k == "focus_id")
                .unwrap()
                .1,
            "local-chat-id"
        );
        assert!(catalog_url(base, "graph", "unknown", None, Some(80), None).is_err());
        assert!(catalog_url(base, "graph", "all", None, Some(201), None).is_err());
        assert!(catalog_url(base, "graph", "all", None, Some(80), Some(" ")).is_err());
    }

    #[test]
    fn tag_filtered_list_url_preserves_tag_in_query_scope() {
        let address = list_url_for_scope_and_tag(
            "http://127.0.0.1:8000",
            "search text",
            None,
            Some("chatgpt"),
            Some("project:key"),
            Some("Work Notes"),
        )
        .unwrap();
        let pairs = address.query_pairs().collect::<Vec<_>>();
        assert!(pairs
            .iter()
            .any(|(key, value)| key == "tag" && value == "Work Notes"));
        assert!(list_url_for_scope_and_tag(
            "http://127.0.0.1:8000",
            "",
            None,
            None,
            None,
            Some("   "),
        )
        .is_err());
    }

    #[test]
    fn archive_filter_is_server_scoped_for_lists_and_searches() {
        let list = list_url_for_scope_tag_archive(
            "http://127.0.0.1:8000",
            "",
            None,
            Some("chatgpt"),
            None,
            None,
            Some("archived"),
        )
        .unwrap();
        assert_eq!(list.path(), "/api/v1/conversations");
        assert_eq!(
            list.query_pairs()
                .find(|(key, _)| key == "archive_state")
                .unwrap()
                .1,
            "archived"
        );
        let search = list_url_for_scope_tag_archive(
            "http://127.0.0.1:8000",
            "needle",
            None,
            Some("chatgpt"),
            None,
            None,
            Some("active"),
        )
        .unwrap();
        assert_eq!(search.path(), "/api/v1/search");
        assert_eq!(
            search
                .query_pairs()
                .find(|(key, _)| key == "archive_state")
                .unwrap()
                .1,
            "active"
        );
        assert!(list_url_for_scope_tag_archive(
            "http://127.0.0.1:8000",
            "",
            None,
            None,
            None,
            None,
            Some("unknown")
        )
        .is_err());
    }

    #[test]
    fn conversation_state_identity_uses_bounded_row_path() {
        let id = "123e4567-e89b-12d3-a456-426614174000";
        let endpoint = conversation_state_url("http://127.0.0.1:8000", id).unwrap();
        assert_eq!(endpoint.path(), format!("/api/v1/conversations/{id}/state"));
        assert!(conversation_state_url("http://127.0.0.1:8000", "").is_err());
        assert!(conversation_state_url("http://127.0.0.1:8000", &"x".repeat(1025)).is_err());
    }
}
