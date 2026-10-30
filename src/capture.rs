//! Legacy capture adapter and normalized ingestion.
use crate::{
    error::Result,
    proxy::CaptureEvent,
    store::{GraphNode, Observation, Store},
};
use serde_json::Value;
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
        self.ingest_document(&doc, &event.host)?;
        Ok(true)
    }
    fn ingest_document(&mut self, doc: &Value, host: &str) -> Result<()> {
        let mut graph = Vec::new();
        for (node_id, node) in doc["mapping"].as_object().unwrap() {
            let m = &node["message"];
            if m.is_null() {
                graph.push(GraphNode {
                    source_node_id: node_id.clone(),
                    source_message_id: None,
                    parent_id: node["parent"].as_str().map(str::to_owned),
                    role: None,
                    author: None,
                    create_time: None,
                    parts: Vec::new(),
                });
                continue;
            }
            let c = &m["content"];
            let parts = c["parts"].as_array().cloned().unwrap_or_default();
            graph.push(GraphNode {
                source_node_id: node_id.clone(),
                source_message_id: m["id"].as_str().map(str::to_owned),
                parent_id: node["parent"].as_str().map(str::to_owned),
                role: m["author"]["role"].as_str().map(str::to_owned),
                author: Some(m["author"].clone()),
                create_time: m["create_time"]
                    .as_number()
                    .map(|_| m["create_time"].clone()),
                parts,
            });
        }
        let source_id = doc["conversation_id"]
            .as_str()
            .ok_or_else(|| crate::error::Error::Configuration("missing conversation_id".into()))?;
        let o = Observation {
            provider: "chatgpt".into(),
            account_scope: "provisional:default".into(),
            source_id: source_id.into(),
            title: doc["title"].as_str().unwrap_or("").into(),
            url: Some(format!("https://{host}/c/{source_id}")),
            current_node: doc["current_node"].as_str().map(str::to_owned),
            graph,
            raw_payload: doc.clone(),
            source_time: None,
            observed_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
            parser: "chatgpt-detail-v1".into(),
            completeness: "complete".into(),
        };
        self.ingest_observation(&o)?;
        Ok(())
    }
}
