//! Legacy capture adapter and normalized ingestion.
use crate::{
    error::{Error, Result},
    proxy::CaptureEvent,
    store::{GraphNode, Observation, Store},
};
use serde_json::Value;

pub fn normalize_chatgpt_document(
    doc: &Value,
    parser: &str,
    observed_ms: i64,
) -> Result<Observation> {
    normalize_chatgpt_document_in_scope(doc, parser, observed_ms, "provisional:default")
}

pub fn normalize_chatgpt_document_in_scope(
    doc: &Value,
    parser: &str,
    observed_ms: i64,
    account_scope: impl Into<String>,
) -> Result<Observation> {
    let account_scope = account_scope.into();
    if account_scope.is_empty()
        || account_scope.len() > 256
        || !account_scope
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'-' | b'_' | b'.'))
    {
        return Err(Error::Configuration("invalid capture profile scope".into()));
    }
    let mapping = doc
        .get("mapping")
        .and_then(Value::as_object)
        .ok_or_else(|| Error::Configuration("missing mapping object".into()))?;
    let source_id = doc
        .get("conversation_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            doc.get("id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        })
        .ok_or_else(|| Error::Configuration("missing conversation id".into()))?;
    let mut graph = Vec::new();
    for (node_id, node) in mapping {
        let node = node
            .as_object()
            .ok_or_else(|| Error::Configuration("mapping node must be an object".into()))?;
        let parent = match node.get("parent") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            _ => {
                return Err(Error::Configuration(
                    "node parent must be null or a string".into(),
                ))
            }
        };
        let m = node.get("message").unwrap_or(&Value::Null);
        if !m.is_null() && !m.is_object() {
            return Err(Error::Configuration(
                "node message must be null or an object".into(),
            ));
        }
        if m.is_null() {
            graph.push(GraphNode {
                source_node_id: node_id.clone(),
                source_message_id: None,
                parent_id: parent.clone(),
                role: None,
                author: None,
                create_time: None,
                parts: Vec::new(),
            });
            continue;
        }
        let content = m.get("content").unwrap_or(&Value::Null);
        if !content.is_null() && !content.is_object() {
            return Err(Error::Configuration(
                "message content must be an object".into(),
            ));
        }
        if let Some(parts) = content.get("parts") {
            if !parts.is_array() {
                return Err(Error::Configuration(
                    "content parts must be an array".into(),
                ));
            }
        }
        graph.push(GraphNode {
            source_node_id: node_id.clone(),
            source_message_id: m["id"].as_str().map(str::to_owned),
            parent_id: parent.clone(),
            role: m["author"]["role"].as_str().map(str::to_owned),
            author: Some(m["author"].clone()),
            create_time: m["create_time"]
                .as_number()
                .map(|_| m["create_time"].clone()),
            parts: m["content"]["parts"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        });
    }
    let encoded: String = source_id
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    Ok(Observation {
        provider: "chatgpt".into(),
        account_scope,
        source_id: source_id.into(),
        title: doc["title"].as_str().unwrap_or("").into(),
        url: Some(format!("https://chatgpt.com/c/{encoded}")),
        current_node: doc["current_node"].as_str().map(str::to_owned),
        graph,
        raw_payload: doc.clone(),
        source_time: doc["update_time"].as_number().map(|n| n.to_string()),
        observed_ms,
        parser: parser.into(),
        completeness: "complete".into(),
    })
}

pub fn parse_event(event: &CaptureEvent) -> Option<Value> {
    if !event.host.eq_ignore_ascii_case("chatgpt.com") || !(200..300).contains(&event.status) {
        return None;
    }
    let path = event.path.split('?').next().unwrap_or(&event.path);
    let id = path.strip_prefix("/backend-api/conversation/")?;
    if id.is_empty() || id.contains('/') {
        return None;
    }
    let mut v: Value = serde_json::from_slice(&event.body).ok()?;
    let o = v.as_object_mut()?;
    if o.get("conversation_id").and_then(Value::as_str).is_none() {
        o.insert("conversation_id".into(), Value::String(id.into()));
    }
    v.get("mapping").and_then(Value::as_object)?;
    Some(v)
}
impl Store {
    pub fn ingest_capture(&mut self, event: CaptureEvent) -> Result<bool> {
        let Some(doc) = parse_event(&event) else {
            return Ok(false);
        };
        let observed_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let observation = normalize_chatgpt_document(&doc, "chatgpt-live-v1", observed_ms)?;
        self.ingest_observation(&observation)?;
        Ok(true)
    }
}
