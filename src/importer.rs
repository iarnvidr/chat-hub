//! Bounded ChatGPT export ingestion shared by CLI and HTTP adapters.
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
    check_size(&bytes)?;
    Ok(bytes)
}

fn check_size(bytes: &[u8]) -> Result<(), ImportError> {
    if bytes.len() > MAX_BYTES {
        return Err(ImportError {
            code: "payload_too_large",
            message: "Import exceeds the 64 MiB limit.",
        });
    }
    Ok(())
}

/// Validate the entire global envelope before returning any items for ingestion.
pub fn parse_export(bytes: &[u8]) -> Result<Vec<Value>, ImportError> {
    check_size(bytes)?;
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

/// Commit each valid item independently. A disconnected caller cannot undo commits.
pub fn import_chatgpt(
    store: &mut crate::store::Store,
    bytes: &[u8],
) -> Result<ImportReceipt, ImportError> {
    import_scoped(
        store,
        bytes,
        "provisional:default",
        "chatgpt-export-v1",
        false,
    )
}

/// Dedicated trusted integration admission. Ordinary export files cannot select
/// this identity scope merely by including an owner field in their JSON.
pub fn import_paseo_history(
    store: &mut crate::store::Store,
    bytes: &[u8],
) -> Result<ImportReceipt, ImportError> {
    check_size(bytes)?;
    let envelope: Value = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    let owner = envelope
        .get("owner")
        .and_then(Value::as_str)
        .filter(|owner| {
            owner.len() == 24
                && owner
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        .ok_or_else(invalid)?;
    let cached = match envelope.get("format").and_then(Value::as_str) {
        Some("paseo-cached-transcripts-v1") => true,
        Some("chat-hub-paseo-history-v1") | None => false,
        _ => return Err(invalid()),
    };
    let scope = format!("{}:{owner}", if cached { "paseo-cache" } else { "paseo" });
    import_scoped(store, bytes, &scope, "chatgpt-paseo-history-v1", cached)
}

fn import_scoped(
    store: &mut crate::store::Store,
    bytes: &[u8],
    scope: &str,
    parser: &str,
    cached: bool,
) -> Result<ImportReceipt, ImportError> {
    use crate::store::IngestOutcome;
    let entries = parse_export(bytes)?;
    let mut receipt = ImportReceipt::default();
    for (item_index, doc) in entries.iter().enumerate() {
        let observed_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64;
        let result = if cached {
            normalize_paseo_cached_transcript(doc, observed_ms, scope)
        } else {
            crate::capture::normalize_chatgpt_document_in_scope(doc, parser, observed_ms, scope)
        };
        let failure = match result {
            Err(_) => Some(("invalid_item", "Conversation entry is invalid.")),
            Ok(observation) => match if cached {
                ingest_cached_if_changed(store, &observation)
            } else {
                store.ingest_observation_outcome(&observation)
            } {
                Ok(IngestOutcome::Imported { .. }) => {
                    receipt.imported += 1;
                    None
                }
                Ok(IngestOutcome::Updated { .. }) => {
                    receipt.updated += 1;
                    None
                }
                Ok(IngestOutcome::Unchanged { .. }) => {
                    receipt.unchanged += 1;
                    None
                }
                Err(_) => Some(("store_failed", "Conversation could not be committed.")),
            },
        };
        if let Some((code, message)) = failure {
            receipt.failed += 1;
            if receipt.errors.len() < 100 {
                receipt.errors.push(ItemError {
                    item_index,
                    code,
                    message,
                });
            } else {
                receipt.errors_truncated = true;
            }
        }
    }
    Ok(receipt)
}

fn ingest_cached_if_changed(
    store: &mut crate::store::Store,
    observation: &crate::store::Observation,
) -> crate::error::Result<crate::store::IngestOutcome> {
    use rusqlite::OptionalExtension;
    let previous: Option<(String, String)> = store.connection().query_row(
        "SELECT c.id,o.raw_payload FROM conversations c JOIN observations o ON o.conversation_id=c.id WHERE c.provider=?1 AND c.account_scope=?2 AND c.source_id=?3 AND o.parser=?4 ORDER BY o.id DESC LIMIT 1",
        rusqlite::params![observation.provider, observation.account_scope, observation.source_id, observation.parser],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional()?;
    if let Some((conversation_id, payload)) = previous {
        if payload == observation.raw_payload.to_string() {
            return Ok(crate::store::IngestOutcome::Unchanged { conversation_id });
        }
    }
    store.ingest_observation_outcome(observation)
}

fn normalize_paseo_cached_transcript(
    doc: &Value,
    observed_ms: i64,
    scope: &str,
) -> crate::error::Result<crate::store::Observation> {
    let invalid = || crate::error::Error::Configuration("Invalid Paseo cached conversation".into());
    let source = doc
        .get("source")
        .and_then(Value::as_str)
        .filter(|source| matches!(*source, "chatgpt" | "codex_local" | "codex_cloud"))
        .ok_or_else(invalid)?;
    if doc.get("kind").and_then(Value::as_str) != Some("conversation") {
        return Err(invalid());
    }
    let id = doc
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(invalid)?;
    let text = doc
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(invalid)?;
    // The plugin cache contains a rendered selected transcript, not the original
    // message graph. Preserve it as one archive record; never invent user/assistant
    // boundaries by parsing headings that may also occur inside message content.
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
    let mut observation = crate::capture::normalize_chatgpt_document_in_scope(
        &graph,
        "paseo-cache-transcript-v1",
        observed_ms,
        scope,
    )?;
    observation.provider = format!("paseo-{source}");
    observation.url = doc.get("url").and_then(Value::as_str).map(str::to_owned);
    observation.completeness = "cached_transcript".into();
    Ok(observation)
}
