//! Best-effort snapshot staging. Only committed Store outcomes are durable.
use crate::{
    capture::normalize_chatgpt_document,
    capture_lane::{CaptureContext, CaptureMetadata},
    store::{IngestOutcome, Store},
};
use serde::Serialize;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

pub const ITEM_BYTES: usize = 2 * 1024 * 1024;
pub const RETAINED_BYTES: usize = 16 * 1024 * 1024;
pub const QUEUE_ITEMS: usize = 32;

#[derive(Clone, Debug, Default, Serialize)]
pub struct Counters {
    pub started: u64,
    pub active: u64,
    pub committed_imported: u64,
    pub committed_updated: u64,
    pub committed_unchanged: u64,
    pub invalid_snapshot: u64,
    pub unsupported_encoding: u64,
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
#[derive(Default)]
struct State {
    counters: Counters,
    stopping: bool,
}
#[derive(Clone)]
pub struct CaptureStatus {
    enabled: bool,
    state: Arc<Mutex<State>>,
}
impl CaptureStatus {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            state: Arc::new(Mutex::new(State::default())),
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        match self.state.try_lock() {
            Ok(s) => Snapshot::Available {
                enabled: self.enabled,
                mode: "chatgpt_snapshot",
                per_capture_bytes: ITEM_BYTES,
                retained_byte_limit: RETAINED_BYTES,
                queue_limit: QUEUE_ITEMS,
                counters: Box::new(s.counters.clone()),
            },
            Err(_) => Snapshot::Unavailable,
        }
    }
    pub fn stop(&self) {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .stopping = true;
    }
}
#[derive(Clone, Copy)]
enum Outcome {
    Imported,
    Updated,
    Unchanged,
    Invalid,
    Encoding,
    Oversized,
    Dropped,
    Interrupted,
    StoreFailed,
    Shutdown,
}
#[derive(Clone, Copy, PartialEq)]
enum Stage {
    Accumulating,
    Queued,
    Writing,
}
struct Attempt {
    status: CaptureStatus,
    reserved: usize,
    stage: Stage,
    done: bool,
}
impl Attempt {
    fn finish(&mut self, outcome: Outcome) {
        if self.done {
            return;
        }
        let mut s = self.status.state.lock().unwrap_or_else(|p| p.into_inner());
        let c = &mut s.counters;
        c.active -= 1;
        c.retained_bytes -= self.reserved;
        match self.stage {
            Stage::Queued => c.queued -= 1,
            Stage::Writing => c.writer_in_flight -= 1,
            _ => {}
        }
        let (counter, code) = match outcome {
            Outcome::Imported => (&mut c.committed_imported, None),
            Outcome::Updated => (&mut c.committed_updated, None),
            Outcome::Unchanged => (&mut c.committed_unchanged, None),
            Outcome::Invalid => (&mut c.invalid_snapshot, Some("invalid_snapshot")),
            Outcome::Encoding => (&mut c.unsupported_encoding, Some("unsupported_encoding")),
            Outcome::Oversized => (&mut c.oversized, Some("capture_oversized")),
            Outcome::Dropped => (&mut c.dropped, Some("capture_dropped")),
            Outcome::Interrupted => (&mut c.interrupted, Some("capture_interrupted")),
            Outcome::StoreFailed => (&mut c.store_failed, Some("capture_store_failed")),
            Outcome::Shutdown => (
                &mut c.shutdown_unprocessed,
                Some("capture_shutdown_unprocessed"),
            ),
        };
        *counter += 1;
        if code.is_some() {
            c.last_error_code = code;
        }
        self.done = true;
    }
}
impl Drop for Attempt {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        let stopping = self
            .status
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .stopping;
        self.finish(if self.stage == Stage::Writing {
            Outcome::StoreFailed
        } else if stopping {
            Outcome::Shutdown
        } else {
            Outcome::Interrupted
        });
    }
}

#[derive(Clone)]
pub struct LiveSink {
    tx: mpsc::Sender<Work>,
    pub status: CaptureStatus,
}
pub(crate) struct Work {
    context: CaptureContext,
    body: Vec<u8>,
    attempt: Attempt,
}
pub struct Accumulator {
    sink: LiveSink,
    work: Option<Work>,
}
pub(crate) fn channel(enabled: bool) -> (LiveSink, mpsc::Receiver<Work>) {
    let (tx, rx) = mpsc::channel(QUEUE_ITEMS);
    (
        LiveSink {
            tx,
            status: CaptureStatus::new(enabled),
        },
        rx,
    )
}
pub fn eligible(context: &CaptureContext, metadata: &CaptureMetadata) -> bool {
    context.method == "GET"
        && metadata.host.eq_ignore_ascii_case("chatgpt.com")
        && (200..300).contains(&metadata.status)
        && source_id(&context.path).is_some()
        && metadata.content_type.as_deref().is_none_or(|t| {
            t.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("application/json")
        })
}
fn source_id(path: &str) -> Option<&str> {
    path.split('?')
        .next()?
        .strip_prefix("/backend-api/conversation/")
        .filter(|id| !id.is_empty() && !id.contains('/'))
}
impl LiveSink {
    pub fn begin(&self, context: CaptureContext, metadata: CaptureMetadata) -> Option<Accumulator> {
        if !self.status.enabled || !eligible(&context, &metadata) {
            return None;
        }
        let mut state = self.status.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.stopping {
            return None;
        }
        state.counters.started += 1;
        state.counters.active += 1;
        let encoding = metadata
            .content_encoding
            .as_deref()
            .is_some_and(|e| !e.eq_ignore_ascii_case("identity"));
        let reserved = if !encoding && state.counters.retained_bytes + ITEM_BYTES <= RETAINED_BYTES
        {
            ITEM_BYTES
        } else {
            0
        };
        state.counters.retained_bytes += reserved;
        drop(state);
        let mut attempt = Attempt {
            status: self.status.clone(),
            reserved,
            stage: Stage::Accumulating,
            done: false,
        };
        if encoding || reserved == 0 {
            attempt.finish(if encoding {
                Outcome::Encoding
            } else {
                Outcome::Dropped
            });
            return None;
        }
        Some(Accumulator {
            sink: self.clone(),
            work: Some(Work {
                context,
                body: Vec::with_capacity(ITEM_BYTES),
                attempt,
            }),
        })
    }
}
impl Accumulator {
    pub fn observe(&mut self, bytes: &[u8]) {
        let Some(work) = &mut self.work else {
            return;
        };
        if work.body.len().saturating_add(bytes.len()) > ITEM_BYTES {
            let mut work = self.work.take().unwrap();
            work.body.clear();
            work.body.shrink_to_fit();
            work.attempt.finish(Outcome::Oversized);
        } else {
            work.body.extend_from_slice(bytes);
        }
    }
    pub fn finish(mut self) {
        let Some(mut work) = self.work.take() else {
            return;
        };
        // Hold the short state lock through channel admission so a dequeue cannot
        // observe a queued gauge before admission accounting is complete.
        let state = work.attempt.status.state.clone();
        let mut s = state.lock().unwrap_or_else(|p| p.into_inner());
        s.counters.completed_responses += 1;
        if s.stopping {
            drop(s);
            work.attempt.finish(Outcome::Shutdown);
            return;
        }
        work.attempt.stage = Stage::Queued;
        s.counters.queued += 1;
        match self.sink.tx.try_send(work) {
            Ok(()) => s.counters.admitted += 1,
            Err(error) => {
                drop(s);
                let mut work = error.into_inner();
                work.body.clear();
                work.body.shrink_to_fit();
                work.attempt.finish(Outcome::Dropped);
            }
        }
    }
    pub fn interrupt(self) {
        drop(self);
    }
}
impl Work {
    pub(crate) fn ingest(mut self, store: &mut Store) -> bool {
        {
            let mut s = self
                .attempt
                .status
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            s.counters.queued -= 1;
            s.counters.writer_in_flight += 1;
            self.attempt.stage = Stage::Writing;
        }
        let result = (|| {
            if self.context.profile_scope != "provisional:default" {
                return Err(());
            }
            let mut doc: serde_json::Value = serde_json::from_slice(&self.body).map_err(|_| ())?;
            let id = source_id(&self.context.path).ok_or(())?;
            let object = doc.as_object_mut().ok_or(())?;
            if let Some(source) = object
                .get("conversation_id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                if source != id {
                    return Err(());
                }
            } else {
                object.insert("conversation_id".into(), id.into());
            }
            normalize_chatgpt_document(
                &doc,
                "chatgpt-live-v1",
                self.context.observed_unix_millis.min(i64::MAX as u128) as i64,
            )
            .map_err(|_| ())
        })();
        let outcome = match result {
            Err(()) => Outcome::Invalid,
            Ok(observation) => match store.ingest_observation_outcome(&observation) {
                Ok(IngestOutcome::Imported { .. }) => Outcome::Imported,
                Ok(IngestOutcome::Updated { .. }) => Outcome::Updated,
                Ok(IngestOutcome::Unchanged { .. }) => Outcome::Unchanged,
                Err(_) => Outcome::StoreFailed,
            },
        };
        // Release source allocation before its budget ownership.
        self.body.clear();
        self.body.shrink_to_fit();
        self.attempt.finish(outcome);
        matches!(
            outcome,
            Outcome::Imported | Outcome::Updated | Outcome::Unchanged
        )
    }
}
