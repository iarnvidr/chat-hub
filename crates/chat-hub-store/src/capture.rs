//! Compatibility ingestion adapter; pure decoding lives in chat-hub-formats.
use crate::{error::Result, store::Store};
use chat_hub_model::{CaptureEvent, Observation};
use serde_json::Value;

pub use chat_hub_formats::parse_event;

pub fn normalize_chatgpt_document(
    doc: &Value,
    parser: &str,
    observed_ms: i64,
) -> Result<Observation> {
    Ok(chat_hub_formats::normalize_chatgpt_document(
        doc,
        parser,
        observed_ms,
    )?)
}

pub fn normalize_chatgpt_document_in_scope(
    doc: &Value,
    parser: &str,
    observed_ms: i64,
    account_scope: impl Into<String>,
) -> Result<Observation> {
    Ok(chat_hub_formats::normalize_chatgpt_document_in_scope(
        doc,
        parser,
        observed_ms,
        account_scope,
    )?)
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

impl chat_hub_model::ObservationSink for Store {
    type Error = crate::error::Error;

    fn store_observation(
        &mut self,
        observation: &Observation,
    ) -> Result<chat_hub_model::IngestOutcome> {
        self.ingest_observation_outcome(observation)
    }
}
