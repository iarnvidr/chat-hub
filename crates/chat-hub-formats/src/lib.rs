//! Bounded input and transcript-format decoding shared by Chat Hub adapters.

use serde::Serialize;
use serde_json::Value;
use std::io::Read;

pub const MAX_IMPORT_BYTES: usize = 67_108_864;
pub const MAX_BYTES: usize = MAX_IMPORT_BYTES;
pub const MAX_ENTRIES: usize = 10_000;

#[derive(Debug, Default, Serialize, PartialEq, Eq)]
pub struct ImportReceipt {
    pub imported: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub failed: usize,
    pub errors: Vec<ItemError>,
    pub errors_truncated: bool,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ItemError {
    pub item_index: usize,
    pub code: &'static str,
    pub message: &'static str,
}

/// Safe admission errors; never include source bytes, file paths or DB diagnostics.
#[derive(Debug, PartialEq, Eq)]
pub struct ImportError {
    pub code: &'static str,
    pub message: &'static str,
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ImportError {}

fn invalid() -> ImportError {
    ImportError {
        code: "invalid_request",
        message: "Expected a UTF-8 JSON conversation array or an object containing conversations.",
    }
}

/// Check the format payload cap without allocating or decoding it.
pub fn validate_input_size(bytes: &[u8]) -> Result<(), ImportError> {
    if bytes.len() > MAX_BYTES {
        return Err(ImportError {
            code: "payload_too_large",
            message: "Import exceeds the 64 MiB limit.",
        });
    }
    Ok(())
}

/// Read at most the limit plus one byte, including when metadata is unavailable.
pub fn read_input(reader: impl Read) -> Result<Vec<u8>, ImportError> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ImportError {
            code: "input_unavailable",
            message: "Unable to read import input.",
        })?;
    validate_input_size(&bytes)?;
    Ok(bytes)
}

/// Validate the entire global envelope before returning any items for ingestion.
/// ZIP exports remain unsupported; callers must extract `conversations.json`.
pub fn parse_export(bytes: &[u8]) -> Result<Vec<Value>, ImportError> {
    validate_input_size(bytes)?;
    if bytes.starts_with(b"PK") {
        return Err(ImportError {
            code: "invalid_request",
            message: "Extract conversations.json from the ZIP archive before importing.",
        });
    }
    let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    let value: Value = serde_json::from_str(text).map_err(|_| invalid())?;
    let entries = match value {
        Value::Array(entries) => entries,
        Value::Object(mut object) => match object.remove("conversations") {
            Some(Value::Array(entries)) => entries,
            _ => return Err(invalid()),
        },
        _ => return Err(invalid()),
    };
    if entries.len() > MAX_ENTRIES {
        return Err(ImportError {
            code: "invalid_request",
            message: "Import exceeds the 10000 conversation limit.",
        });
    }
    Ok(entries)
}

/// Pure decoded representation of a cached transcript before core graph
/// normalization and account-scoped persistence.
#[derive(Clone, Debug, PartialEq)]
pub struct PaseoCachedTranscript {
    pub source: String,
    pub graph: Value,
    pub url: Option<String>,
}

/// Build the one-node archive graph used for a rendered cached transcript.
/// This deliberately does not infer speaker boundaries from transcript text.
pub fn paseo_cached_transcript(doc: &Value) -> Option<PaseoCachedTranscript> {
    let source = doc
        .get("source")
        .and_then(Value::as_str)
        .filter(|source| matches!(*source, "chatgpt" | "codex_local" | "codex_cloud"))?;
    if doc.get("kind").and_then(Value::as_str) != Some("conversation") {
        return None;
    }
    let id = doc
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())?;
    let text = doc.get("text").and_then(Value::as_str)?;
    let mut graph = serde_json::json!({
        "id": id, "title": doc.get("title").and_then(Value::as_str).unwrap_or("Saved conversation"),
        "current_node": "paseo-cached-transcript", "paseo_cache": doc,
        "mapping": {"paseo-cached-transcript": {
            "id": "paseo-cached-transcript", "parent": null, "children": [],
            "message": {"id":"paseo-cached-transcript", "author":{"role":"archive"},
                "content":{"content_type":"text", "parts":[text]}}
        }}
    });
    if let Some(updated) = doc.get("updated").and_then(Value::as_str) {
        let seconds = updated
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .or_else(|| {
                time::OffsetDateTime::parse(updated, &time::format_description::well_known::Rfc3339)
                    .ok()
                    .map(|value| value.unix_timestamp_nanos() as f64 / 1_000_000_000.0)
            });
        if let Some(value) = seconds.and_then(serde_json::Number::from_f64) {
            graph["update_time"] = Value::Number(value);
        }
    }
    Some(PaseoCachedTranscript {
        source: source.to_owned(),
        graph,
        url: doc.get("url").and_then(Value::as_str).map(str::to_owned),
    })
}

#[cfg(test)]
mod tests {
    use super::{parse_export, paseo_cached_transcript, read_input, MAX_BYTES, MAX_ENTRIES};
    use serde_json::json;

    #[test]
    fn bounded_json_export_parser_preserves_existing_envelope_rules() {
        assert_eq!(parse_export(br#"[{"id":"one"}]"#).unwrap().len(), 1);
        assert_eq!(parse_export(br#"{"conversations":[]}"#).unwrap().len(), 0);
        assert_eq!(parse_export(b"PK\x03\x04").unwrap_err().code, "invalid_request");
        assert_eq!(
            read_input(vec![0; MAX_BYTES + 1].as_slice())
                .unwrap_err()
                .code,
            "payload_too_large"
        );
        let entries = serde_json::to_vec(&vec![json!({}); MAX_ENTRIES + 1]).unwrap();
        assert_eq!(parse_export(&entries).unwrap_err().code, "invalid_request");
    }

    #[test]
    fn cached_transcript_remains_one_archive_node_without_invented_roles() {
        let source = json!({"kind":"conversation","source":"chatgpt","id":"chat-1",
            "text":"User: arbitrary text\nAssistant: not parsed as roles","updated":"1700000000"});
        let decoded = paseo_cached_transcript(&source).unwrap();
        assert_eq!(decoded.source, "chatgpt");
        assert_eq!(
            decoded.graph["mapping"]["paseo-cached-transcript"]["message"]["author"]["role"],
            "archive"
        );
        assert_eq!(
            decoded.graph["mapping"]["paseo-cached-transcript"]["message"]["content"]["parts"][0],
            source["text"]
        );
        assert!(paseo_cached_transcript(&json!({
            "kind": "conversation",
            "source": "unknown",
            "id": "x",
            "text": "x"
        }))
        .is_none());
    }
}
