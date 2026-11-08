//! Pure transcript normalization, shared by imports and network capture.
use chat_hub_model::{CaptureEvent, GraphNode, Observation};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizationError(pub String);

impl std::fmt::Display for NormalizationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for NormalizationError {}

pub fn normalize_chatgpt_document(
    doc: &Value,
    parser: &str,
    observed_ms: i64,
) -> Result<Observation, NormalizationError> {
    normalize_chatgpt_document_in_scope(doc, parser, observed_ms, "provisional:default")
}

pub fn normalize_chatgpt_document_in_scope(
    doc: &Value,
    parser: &str,
    observed_ms: i64,
    account_scope: impl Into<String>,
) -> Result<Observation, NormalizationError> {
    let account_scope = account_scope.into();
    if !chat_hub_model::valid_capture_profile_scope(&account_scope) {
        return Err(NormalizationError("invalid capture profile scope".into()));
    }
    let mapping = doc
        .get("mapping")
        .and_then(Value::as_object)
        .ok_or_else(|| NormalizationError("missing mapping object".into()))?;
    let source_id = doc
        .get("conversation_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            doc.get("id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        })
        .ok_or_else(|| NormalizationError("missing conversation id".into()))?;
    let mut graph = Vec::new();
    for (node_id, node) in mapping {
        let node = node
            .as_object()
            .ok_or_else(|| NormalizationError("mapping node must be an object".into()))?;
        let parent = match node.get("parent") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            _ => {
                return Err(NormalizationError(
                    "node parent must be null or a string".into(),
                ))
            }
        };
        let m = node.get("message").unwrap_or(&Value::Null);
        if !m.is_null() && !m.is_object() {
            return Err(NormalizationError(
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
            return Err(NormalizationError(
                "message content must be an object".into(),
            ));
        }
        if let Some(parts) = content.get("parts") {
            if !parts.is_array() {
                return Err(NormalizationError(
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
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalization_preserves_graph_scope_and_structured_parts_without_storage() {
        let document = json!({
            "id": "conversation/with spaces", "title": "Saved chat", "current_node": "reply",
            "mapping": {
                "root": {"parent": null, "message": null},
                "reply": {"parent": "root", "message": {
                    "id": "message-1", "author": {"role": "assistant"}, "create_time": 123,
                    "content": {"parts": ["text", {"content_type": "image", "asset_pointer": "asset-1"}]}
                }}
            }
        });
        let observation = normalize_chatgpt_document_in_scope(
            &document, "fixture-v1", 456, "chatgpt:observed:account-a"
        ).unwrap();
        assert_eq!(observation.account_scope, "chatgpt:observed:account-a");
        assert_eq!(observation.graph.len(), 2);
        assert_eq!(observation.current_node.as_deref(), Some("reply"));
        assert_eq!(observation.raw_payload, document);
        assert_eq!(observation.url.as_deref(), Some("https://chatgpt.com/c/conversation%2Fwith%20spaces"));
        let reply = observation.graph.iter().find(|node| node.source_node_id == "reply").unwrap();
        assert_eq!(reply.parts.len(), 2);
        assert_eq!(reply.parts[1]["asset_pointer"], "asset-1");
    }

    #[test]
    fn invalid_scope_and_malformed_parts_are_rejected_by_the_shared_parser() {
        let mut document = json!({"id": "one", "mapping": {}});
        assert_eq!(normalize_chatgpt_document_in_scope(&document, "fixture", 1, "bad/scope").unwrap_err().0,
            "invalid capture profile scope");
        document["mapping"]["node"] = json!({"parent": null, "message": {"content": {"parts": {}}}});
        assert_eq!(normalize_chatgpt_document(&document, "fixture", 1).unwrap_err().0,
            "content parts must be an array");
    }
}
