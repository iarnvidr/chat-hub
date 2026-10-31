//! Best-effort snapshot staging. Only committed Store outcomes are durable.
use crate::{
    capture_lane::{CaptureContext, CaptureMetadata},
    store::{IngestOutcome, Store},
};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::{
    io::Read,
    sync::{Arc, Mutex},
};
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
                mode: "chatgpt_snapshot_and_sse",
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
#[derive(Clone, Copy, Debug)]
enum Outcome {
    Imported,
    Updated,
    Unchanged,
    Invalid,
    Encoding,
    DecodeFailed,
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
            Outcome::DecodeFailed => (&mut c.decode_failed, Some("capture_decode_failed")),
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
    metadata: CaptureMetadata,
    encoding: Encoding,
    body: Vec<u8>,
    attempt: Attempt,
}
#[derive(Clone, Copy)]
enum Encoding {
    Identity,
    Gzip,
    Deflate,
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
    let path = context.path.split('?').next().unwrap_or(&context.path);
    let content_type = metadata
        .content_type
        .as_deref()
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    let snapshot = context.method == "GET"
        && source_id(path).is_some()
        && content_type.is_none_or(|value| value.eq_ignore_ascii_case("application/json"));
    let stream = context.method == "POST"
        && path == "/backend-api/conversation"
        && content_type.is_some_and(|value| value.eq_ignore_ascii_case("text/event-stream"));
    (snapshot || stream)
        && metadata.host.eq_ignore_ascii_case("chatgpt.com")
        && (200..300).contains(&metadata.status)
}
fn source_id(path: &str) -> Option<&str> {
    path.split('?')
        .next()?
        .strip_prefix("/backend-api/conversation/")
        .filter(|id| !id.is_empty() && !id.contains('/'))
}
impl LiveSink {
    pub(crate) fn request_capture_enabled(&self) -> bool {
        self.status.enabled
            && !self
                .status
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .stopping
    }

    pub fn begin(&self, context: CaptureContext, metadata: CaptureMetadata) -> Option<Accumulator> {
        if !self.status.enabled
            || !valid_profile_scope(&context.profile_scope)
            || !eligible(&context, &metadata)
        {
            return None;
        }
        let mut state = self.status.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.stopping {
            return None;
        }
        state.counters.started += 1;
        state.counters.active += 1;
        let encoding = match parse_encoding(metadata.content_encoding.as_deref()) {
            Ok(encoding) => encoding,
            Err(()) => {
                drop(state);
                let mut attempt = Attempt {
                    status: self.status.clone(),
                    reserved: 0,
                    stage: Stage::Accumulating,
                    done: false,
                };
                attempt.finish(Outcome::Encoding);
                return None;
            }
        };
        let reserved = if state.counters.retained_bytes + self.status.limits.item
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
        if reserved == 0 {
            attempt.finish(Outcome::Dropped);
            return None;
        }
        Some(Accumulator {
            sink: self.clone(),
            work: Some(Work {
                context,
                metadata,
                encoding,
                body: Vec::with_capacity(reserved),
                attempt,
            }),
        })
    }
}
fn valid_profile_scope(scope: &str) -> bool {
    crate::config::valid_capture_profile_scope(scope)
}
fn parse_encoding(value: Option<&str>) -> Result<Encoding, ()> {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        None => Ok(Encoding::Identity),
        Some(value) if value.eq_ignore_ascii_case("identity") => Ok(Encoding::Identity),
        Some(value)
            if value.eq_ignore_ascii_case("gzip") || value.eq_ignore_ascii_case("x-gzip") =>
        {
            Ok(Encoding::Gzip)
        }
        Some(value) if value.eq_ignore_ascii_case("deflate") => Ok(Encoding::Deflate),
        Some(_) => Err(()),
    }
}
fn decode_body(encoding: Encoding, body: &[u8], limit: usize) -> Result<Vec<u8>, Outcome> {
    let mut decoded = Vec::with_capacity(body.len().min(limit));
    let result = match encoding {
        Encoding::Identity => {
            decoded.extend_from_slice(body);
            Ok(decoded.len())
        }
        Encoding::Gzip => flate2::read::MultiGzDecoder::new(body)
            .take(limit as u64 + 1)
            .read_to_end(&mut decoded),
        Encoding::Deflate => flate2::read::ZlibDecoder::new(body)
            .take(limit as u64 + 1)
            .read_to_end(&mut decoded),
    };
    if result.is_err() {
        return Err(Outcome::DecodeFailed);
    }
    if decoded.len() > limit {
        return Err(Outcome::Oversized);
    }
    Ok(decoded)
}
pub(crate) fn decode_schema_body(encoding: &str, body: &[u8], limit: usize) -> Option<Vec<u8>> {
    decode_body(parse_encoding(Some(encoding)).ok()?, body, limit).ok()
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
        let decoded = match decode_body(self.encoding, &self.body, self.attempt.status.limits.item)
        {
            Ok(body) => body,
            Err(outcome) => {
                self.body.clear();
                self.body.shrink_to_fit();
                self.attempt.finish(outcome);
                return false;
            }
        };
        let is_stream = self.metadata.content_type.as_deref().is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("text/event-stream"))
        });
        let result = (|| {
            let raw: Value = if is_stream {
                let request_body = self
                    .context
                    .request_body
                    .as_ref()
                    .and_then(|capture| {
                        capture
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .body()
                            .map(ToOwned::to_owned)
                    })
                    .ok_or(())?;
                parse_sse_document(&decoded, Some(&request_body))?
            } else {
                serde_json::from_slice(&decoded).map_err(|_| ())?
            };
            let mut doc = raw.clone();
            let id = source_id(&self.context.path)
                .map(str::to_owned)
                .or_else(|| doc["conversation_id"].as_str().map(str::to_owned))
                .ok_or(())?;
            let object = doc.as_object_mut().ok_or(())?;
            let mut has_identity = false;
            for key in ["conversation_id", "id"] {
                match object.get(key) {
                    None | Some(serde_json::Value::Null) => {}
                    Some(serde_json::Value::String(source)) if source.is_empty() => {}
                    Some(serde_json::Value::String(source)) if source == &id => {
                        has_identity = true;
                    }
                    _ => return Err(()),
                }
            }
            if !has_identity {
                object.insert("conversation_id".into(), id.clone().into());
            }
            let mut observation = crate::capture::normalize_chatgpt_document_in_scope(
                &doc,
                if is_stream {
                    "chatgpt-sse-v1"
                } else {
                    "chatgpt-live-v1"
                },
                self.context.observed_unix_millis.min(i64::MAX as u128) as i64,
                self.context.profile_scope.clone(),
            )
            .map_err(|_| ())?;
            observation.raw_payload = if is_stream {
                Value::String(String::from_utf8(decoded.clone()).map_err(|_| ())?)
            } else {
                raw
            };
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

fn parse_sse_document(body: &[u8], request_body: Option<&[u8]>) -> Result<Value, ()> {
    let text = std::str::from_utf8(body).map_err(|_| ())?;
    let mut stream = SseDocument::default();
    if let Some(request_body) = request_body {
        let request: Value = serde_json::from_slice(request_body).map_err(|_| ())?;
        if let Some(id) = request
            .get("conversation_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        {
            stream.conversation_id = Some(id.to_owned());
        }
        let messages = request
            .get("messages")
            .and_then(Value::as_array)
            .filter(|messages| !messages.is_empty())
            .ok_or(())?;
        let parent_message_id = request
            .get("parent_message_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty());
        for (index, request_message) in messages.iter().enumerate() {
            let Some(id) = request_message
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
            else {
                return Err(());
            };
            let mut message = request_message.clone();
            if message.get("parent_id").is_none() && index == 0 {
                if let Some(parent) = parent_message_id {
                    message["parent_id"] = Value::String(parent.to_owned());
                }
            }
            stream.current_node = Some(id.to_owned());
            stream.messages.insert(id.to_owned(), message);
        }
    }
    let mut data = Vec::new();
    for raw_line in text.lines() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.is_empty() {
            dispatch_sse_event(&mut stream, &mut data)?;
        } else if line.starts_with(':') {
            continue;
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push(value.strip_prefix(' ').unwrap_or(value).to_owned());
        }
    }
    dispatch_sse_event(&mut stream, &mut data)?;
    if !stream.done || stream.conversation_id.is_none() || stream.messages.is_empty() {
        return Err(());
    }
    let conversation_id = stream.conversation_id.ok_or(())?;
    let mut mapping = Map::new();
    for (message_id, message) in stream.messages {
        mapping.insert(
            message_id,
            json!({"parent": message.get("parent_id").cloned().unwrap_or(Value::Null), "message": message}),
        );
    }
    let current_node = stream
        .current_node
        .filter(|node| mapping.contains_key(node));
    Ok(json!({
        "conversation_id": conversation_id,
        "title": stream.title.unwrap_or_default(),
        "current_node": current_node,
        "mapping": mapping,
    }))
}

#[derive(Default)]
struct SseDocument {
    conversation_id: Option<String>,
    title: Option<String>,
    messages: BTreeMap<String, Value>,
    current_node: Option<String>,
    done: bool,
}

fn dispatch_sse_event(stream: &mut SseDocument, data: &mut Vec<String>) -> Result<(), ()> {
    if data.is_empty() {
        return Ok(());
    }
    let event = data.join("\n");
    data.clear();
    if event.trim() == "[DONE]" {
        stream.done = true;
        return Ok(());
    }
    if stream.done {
        return Err(());
    }
    let value: Value = serde_json::from_str(&event).map_err(|_| ())?;
    if let Some(id) = value.get("conversation_id") {
        match id {
            Value::Null => {}
            Value::String(id) if id.is_empty() => {}
            Value::String(id) => {
                if stream
                    .conversation_id
                    .as_deref()
                    .is_some_and(|current| current != id)
                {
                    return Err(());
                }
                stream.conversation_id = Some(id.clone());
            }
            _ => return Err(()),
        }
    }
    for key in ["title", "conversation_title"] {
        if let Some(title) = value
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            stream.title = Some(title.to_owned());
        }
    }
    let message = value
        .get("message")
        .or_else(|| value.get("v").and_then(|v| v.get("message")));
    if let Some(message) = message.filter(|message| message.is_object()) {
        let Some(id) = message
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        else {
            return Err(());
        };
        stream.current_node = Some(id.to_owned());
        stream.messages.insert(id.to_owned(), message.clone());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture_lane::RequestBodyCapture;
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
    fn stream_context() -> CaptureContext {
        CaptureContext::at_entry("provisional:default", "POST", "/backend-api/conversation")
    }
    fn stream_metadata() -> CaptureMetadata {
        CaptureMetadata {
            content_type: Some("text/event-stream; charset=utf-8".into()),
            ..metadata()
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
                        + counters.decode_failed
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
    fn only_supported_chatgpt_snapshot_and_sse_routes_are_eligible() {
        let stream = stream_context();
        let meta = stream_metadata();
        assert!(eligible(&stream, &meta));
        let mut wrong_host = meta.clone();
        wrong_host.host = "accounts.chatgpt.com".into();
        assert!(!eligible(&stream, &wrong_host));
        let mut wrong_type = meta.clone();
        wrong_type.content_type = Some("application/json".into());
        assert!(!eligible(&stream, &wrong_type));
        let mut wrong_path = stream.clone();
        wrong_path.path = "/backend-api/other".into();
        assert!(!eligible(&wrong_path, &meta));
        assert!(eligible(&context(), &metadata()));
    }
    #[test]
    fn complete_sse_stream_replays_message_updates_into_a_branched_graph() {
        let request_body = json!({
            "conversation_id": null,
            "parent_message_id": "root-node",
            "messages": [{
                "id": "user-node",
                "author": {"role":"user"},
                "content": {"content_type":"text","parts":["streamed question"]}
            }]
        })
        .to_string();
        let mut body = String::new();
        for value in [
            json!({"conversation_id":"stream-id","title":"Streamed title"}),
            json!({"conversation_id":"stream-id","message":{"id":"assistant-node","parent_id":"user-node","author":{"role":"assistant"},"content":{"content_type":"text","parts":["partial answer"]}}}),
            json!({"conversation_id":"stream-id","message":{"id":"assistant-node","parent_id":"user-node","author":{"role":"assistant"},"content":{"content_type":"text","parts":["completed answer"]}}}),
        ] {
            body.push_str("data: ");
            body.push_str(&value.to_string());
            body.push_str("\r\n\r\n");
        }
        body.push_str("data: [DONE]\r\n\r\n");

        let parsed = parse_sse_document(body.as_bytes(), Some(request_body.as_bytes())).unwrap();
        assert_eq!(parsed["conversation_id"], "stream-id");
        assert_eq!(parsed["title"], "Streamed title");
        assert_eq!(parsed["current_node"], "assistant-node");
        assert_eq!(parsed["mapping"].as_object().unwrap().len(), 2);
        assert_eq!(parsed["mapping"]["user-node"]["parent"], "root-node");
        assert_eq!(
            parsed["mapping"]["assistant-node"]["message"]["content"]["parts"][0],
            "completed answer"
        );

        let (sink, mut rx) = channel(true);
        let request_permit = Arc::new(tokio::sync::Semaphore::new(1))
            .try_acquire_owned()
            .unwrap();
        let request_capture = Arc::new(Mutex::new(RequestBodyCapture::new(
            request_permit,
            request_body.len(),
        )));
        {
            let mut request_capture = request_capture.lock().unwrap();
            request_capture.observe(request_body.as_bytes());
            request_capture.complete();
        }
        let mut context = stream_context();
        context.request_body = Some(request_capture);
        let mut capture = sink.begin(context, stream_metadata()).unwrap();
        capture.observe(body.as_bytes());
        capture.finish();
        let mut store = Store::in_memory().unwrap();
        assert!(rx.try_recv().unwrap().ingest(&mut store));
        let page = store.library_snapshot().unwrap();
        assert_eq!(page.conversations[0]["title"], "Streamed title");
        let conversation_id = page.conversations[0]["id"].as_str().unwrap();
        let detail = store.read_normalized(conversation_id).unwrap().unwrap();
        assert_eq!(detail["messages"].as_array().unwrap().len(), 2);
        assert_eq!(detail["selected_node_id"], "assistant-node");
        assert_eq!(store.search("completed answer", 10).unwrap().len(), 1);
        let c = counters(&sink.status);
        assert_eq!(c.committed_imported, 1);
        assert_eq!(c.invalid_snapshot, 0);
    }
    #[test]
    fn incomplete_or_cross_conversation_sse_is_not_normalized() {
        let request =
            br#"{"messages":[{"id":"user","author":{"role":"user"},"content":{"parts":["q"]}}]}"#;
        let incomplete = b"data: {\"conversation_id\":\"c\",\"message\":{\"id\":\"m\"}}\n\n";
        assert!(parse_sse_document(incomplete, Some(request)).is_err());
        let mismatch = concat!(
            "data: {\"conversation_id\":\"c\",\"message\":{\"id\":\"m1\"}}\n\n",
            "data: {\"conversation_id\":\"other\",\"message\":{\"id\":\"m2\"}}\n\n",
            "data: [DONE]\n\n"
        );
        assert!(parse_sse_document(mismatch.as_bytes(), Some(request)).is_err());
        let malformed_message = concat!(
            "data: {\"conversation_id\":\"c\",\"message\":{\"id\":\"m1\"}}\n\n",
            "data: {\"conversation_id\":\"c\",\"message\":{\"author\":{\"role\":\"assistant\"}}}\n\n",
            "data: [DONE]\n\n"
        );
        assert!(parse_sse_document(malformed_message.as_bytes(), Some(request)).is_err());
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
        encoded.content_encoding = Some("br".into());
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
    fn gzip_and_deflate_snapshots_decode_with_a_hard_output_limit() {
        use flate2::{write::GzEncoder, Compression};
        use std::io::Write;
        let payload = br#"{"title":"compressed","update_time":1,"mapping":{}}"#;
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(payload).unwrap();
        let gzip = encoder.finish().unwrap();
        let decoded = decode_body(Encoding::Gzip, &gzip, ITEM_BYTES).unwrap();
        assert_eq!(decoded, payload);
        let mut first = GzEncoder::new(Vec::new(), Compression::default());
        first.write_all(b"{\"title\":\"").unwrap();
        let first = first.finish().unwrap();
        let mut second = GzEncoder::new(Vec::new(), Compression::default());
        second.write_all(b"joined\",\"mapping\":{}}").unwrap();
        let mut joined = first;
        joined.extend(second.finish().unwrap());
        assert_eq!(
            decode_body(Encoding::Gzip, &joined, ITEM_BYTES).unwrap(),
            br#"{"title":"joined","mapping":{}}"#
        );
        let invalid = decode_body(Encoding::Gzip, b"not gzip", ITEM_BYTES).unwrap_err();
        assert!(matches!(invalid, Outcome::DecodeFailed));
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(payload).unwrap();
        let deflate = encoder.finish().unwrap();
        assert_eq!(
            decode_body(Encoding::Deflate, &deflate, ITEM_BYTES).unwrap(),
            payload
        );
        let bomb = vec![b'x'; ITEM_BYTES + 1];
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&bomb).unwrap();
        assert!(matches!(
            decode_body(Encoding::Gzip, &encoder.finish().unwrap(), ITEM_BYTES),
            Err(Outcome::Oversized)
        ));
    }
    #[test]
    fn compressed_snapshot_persists_after_decode() {
        use flate2::{write::GzEncoder, Compression};
        use std::io::Write;
        let (sink, mut rx) = channel(true);
        let payload = br#"{"title":"compressed snapshot","update_time":1,"mapping":{}}"#;
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(payload).unwrap();
        let bytes = encoder.finish().unwrap();
        let mut meta = metadata();
        meta.content_encoding = Some("gzip".into());
        let mut capture = sink.begin(context(), meta).unwrap();
        capture.observe(&bytes);
        capture.finish();
        let mut store = Store::in_memory().unwrap();
        assert!(rx.try_recv().unwrap().ingest(&mut store));
        assert_eq!(
            store.library_snapshot().unwrap().conversations[0]["title"],
            "compressed snapshot"
        );
        assert_eq!(counters(&sink.status).committed_imported, 1);
        assert_eq!(counters(&sink.status).decode_failed, 0);
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
    fn caller_profile_scope_survives_capture_and_prevents_cross_profile_merge() {
        let (sink, mut rx) = channel(true);
        let body = br#"{"title":"same source id","update_time":1,"mapping":{}}"#;
        let mut store = Store::in_memory().unwrap();
        for profile in ["profile:one", "profile:two"] {
            let mut context = context();
            context.profile_scope = profile.into();
            let mut capture = sink.begin(context, metadata()).unwrap();
            capture.observe(body);
            capture.finish();
            assert!(rx.try_recv().unwrap().ingest(&mut store));
        }
        let rows = store.library_snapshot().unwrap().conversations;
        assert_eq!(rows.len(), 2);
        let mut scopes: Vec<_> = rows
            .iter()
            .map(|row| row["account_scope"].as_str().unwrap())
            .collect();
        scopes.sort_unstable();
        assert_eq!(scopes, ["profile:one", "profile:two"]);
    }

    #[test]
    fn invalid_or_overlong_profile_scope_is_rejected_without_capture() {
        let (sink, _) = channel(true);
        let mut invalid = context();
        invalid.profile_scope = "private/account@example.test".into();
        assert!(sink.begin(invalid, metadata()).is_none());
        let too_long = "p".repeat(257);
        let invalid =
            CaptureContext::at_entry(too_long, "GET", "/backend-api/conversation/synthetic");
        assert!(invalid.profile_scope.is_empty());
        assert!(sink.begin(invalid, metadata()).is_none());
        assert_eq!(counters(&sink.status).started, 0);
    }

    #[test]
    fn request_account_switch_with_reversed_response_order_preserves_two_libraries() {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        let resolver = crate::request_identity::RequestIdentity::new().unwrap();
        let request = |account: &str| {
            let payload = json!({"https://api.openai.com/auth":{"chatgpt_account_id":account,"chatgpt_user_id":"synthetic-user"}});
            let mut headers = hyper::HeaderMap::new();
            headers.insert(
                "authorization",
                format!(
                    "Bearer e30.{}.PRIVATE_SIGNATURE",
                    URL_SAFE_NO_PAD.encode(payload.to_string())
                )
                .parse()
                .unwrap(),
            );
            let scope = resolver.resolve("provisional:default", &headers).unwrap();
            CaptureContext::at_entry(scope, "GET", "/backend-api/conversation/same-source")
        };
        let first = request("account-one");
        let second = request("account-two");
        let scopes = [first.profile_scope.clone(), second.profile_scope.clone()];
        let (sink, mut rx) = channel(true);
        let mut first_capture = sink.begin(first, metadata()).unwrap();
        let mut second_capture = sink.begin(second, metadata()).unwrap();
        let body = br#"{"title":"shared source ID","mapping":{}}"#;
        first_capture.observe(body);
        second_capture.observe(body);
        second_capture.finish();
        first_capture.finish();
        let mut store = Store::in_memory().unwrap();
        assert!(rx.try_recv().unwrap().ingest(&mut store));
        assert!(rx.try_recv().unwrap().ingest(&mut store));
        let rows = store.library_snapshot().unwrap().conversations;
        assert_eq!(rows.len(), 2);
        for scope in scopes {
            assert_eq!(
                rows.iter()
                    .filter(|row| row["account_scope"] == scope)
                    .count(),
                1
            );
        }
        assert!(!serde_json::to_string(&rows)
            .unwrap()
            .contains("PRIVATE_SIGNATURE"));
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
