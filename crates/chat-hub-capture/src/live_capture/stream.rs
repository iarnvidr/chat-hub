//! Live stream view of a ChatGPT generation response.
//!
//! While the proxy tees a `POST /backend-api/(f/)conversation` event stream, the
//! bytes are also fed here, and only while someone subscribes. The existing
//! delta-encoding decoder replays each complete SSE event; the growth of the
//! assistant text is published as small events. Forwarding and the completed
//! snapshot persistence never depend on this module. Events carry assistant text
//! parts only: no headers, tokens, user text or resume tokens.
use super::{dispatch_sse_event, SseDocument};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use tokio::sync::broadcast;

/// Events buffered per subscriber before it lags and must resynchronise.
pub const EVENT_QUEUE: usize = 256;
/// Largest text chunk in one event.
pub const MAX_DELTA_BYTES: usize = 4096;
/// Assistant messages tracked in one response.
const MAX_MESSAGES: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveEventKind {
    Start,
    Delta,
    Done,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LiveEvent {
    pub conversation_id: Option<String>,
    pub message_id: String,
    pub kind: LiveEventKind,
    pub text_delta: String,
}

/// Process-wide hub. Cloning shares the channel.
#[derive(Clone)]
pub struct LiveEvents {
    tx: broadcast::Sender<LiveEvent>,
}
impl Default for LiveEvents {
    fn default() -> Self {
        Self {
            tx: broadcast::channel(EVENT_QUEUE).0,
        }
    }
}
impl LiveEvents {
    pub fn subscribe(&self) -> broadcast::Receiver<LiveEvent> {
        self.tx.subscribe()
    }
    pub fn has_subscribers(&self) -> bool {
        self.tx.receiver_count() > 0
    }
    pub fn publish(&self, event: LiveEvent) {
        let _ = self.tx.send(event);
    }
}

/// Incremental decoder state for one response. Dropping it unfinished reports an error.
pub(super) struct LiveStream {
    events: LiveEvents,
    doc: SseDocument,
    pending: Vec<u8>,
    data: Vec<String>,
    sent: HashMap<String, usize>,
    last_id: String,
    seen: usize,
    limit: usize,
    ended: bool,
}

impl LiveStream {
    pub(super) fn new(events: LiveEvents, limit: usize) -> Self {
        Self {
            events,
            doc: SseDocument::default(),
            pending: Vec::new(),
            data: Vec::new(),
            sent: HashMap::new(),
            last_id: String::new(),
            seen: 0,
            limit,
            ended: false,
        }
    }

    pub(super) fn observe(&mut self, bytes: &[u8]) {
        if self.ended {
            return;
        }
        self.seen = self.seen.saturating_add(bytes.len());
        if self.seen > self.limit {
            return self.end(LiveEventKind::Error);
        }
        self.pending.extend_from_slice(bytes);
        while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
            let raw: Vec<u8> = self.pending.drain(..=end).collect();
            if !self.line(&raw[..raw.len() - 1]) {
                return self.end(LiveEventKind::Error);
            }
        }
    }

    pub(super) fn finish(mut self) {
        if self.ended {
            return;
        }
        let tail = std::mem::take(&mut self.pending);
        let ok = (tail.is_empty() || self.line(&tail)) && self.dispatch();
        self.end(if ok {
            LiveEventKind::Done
        } else {
            LiveEventKind::Error
        });
    }

    fn line(&mut self, raw: &[u8]) -> bool {
        let Ok(line) = std::str::from_utf8(raw) else {
            return false;
        };
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            self.dispatch()
        } else {
            if let Some(value) = line.strip_prefix("data:") {
                self.data
                    .push(value.strip_prefix(' ').unwrap_or(value).to_owned());
            }
            true
        }
    }

    fn dispatch(&mut self) -> bool {
        if dispatch_sse_event(&mut self.doc, &mut self.data).is_err() {
            return false;
        }
        self.emit()
    }

    /// Publish growth of the current assistant message. False when bounds are exceeded.
    fn emit(&mut self) -> bool {
        let Some(id) = self.doc.current_node.clone() else {
            return true;
        };
        let Some(text) = self.doc.messages.get(&id).and_then(assistant_text) else {
            return true;
        };
        if !self.sent.contains_key(&id) {
            if self.sent.len() >= MAX_MESSAGES {
                return false;
            }
            self.sent.insert(id.clone(), 0);
            self.send(LiveEventKind::Start, &id, String::new());
        }
        self.last_id = id.clone();
        let sent = self.sent.get(&id).copied().unwrap_or(0);
        // A rewritten (non-append) text cannot be expressed as a delta; the saved
        // conversation reload at the end of the stream corrects the view.
        let Some(new) = text.get(sent..).filter(|new| !new.is_empty()) else {
            return true;
        };
        let new = new.to_owned();
        self.sent.insert(id.clone(), text.len());
        let mut rest = new.as_str();
        while !rest.is_empty() {
            let mut cut = rest.len().min(MAX_DELTA_BYTES);
            while !rest.is_char_boundary(cut) {
                cut -= 1;
            }
            let (chunk, tail) = rest.split_at(cut);
            self.send(LiveEventKind::Delta, &id, chunk.to_owned());
            rest = tail;
        }
        true
    }

    fn send(&self, kind: LiveEventKind, message_id: &str, text_delta: String) {
        self.events.publish(LiveEvent {
            conversation_id: self.doc.conversation_id.clone(),
            message_id: message_id.to_owned(),
            kind,
            text_delta,
        });
    }

    fn end(&mut self, kind: LiveEventKind) {
        self.ended = true;
        self.pending = Vec::new();
        let id = self.last_id.clone();
        self.send(kind, &id, String::new());
    }
}

impl Drop for LiveStream {
    fn drop(&mut self) {
        if !self.ended {
            self.end(LiveEventKind::Error);
        }
    }
}

/// Text of an assistant message with plain text content, or None for anything else.
fn assistant_text(message: &Value) -> Option<String> {
    if message.pointer("/author/role").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let content = message.get("content")?;
    if content.get("content_type").and_then(Value::as_str) != Some("text") {
        return None;
    }
    let parts = content.get("parts")?.as_array()?;
    Some(parts.iter().filter_map(Value::as_str).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture_lane::{CaptureContext, CaptureMetadata};

    const BODY: &str = concat!(
        "data: \"v1\"\n\n",
        "data: {\"type\":\"resume_conversation_token\",\"conversation_id\":\"conv-1\",\"token\":\"secret-token\"}\n\n",
        ": keepalive\n\n",
        "data: {\"p\":\"\",\"o\":\"add\",\"c\":0,\"v\":{\"message\":{\"id\":\"user-node\",\"author\":{\"role\":\"user\"},\"content\":{\"content_type\":\"text\",\"parts\":[\"private question\"]}}}}\n\n",
        "data: {\"p\":\"\",\"o\":\"add\",\"c\":1,\"v\":{\"message\":{\"id\":\"asst-node\",\"author\":{\"role\":\"assistant\"},\"content\":{\"content_type\":\"text\",\"parts\":[\"\"]}}}}\n\n",
        "data: {\"p\":\"/message/content/parts/0\",\"o\":\"append\",\"c\":1,\"v\":\"h\u{e9}llo\"}\n\n",
        "data: {\"v\":\" world\"}\n\n",
        "data: [DONE]\n\n"
    );

    use crate::live_capture::{channel, Accumulator, LiveSink, Work};
    use tokio::sync::mpsc;

    fn sink() -> (LiveSink, mpsc::Receiver<Work>) {
        channel(true)
    }

    fn begin(sink: &LiveSink) -> Option<Accumulator> {
        sink.begin(
            CaptureContext::at_entry("provisional:default", "POST", "/backend-api/f/conversation"),
            CaptureMetadata {
                host: "chatgpt.com".into(),
                status: 200,
                content_type: Some("text/event-stream".into()),
                ..Default::default()
            },
        )
    }

    fn drain(rx: &mut broadcast::Receiver<LiveEvent>) -> Vec<LiveEvent> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    #[test]
    fn assistant_text_streams_as_deltas_across_arbitrary_chunk_boundaries() {
        let (sink, _work) = sink();
        let mut rx = sink.status.events().subscribe();
        let mut accumulator = begin(&sink).unwrap();
        for chunk in BODY.as_bytes().chunks(7) {
            accumulator.observe(chunk);
        }
        accumulator.finish();
        let got = drain(&mut rx);
        let kinds: Vec<_> = got.iter().map(|e| e.kind).collect();
        assert_eq!(kinds[0], LiveEventKind::Start);
        assert_eq!(*kinds.last().unwrap(), LiveEventKind::Done);
        let text: String = got.iter().map(|e| e.text_delta.as_str()).collect();
        assert_eq!(text, "h\u{e9}llo world");
        for event in &got {
            assert_eq!(event.message_id, "asst-node");
            assert_eq!(event.conversation_id.as_deref(), Some("conv-1"));
        }
        assert!(!format!("{got:?}").contains("secret-token"));
        assert!(!format!("{got:?}").contains("private question"));
    }

    #[test]
    fn no_subscriber_means_no_stream_state() {
        let (sink, _work) = sink();
        assert!(begin(&sink).unwrap().live.is_none());
        let _rx = sink.status.events().subscribe();
        assert!(begin(&sink).unwrap().live.is_some());
    }

    #[test]
    fn interrupted_or_malformed_streams_report_an_error() {
        for malformed in [false, true] {
            let (sink, _work) = sink();
            let mut rx = sink.status.events().subscribe();
            let mut accumulator = begin(&sink).unwrap();
            let bytes: &[u8] = if malformed {
                b"data: {not json}\n\n"
            } else {
                b"data: \"v1\"\n\n"
            };
            accumulator.observe(bytes);
            if malformed {
                accumulator.finish();
            } else {
                accumulator.interrupt();
            }
            let got = drain(&mut rx);
            assert_eq!(got.last().unwrap().kind, LiveEventKind::Error);
            assert_eq!(
                got.iter()
                    .filter(|e| e.kind == LiveEventKind::Error)
                    .count(),
                1
            );
        }
    }

    #[test]
    fn oversized_text_chunks_are_bounded_and_split_on_char_boundaries() {
        let events = LiveEvents::default();
        let mut rx = events.subscribe();
        let mut stream = LiveStream::new(events.clone(), 1 << 20);
        let big = "\u{e9}".repeat(MAX_DELTA_BYTES);
        let add = r#"data: {"p":"","o":"add","c":0,"v":{"message":{"id":"a","author":{"role":"assistant"},"content":{"content_type":"text","parts":[""]}}}}"#;
        let append =
            format!(r#"data: {{"p":"/message/content/parts/0","o":"append","v":"{big}"}}"#);
        stream.observe(format!("data: \"v1\"\n\n{add}\n\n{append}\n\n").as_bytes());
        stream.finish();
        let got = drain(&mut rx);
        let deltas: Vec<_> = got
            .iter()
            .filter(|e| e.kind == LiveEventKind::Delta)
            .collect();
        assert!(deltas.len() >= 2);
        assert!(deltas.iter().all(|e| e.text_delta.len() <= MAX_DELTA_BYTES));
        assert_eq!(
            deltas
                .iter()
                .map(|e| e.text_delta.as_str())
                .collect::<String>(),
            big
        );
    }
}
