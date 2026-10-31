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
        .timeout(Duration::from_secs(if source == "paseo-history" {
            300
        } else {
            60
        }))
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
    let mut q = vec![("limit", "50")];
    if !query.trim().is_empty() {
        q.push(("q", query.trim()));
    }
    if let Some(c) = cursor {
        q.push(("cursor", c));
    }
    let filtered = match source {
        None | Some("all") => false,
        Some("chatgpt") => true,
        _ => return Err("invalid_source".into()),
    };
    let endpoint = match (filtered, query.trim().is_empty()) {
        (false, true) => "conversations",
        (false, false) => "search",
        (true, true) => "conversations",
        (true, false) => "search",
    };
    let segments: &[&str] = if filtered {
        &["api", "v1", "chat", endpoint]
    } else {
        &["api", "v1", endpoint]
    };
    url(base, segments, &q)
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
    let c = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| "Desktop API unavailable")?;
    let u = list_url_for_source(base, q, cursor, source)?;
    get(&c, u, token).await
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
    use super::list_url_for_source;

    #[test]
    fn source_selects_server_side_list_and_search_routes() {
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
            "/api/v1/chat/search"
        );
        assert_eq!(
            list_url_for_source(base, "", Some("cursor"), Some("chatgpt"))
                .unwrap()
                .path(),
            "/api/v1/chat/conversations"
        );
        assert_eq!(
            list_url_for_source(base, "", None, Some("codex")).unwrap_err(),
            "invalid_source"
        );
    }
}
