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
    use crate::store::IngestOutcome;
    let entries = parse_export(bytes)?;
    let mut receipt = ImportReceipt::default();
    for (item_index, doc) in entries.iter().enumerate() {
        let observed_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64;
        let result =
            crate::capture::normalize_chatgpt_document(doc, "chatgpt-export-v1", observed_ms);
        let failure = match result {
            Err(_) => Some(("invalid_item", "Conversation entry is invalid.")),
            Ok(observation) => match store.ingest_observation_outcome(&observation) {
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
