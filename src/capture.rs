//! Minimal ChatGPT conversation-response recognizer and capture ingestion.
use crate::{error::Result, proxy::CaptureEvent, store::Store};
use serde_json::Value;

/// Identifies a successful ChatGPT conversation-detail response by request host/path.
pub fn parse_event(event: &CaptureEvent) -> Option<Value> {
    if !event.host.eq_ignore_ascii_case("chatgpt.com") || !(200..300).contains(&event.status) {
        return None;
    }
    let path = event.path.split('?').next().unwrap_or(&event.path);
    let id = path.strip_prefix("/backend-api/conversation/")?;
    if id.is_empty() || id.contains('/') {
        return None;
    }
    let mut value: Value = serde_json::from_slice(&event.body).ok()?;
    let object = value.as_object_mut()?;
    if object
        .get("conversation_id")
        .and_then(Value::as_str)
        .is_none()
    {
        object.insert("conversation_id".to_owned(), Value::String(id.to_owned()));
    }
    value.get("mapping").and_then(Value::as_object)?;
    Some(value)
}

impl Store {
    /// Ingests a captured provider response. Returns `true` when a recognized conversation
    /// was stored, `false` for unsupported/non-conversation responses (a safe no-op).
    pub fn ingest_capture(&mut self, event: CaptureEvent) -> Result<bool> {
        let Some(doc) = parse_event(&event) else {
            return Ok(false);
        };
        self.ingest_document(&doc, &event.host)?;
        Ok(true)
    }
    fn ingest_document(&mut self, doc: &Value, host: &str) -> Result<()> {
        let id = doc["conversation_id"].as_str().unwrap();
        let title = doc["title"].as_str().unwrap_or("");
        let url = format!("https://{host}/c/{id}");
        let tx = self.connection.transaction()?;
        tx.execute("INSERT INTO conversations(id,title,provider,url) VALUES (?1,?2,'chatgpt',?3) ON CONFLICT(id) DO UPDATE SET title=excluded.title,url=excluded.url", rusqlite::params![id,title,url])?;
        for (node_id, node) in doc["mapping"].as_object().unwrap() {
            let message = &node["message"];
            if message.is_null() {
                continue;
            }
            let role = message["author"]["role"].as_str().unwrap_or("unknown");
            let content = message["content"]["parts"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            let parent = node["parent"].as_str();
            tx.execute("INSERT INTO messages(id,conversation_id,parent_id,role,content) VALUES (?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET conversation_id=excluded.conversation_id,parent_id=excluded.parent_id,role=excluded.role,content=excluded.content", rusqlite::params![node_id,id,parent,role,content])?;
        }
        tx.commit()?;
        Ok(())
    }
}
