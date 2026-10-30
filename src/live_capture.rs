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

#[derive(Clone, Copy)]
struct Limits {
    item: usize,
    retained: usize,
    queue: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            item: ITEM_BYTES,
            retained: RETAINED_BYTES,
            queue: QUEUE_ITEMS,
        }
    }
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
    limits: Limits,
    state: Arc<Mutex<State>>,
}
impl CaptureStatus {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            limits: Limits::default(),
            state: Arc::new(Mutex::new(State::default())),
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        match self.state.try_lock() {
            Ok(s) => Snapshot::Available {
                enabled: self.enabled,
                mode: "chatgpt_snapshot",
                per_capture_bytes: self.limits.item,
                retained_byte_limit: self.limits.retained,
                queue_limit: self.limits.queue,
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
    channel_with_limits(enabled, Limits::default())
}
fn channel_with_limits(enabled: bool, limits: Limits) -> (LiveSink, mpsc::Receiver<Work>) {
    let (tx, rx) = mpsc::channel(limits.queue);
    let mut status = CaptureStatus::new(enabled);
    status.limits = limits;
    (LiveSink { tx, status }, rx)
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
        let reserved = if !encoding
            && state.counters.retained_bytes + self.status.limits.item
                <= self.status.limits.retained
        {
            self.status.limits.item
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
                body: Vec::with_capacity(reserved),
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
        if work.body.len().saturating_add(bytes.len()) > self.sink.status.limits.item {
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
            work.body.clear();
            work.body.shrink_to_fit();
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
    pub(crate) fn ingest(self, store: &mut Store) -> bool {
        self.ingest_after(store, || {})
    }
    pub(crate) fn ingest_after(mut self, store: &mut Store, before_commit: impl FnOnce()) -> bool {
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
        before_commit();
        let result = (|| {
            if self.context.profile_scope != "provisional:default" {
                return Err(());
            }
            let raw: serde_json::Value = serde_json::from_slice(&self.body).map_err(|_| ())?;
            let mut doc = raw.clone();
            let id = source_id(&self.context.path).ok_or(())?;
            let object = doc.as_object_mut().ok_or(())?;
            let mut has_identity = false;
            for key in ["conversation_id", "id"] {
                match object.get(key) {
                    None | Some(serde_json::Value::Null) => {}
                    Some(serde_json::Value::String(source)) if source.is_empty() => {}
                    Some(serde_json::Value::String(source)) if source == id => {
                        has_identity = true;
                    }
                    _ => return Err(()),
                }
            }
            if !has_identity {
                object.insert("conversation_id".into(), id.into());
            }
            let mut observation = normalize_chatgpt_document(
                &doc,
                "chatgpt-live-v1",
                self.context.observed_unix_millis.min(i64::MAX as u128) as i64,
            )
            .map_err(|_| ())?;
            observation.raw_payload = raw;
            Ok(observation)
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

#[cfg(test)]
mod tests {
    use super::*;
    fn context() -> CaptureContext {
        CaptureContext::at_entry(
            "provisional:default",
            "GET",
            "/backend-api/conversation/synthetic",
        )
    }
    fn metadata() -> CaptureMetadata {
        CaptureMetadata {
            host: "chatgpt.com".into(),
            status: 200,
            ..Default::default()
        }
    }
    fn counters(status: &CaptureStatus) -> Counters {
        match status.snapshot() {
            Snapshot::Available { counters, .. } => {
                assert_eq!(
                    counters.started,
                    counters.active
                        + counters.committed_imported
                        + counters.committed_updated
                        + counters.committed_unchanged
                        + counters.invalid_snapshot
                        + counters.unsupported_encoding
                        + counters.oversized
                        + counters.dropped
                        + counters.interrupted
                        + counters.store_failed
                        + counters.shutdown_unprocessed
                );
                *counters
            }
            _ => panic!("unexpected unavailable snapshot"),
        }
    }
    #[test]
    fn default_off_ineligible_and_contended_status_do_not_invent_attempts() {
        let (off, _) = channel(false);
        assert!(off.begin(context(), metadata()).is_none());
        assert_eq!(counters(&off.status).started, 0);
        let (on, _) = channel(true);
        let mut ctx = context();
        ctx.method = "POST".into();
        assert!(on.begin(ctx, metadata()).is_none());
        assert_eq!(counters(&on.status).started, 0);
        let guard = on.status.state.lock().unwrap();
        assert!(matches!(on.status.snapshot(), Snapshot::Unavailable));
        drop(guard);
        assert_eq!(counters(&on.status).started, 0);
    }
    #[test]
    fn byte_budget_oversize_encoding_cancel_and_shutdown_are_single_terminal() {
        let (sink, _rx) = channel(true);
        let mut held = Vec::new();
        for _ in 0..RETAINED_BYTES / ITEM_BYTES {
            held.push(sink.begin(context(), metadata()).unwrap());
        }
        assert!(sink.begin(context(), metadata()).is_none());
        assert_eq!(counters(&sink.status).retained_bytes, RETAINED_BYTES);
        assert_eq!(counters(&sink.status).dropped, 1);
        drop(held);
        assert_eq!(counters(&sink.status).retained_bytes, 0);
        let mut oversized = sink.begin(context(), metadata()).unwrap();
        oversized.observe(&vec![b'x'; ITEM_BYTES]);
        oversized.observe(b"x");
        oversized.finish();
        assert_eq!(counters(&sink.status).oversized, 1);
        let mut encoded = metadata();
        encoded.content_encoding = Some("gzip".into());
        assert!(sink.begin(context(), encoded).is_none());
        assert_eq!(counters(&sink.status).unsupported_encoding, 1);
        let pending = sink.begin(context(), metadata()).unwrap();
        sink.status.stop();
        drop(pending);
        let c = counters(&sink.status);
        assert_eq!(c.shutdown_unprocessed, 1);
        assert_eq!(c.active, 0);
        assert_eq!(c.retained_bytes, 0);
    }
    #[test]
    fn closed_queue_and_queued_drop_release_budget_once() {
        let (sink, rx) = channel(true);
        drop(rx);
        sink.begin(context(), metadata()).unwrap().finish();
        let c = counters(&sink.status);
        assert_eq!(
            (c.dropped, c.admitted, c.queued, c.active, c.retained_bytes),
            (1, 0, 0, 0, 0)
        );
        let (sink, rx) = channel(true);
        sink.begin(context(), metadata()).unwrap().finish();
        assert_eq!(counters(&sink.status).queued, 1);
        sink.status.stop();
        drop(rx);
        let c = counters(&sink.status);
        assert_eq!(
            (c.shutdown_unprocessed, c.queued, c.active, c.retained_bytes),
            (1, 0, 0, 0)
        );
    }
    #[test]
    fn actual_store_failure_invalid_and_duplicate_outcomes_conserve_attempts() {
        let (sink, mut rx) = channel(true);
        let mut store = Store::in_memory().unwrap();
        let ingest = |bytes: &[u8], rx: &mut mpsc::Receiver<Work>, store: &mut Store| {
            let mut a = sink.begin(context(), metadata()).unwrap();
            a.observe(bytes);
            a.finish();
            rx.try_recv().unwrap().ingest(store)
        };
        assert!(!ingest(b"not-json", &mut rx, &mut store));
        let payload = br#"{"title":"synthetic","update_time":1,"mapping":{}}"#;
        assert!(ingest(payload, &mut rx, &mut store));
        let revision = store.library_revision().unwrap();
        assert!(ingest(payload, &mut rx, &mut store));
        assert_eq!(store.library_revision().unwrap(), revision);
        store.connection().execute_batch("CREATE TRIGGER synthetic_abort BEFORE INSERT ON observations BEGIN SELECT RAISE(ABORT,'PRIVATE_SQL_SENTINEL'); END;").unwrap();
        assert!(!ingest(
            br#"{"title":"change","update_time":2,"mapping":{}}"#,
            &mut rx,
            &mut store
        ));
        assert_eq!(store.library_revision().unwrap(), revision);
        let c = counters(&sink.status);
        assert_eq!(
            (
                c.invalid_snapshot,
                c.committed_imported,
                c.committed_unchanged,
                c.store_failed,
                c.active,
                c.retained_bytes
            ),
            (1, 1, 1, 1, 0, 0)
        );
        assert!(!serde_json::to_string(&sink.status.snapshot())
            .unwrap()
            .contains("PRIVATE_SQL_SENTINEL"));
    }

    #[test]
    fn entry_context_is_owned_until_normalization_and_durable_provenance() {
        let (sink, mut rx) = channel(true);
        let mut entry = context();
        entry.observed_unix_millis = 12345;
        entry.correlation = 7823;
        let mut accumulator = sink.begin(entry.clone(), metadata()).unwrap();
        entry.profile_scope = "changed-after-entry".into();
        entry.observed_unix_millis = 99999;
        let body = br#"{"title":"synthetic","update_time":1,"mapping":{}}"#;
        accumulator.observe(body);
        accumulator.finish();
        let work = rx.try_recv().unwrap();
        assert_eq!(work.context.correlation, 7823);
        let mut store = Store::in_memory().unwrap();
        assert!(work.ingest(&mut store));
        let (observed, parser, raw): (i64, String, String) = store
            .connection()
            .query_row(
                "SELECT observed_ms,parser,raw_payload FROM observations",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(observed, 12345);
        assert_eq!(parser, "chatgpt-live-v1");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&raw).unwrap(),
            serde_json::from_slice::<serde_json::Value>(body).unwrap()
        );
        assert_eq!(
            store.library_snapshot().unwrap().conversations[0]["account_scope"],
            "provisional:default"
        );
    }

    #[test]
    fn queue_limit_and_panic_have_exactly_one_terminal_disposition() {
        let (sink, mut rx) = channel_with_limits(
            true,
            Limits {
                item: 4,
                retained: 16,
                queue: 1,
            },
        );
        sink.begin(context(), metadata()).unwrap().finish();
        sink.begin(context(), metadata()).unwrap().finish();
        let c = counters(&sink.status);
        assert_eq!(
            (c.admitted, c.queued, c.dropped, c.active, c.retained_bytes),
            (1, 1, 1, 1, 4)
        );
        let work = rx.try_recv().unwrap();
        let mut store = Store::in_memory().unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            work.ingest_after(&mut store, || panic!("synthetic writer panic"))
        }));
        assert!(result.is_err());
        let c = counters(&sink.status);
        assert_eq!(
            (
                c.store_failed,
                c.active,
                c.writer_in_flight,
                c.retained_bytes
            ),
            (1, 0, 0, 0)
        );
    }

    #[test]
    fn unresolved_writer_remains_active_across_stop_then_commits_once() {
        let (sink, mut rx) = channel(true);
        let mut a = sink.begin(context(), metadata()).unwrap();
        a.observe(br#"{"title":"synthetic","update_time":1,"mapping":{}}"#);
        a.finish();
        let work = rx.try_recv().unwrap();
        let (entered, blocked) = std::sync::mpsc::channel();
        let (release, resume) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            let mut store = Store::in_memory().unwrap();
            work.ingest_after(&mut store, || {
                entered.send(()).unwrap();
                resume
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
            })
        });
        blocked
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        sink.status.stop();
        let c = counters(&sink.status);
        assert_eq!(
            (
                c.active,
                c.writer_in_flight,
                c.shutdown_unprocessed,
                c.committed_imported
            ),
            (1, 1, 0, 0)
        );
        release.send(()).unwrap();
        assert!(writer.join().unwrap());
        let c = counters(&sink.status);
        assert_eq!(
            (
                c.active,
                c.writer_in_flight,
                c.shutdown_unprocessed,
                c.committed_imported,
                c.retained_bytes
            ),
            (0, 0, 0, 1, 0)
        );
    }
}
