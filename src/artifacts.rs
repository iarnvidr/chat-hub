//! Metadata for generated files whose bytes are saved in the local library.
use serde::Serialize;

pub const MAX_ARTIFACT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SavedArtifact {
    pub entity_id: String,
    pub conversation_id: String,
    pub filename: String,
    pub mime_type: Option<String>,
    pub byte_size: i64,
    pub sha256: String,
    pub saved_at_ms: i64,
}

pub fn valid_filename(name: &str) -> bool {
    !name.trim().is_empty()
        && name.len() <= 255
        && !matches!(name, "." | "..")
        && !name.ends_with('.')
        && !name.ends_with(' ')
        && !name.chars().any(|c| {
            c.is_control() || matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')
        })
}

pub fn content_sha256(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
