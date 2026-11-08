//! Capture values and persistence contracts shared across independent adapters.

use crate::{IngestOutcome, Observation};
use serde::Serialize;

/// A bounded legacy HTTP response observation, independent of proxy transport.
#[derive(Clone, Debug)]
pub struct CaptureEvent {
    pub host: String,
    pub path: String,
    pub status: u16,
    pub body: Vec<u8>,
}

/// A redacted operation shape. The producer discards request/response values;
/// persistence receives only field names, types and the normalized route.
#[derive(Clone, Debug, Serialize)]
pub struct OperationObservation {
    pub method: String,
    pub route: String,
    pub status: u16,
    pub request_fields: String,
    pub response_fields: String,
    pub observed_at_ms: i64,
}

/// Commit an observation atomically and report its durable outcome.
///
/// Capture owns no database. The runtime supplies the persistence adapter, and
/// a successful result must mean its transaction has committed.
pub trait ObservationSink {
    type Error;

    fn store_observation(
        &mut self,
        observation: &Observation,
    ) -> Result<IngestOutcome, Self::Error>;
}

/// Validate a bounded provider/account scope without importing runtime config.
pub fn valid_capture_profile_scope(scope: &str) -> bool {
    !scope.is_empty()
        && scope.len() <= 256
        && scope
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'-' | b'_' | b'.'))
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Counters {
    pub started: u64,
    pub active: u64,
    pub committed_imported: u64,
    pub committed_updated: u64,
    pub committed_unchanged: u64,
    pub invalid_snapshot: u64,
    pub unsupported_encoding: u64,
    pub decode_failed: u64,
    pub oversized: u64,
    pub dropped: u64,
    pub interrupted: u64,
    pub store_failed: u64,
    pub shutdown_unprocessed: u64,
    pub admitted: u64,
    pub completed_responses: u64,
    pub queued: u64,
    pub writer_in_flight: u64,
    pub retained_bytes: usize,
    pub lost_observations: u64,
    pub last_error_code: Option<&'static str>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Snapshot {
    Available {
        enabled: bool,
        mode: &'static str,
        per_capture_bytes: usize,
        retained_byte_limit: usize,
        queue_limit: usize,
        counters: Box<Counters>,
    },
    Unavailable,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_scope_contract_is_bounded_and_ascii_only() {
        for scope in ["provisional:default", "chatgpt:observed:abc-123_4.5", "x"] {
            assert!(valid_capture_profile_scope(scope));
        }
        assert!(valid_capture_profile_scope(&"a".repeat(256)));
        for scope in ["", "with space", "a/b", "a\\b", "å", "a\nb"] {
            assert!(!valid_capture_profile_scope(scope));
        }
        assert!(!valid_capture_profile_scope(&"a".repeat(257)));
    }

    #[test]
    fn capture_status_wire_shape_stays_stable_without_capture_runtime() {
        assert_eq!(
            serde_json::to_value(Snapshot::Unavailable).unwrap(),
            serde_json::json!({"status": "unavailable"})
        );
        let status = Snapshot::Available {
            enabled: false,
            mode: "chatgpt_snapshot_and_sse",
            per_capture_bytes: 2 * 1024 * 1024,
            retained_byte_limit: 16 * 1024 * 1024,
            queue_limit: 32,
            counters: Box::default(),
        };
        let value = serde_json::to_value(status).unwrap();
        assert_eq!(value["status"], "available");
        assert_eq!(value["enabled"], false);
        assert_eq!(value["counters"]["active"], 0);
    }
}
