//! Bounded capture accumulator. Inputs are UTF-8 safely truncated to documented
//! limits: profile scope 256 bytes, method 32 bytes, path 4096 bytes.
use crate::proxy::CaptureEvent;
use std::{
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc;
static NEXT_CORRELATION: AtomicU64 = AtomicU64::new(1);
const PROFILE_MAX: usize = 256;
const METHOD_MAX: usize = 32;
const PATH_MAX: usize = 4096;
const HOST_MAX: usize = 256;
const CONTENT_TYPE_MAX: usize = 256;
const CONTENT_ENCODING_MAX: usize = 64;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureState {
    Complete,
    Interrupted,
    Oversized,
    Dropped,
    UnsupportedEncoding,
}
#[derive(Clone, Debug, Default)]
pub struct CaptureContext {
    pub profile_scope: String,
    pub observed_unix_millis: u128,
    pub correlation: u64,
    pub method: String,
    pub path: String,
}
pub(crate) fn bounded(s: &str, max: usize) -> String {
    let mut end = max.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_owned()
}
impl CaptureContext {
    /// Construct entry context with a fixed profile scope before processing the request.
    pub fn at_entry(
        profile_scope: impl AsRef<str>,
        method: impl AsRef<str>,
        path: impl AsRef<str>,
    ) -> Self {
        Self {
            profile_scope: bounded(profile_scope.as_ref(), PROFILE_MAX),
            observed_unix_millis: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            correlation: NEXT_CORRELATION.fetch_add(1, Ordering::Relaxed),
            method: bounded(method.as_ref(), METHOD_MAX),
            path: bounded(path.as_ref(), PATH_MAX),
        }
    }
    pub fn provisional(method: impl AsRef<str>, path: impl AsRef<str>) -> Self {
        Self::at_entry("provisional:unknown", method, path)
    }
}
#[derive(Clone, Debug, Default)]
pub struct CaptureMetadata {
    pub host: String,
    pub status: u16,
    pub content_type: Option<String>,
    pub content_encoding: Option<String>,
}
#[derive(Clone, Debug)]
pub struct CaptureObservation {
    pub context: CaptureContext,
    pub metadata: CaptureMetadata,
    pub state: CaptureState,
}
#[derive(Clone, Debug)]
pub struct CaptureLimits {
    pub per_capture_bytes: usize,
    pub retained_bytes: usize,
    pub queue: usize,
}
impl Default for CaptureLimits {
    fn default() -> Self {
        Self {
            per_capture_bytes: 2 * 1024 * 1024,
            retained_bytes: 16 * 1024 * 1024,
            queue: 32,
        }
    }
}
#[derive(Default)]
pub struct CaptureCounters {
    pub lost_observations: AtomicUsize,
    pub complete: AtomicUsize,
    pub interrupted: AtomicUsize,
    pub oversized: AtomicUsize,
    pub dropped: AtomicUsize,
    pub unsupported_encoding: AtomicUsize,
}
struct Budget {
    used: AtomicUsize,
    limit: usize,
}
struct Permit {
    budget: Arc<Budget>,
    bytes: usize,
}
impl Drop for Permit {
    fn drop(&mut self) {
        self.budget.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
struct Work {
    context: CaptureContext,
    metadata: CaptureMetadata,
    body: Vec<u8>,
    _permit: Permit,
}
#[derive(Clone)]
pub struct CaptureLane {
    tx: mpsc::Sender<Work>,
    budget: Arc<Budget>,
    per_capture: usize,
    observations: mpsc::Sender<CaptureObservation>,
    counters: Arc<CaptureCounters>,
}
impl CaptureLane {
    /// Limits must be nonzero; capacities are bounded and checked for arithmetic overflow.
    pub fn new(
        legacy: mpsc::Sender<CaptureEvent>,
        observations: mpsc::Sender<CaptureObservation>,
        limits: CaptureLimits,
    ) -> Self {
        assert!(
            limits.per_capture_bytes > 0 && limits.retained_bytes > 0 && limits.queue > 0,
            "capture limits must be nonzero"
        );
        assert!(
            limits.per_capture_bytes.checked_mul(limits.queue).is_some(),
            "capture capacity overflow"
        );
        let (tx, mut rx) = mpsc::channel::<Work>(limits.queue);
        let counters = Arc::new(CaptureCounters::default());
        let wc = counters.clone();
        let wo = observations.clone();
        tokio::spawn(async move {
            while let Some(work) = rx.recv().await {
                let Work {
                    context,
                    metadata,
                    body,
                    _permit,
                } = work;
                let state = if metadata
                    .content_encoding
                    .as_deref()
                    .is_some_and(|s| !s.eq_ignore_ascii_case("identity"))
                {
                    // Release body allocation before its retained-byte permit.
                    drop(body);
                    CaptureState::UnsupportedEncoding
                } else {
                    let event = CaptureEvent {
                        host: metadata.host.clone(),
                        path: context.path.clone(),
                        status: metadata.status,
                        body,
                    };
                    if legacy.try_send(event).is_err() {
                        CaptureState::Dropped
                    } else {
                        CaptureState::Complete
                    }
                };
                bump(&wc, state);
                observe(&wo, &wc, context, metadata, state);
                drop(_permit);
            }
        });
        Self {
            tx,
            budget: Arc::new(Budget {
                used: AtomicUsize::new(0),
                limit: limits.retained_bytes,
            }),
            per_capture: limits.per_capture_bytes,
            observations,
            counters,
        }
    }
    pub fn begin(&self, mut context: CaptureContext, mut metadata: CaptureMetadata) -> Accumulator {
        // Fields are public for compatibility, so normalize even contexts constructed directly.
        context.profile_scope = bounded(&context.profile_scope, PROFILE_MAX);
        context.method = bounded(&context.method, METHOD_MAX);
        context.path = bounded(&context.path, PATH_MAX);
        metadata.host = bounded(&metadata.host, HOST_MAX);
        metadata.content_type = metadata.content_type.map(|s| bounded(&s, CONTENT_TYPE_MAX));
        metadata.content_encoding = metadata
            .content_encoding
            .map(|s| bounded(&s, CONTENT_ENCODING_MAX));
        let acquired = self.reserve(self.per_capture);
        let body = if acquired {
            Vec::with_capacity(self.per_capture)
        } else {
            Vec::new()
        };
        let mut a = Accumulator {
            lane: self.clone(),
            context,
            metadata,
            body,
            reserved: if acquired { self.per_capture } else { 0 },
            terminal: None,
        };
        if !acquired {
            a.fail(CaptureState::Dropped);
        }
        a
    }
    pub fn retained_bytes(&self) -> usize {
        self.budget.used.load(Ordering::Acquire)
    }
    pub fn counters(&self) -> &Arc<CaptureCounters> {
        &self.counters
    }
    fn reserve(&self, bytes: usize) -> bool {
        let mut used = self.budget.used.load(Ordering::Acquire);
        loop {
            let Some(next) = used.checked_add(bytes) else {
                return false;
            };
            if next > self.budget.limit {
                return false;
            }
            match self.budget.used.compare_exchange_weak(
                used,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(v) => used = v,
            }
        }
    }
    fn observation(&self, context: CaptureContext, metadata: CaptureMetadata, state: CaptureState) {
        observe(&self.observations, &self.counters, context, metadata, state)
    }
}
fn bump(c: &CaptureCounters, s: CaptureState) {
    let a = match s {
        CaptureState::Complete => &c.complete,
        CaptureState::Interrupted => &c.interrupted,
        CaptureState::Oversized => &c.oversized,
        CaptureState::Dropped => &c.dropped,
        CaptureState::UnsupportedEncoding => &c.unsupported_encoding,
    };
    a.fetch_add(1, Ordering::Relaxed);
}
fn observe(
    tx: &mpsc::Sender<CaptureObservation>,
    c: &CaptureCounters,
    context: CaptureContext,
    metadata: CaptureMetadata,
    state: CaptureState,
) {
    if tx
        .try_send(CaptureObservation {
            context,
            metadata,
            state,
        })
        .is_err()
    {
        c.lost_observations.fetch_add(1, Ordering::Relaxed);
    }
}
pub struct Accumulator {
    lane: CaptureLane,
    context: CaptureContext,
    metadata: CaptureMetadata,
    body: Vec<u8>,
    reserved: usize,
    terminal: Option<CaptureState>,
}
impl Accumulator {
    pub fn observe(&mut self, bytes: &[u8]) {
        if self.terminal.is_some() {
            return;
        }
        if self
            .body
            .len()
            .checked_add(bytes.len())
            .is_none_or(|n| n > self.lane.per_capture)
        {
            self.fail(CaptureState::Oversized);
            return;
        }
        self.body.extend_from_slice(bytes);
    }
    fn release(&mut self) {
        let body = std::mem::take(&mut self.body);
        drop(body);
        if self.reserved > 0 {
            self.lane
                .budget
                .used
                .fetch_sub(self.reserved, Ordering::AcqRel);
            self.reserved = 0;
        }
    }
    fn fail(&mut self, state: CaptureState) {
        if self.terminal.is_some() {
            return;
        }
        self.terminal = Some(state);
        self.release();
        bump(&self.lane.counters, state);
        self.lane
            .observation(self.context.clone(), self.metadata.clone(), state);
    }
    pub fn finish(mut self) {
        if self.terminal.is_some() {
            return;
        }
        self.terminal = Some(CaptureState::Complete);
        let permit = Permit {
            budget: self.lane.budget.clone(),
            bytes: self.reserved,
        };
        self.reserved = 0;
        let work = Work {
            context: self.context.clone(),
            metadata: self.metadata.clone(),
            body: std::mem::take(&mut self.body),
            _permit: permit,
        };
        if let Err(e) = self.lane.tx.try_send(work) {
            drop(e);
            self.terminal = Some(CaptureState::Dropped);
            bump(&self.lane.counters, CaptureState::Dropped);
            self.lane.observation(
                self.context.clone(),
                self.metadata.clone(),
                CaptureState::Dropped,
            );
        }
    }
    pub fn interrupt(mut self) {
        self.fail(CaptureState::Interrupted)
    }
}
impl Drop for Accumulator {
    fn drop(&mut self) {
        if self.terminal.is_none() {
            self.fail(CaptureState::Interrupted)
        } else if self.reserved > 0 {
            self.release()
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn setup(
        l: CaptureLimits,
    ) -> (
        CaptureLane,
        mpsc::Receiver<CaptureObservation>,
        mpsc::Receiver<CaptureEvent>,
    ) {
        let (lt, lr) = mpsc::channel(8);
        let (ot, or) = mpsc::channel(8);
        (CaptureLane::new(lt, ot, l), or, lr)
    }
    fn ctx() -> CaptureContext {
        CaptureContext::provisional("GET", "/")
    }
    fn meta() -> CaptureMetadata {
        CaptureMetadata::default()
    }
    #[tokio::test]
    async fn context_is_initialized_and_bounded() {
        let c = CaptureContext::at_entry("team", "GET", "/");
        assert_eq!(c.profile_scope, "team");
        assert!(c.observed_unix_millis > 0);
        assert_ne!(c.correlation, 0);
        let b = CaptureContext::provisional("é".repeat(100), "x".repeat(5000));
        assert!(b.method.len() <= METHOD_MAX && b.path.len() <= PATH_MAX);
    }
    #[tokio::test]
    async fn reservation_cap_cancel_and_oversize() {
        let (lane, mut obs, _) = setup(CaptureLimits {
            per_capture_bytes: 2,
            retained_bytes: 4,
            queue: 2,
        });
        let mut a = lane.begin(ctx(), meta());
        assert_eq!(lane.retained_bytes(), 2);
        a.observe(b"abc");
        assert_eq!(lane.retained_bytes(), 0);
        assert_eq!(obs.try_recv().unwrap().state, CaptureState::Oversized);
        let b = lane.begin(ctx(), meta());
        assert_eq!(lane.retained_bytes(), 2);
        drop(b);
        assert_eq!(lane.retained_bytes(), 0);
        assert_eq!(obs.try_recv().unwrap().state, CaptureState::Interrupted);
    }
    #[tokio::test]
    async fn global_exhaustion_and_full_legacy_release() {
        let (lt, mut lr) = mpsc::channel(1);
        lt.try_send(CaptureEvent {
            host: String::new(),
            path: String::new(),
            status: 0,
            body: vec![],
        })
        .unwrap();
        let (ot, mut obs) = mpsc::channel(8);
        let lane = CaptureLane::new(
            lt,
            ot,
            CaptureLimits {
                per_capture_bytes: 2,
                retained_bytes: 2,
                queue: 1,
            },
        );
        let a = lane.begin(ctx(), meta());
        let b = lane.begin(ctx(), meta());
        assert_eq!(lane.retained_bytes(), 2);
        assert_eq!(obs.try_recv().unwrap().state, CaptureState::Dropped);
        drop(a);
        assert_eq!(lane.retained_bytes(), 0);
        b.finish();
        tokio::task::yield_now().await;
        assert_eq!(lane.retained_bytes(), 0);
        assert_eq!(obs.try_recv().unwrap().state, CaptureState::Interrupted);
        lr.close();
    }
    #[tokio::test]
    async fn unsupported_encoding_and_internal_queue_full() {
        let (lt, mut legacy) = mpsc::channel(1);
        let (ot, mut obs) = mpsc::channel(8);
        let lane = CaptureLane::new(
            lt,
            ot,
            CaptureLimits {
                per_capture_bytes: 1,
                retained_bytes: 1,
                queue: 1,
            },
        );
        let mut a = lane.begin(ctx(), meta());
        a.observe(b"x");
        a.finish();
        tokio::task::yield_now().await;
        assert_eq!(obs.try_recv().unwrap().state, CaptureState::Complete);
        let _ = legacy.try_recv();
        let m = CaptureMetadata {
            content_encoding: Some("gzip".into()),
            ..meta()
        };
        lane.begin(ctx(), m).finish();
        tokio::task::yield_now().await;
        assert_eq!(
            obs.try_recv().unwrap().state,
            CaptureState::UnsupportedEncoding
        );
    }
    #[tokio::test]
    async fn queue_saturation_lost_observations_and_empty_complete() {
        let (lt, mut legacy) = mpsc::channel(8);
        let (ot, mut obs) = mpsc::channel(1);
        let lane = CaptureLane::new(
            lt,
            ot,
            CaptureLimits {
                per_capture_bytes: 1,
                retained_bytes: 1,
                queue: 1,
            },
        );
        let mut first = lane.begin(ctx(), meta());
        first.observe(b"x");
        first.finish();
        let mut second = lane.begin(ctx(), meta());
        second.observe(b"y");
        second.finish();
        assert_eq!(lane.counters().dropped.load(Ordering::Relaxed), 1);
        assert_eq!(lane.retained_bytes(), 1);
        tokio::task::yield_now().await;
        assert_eq!(lane.counters().lost_observations.load(Ordering::Relaxed), 1);
        assert_eq!(lane.retained_bytes(), 0);
        assert_eq!(obs.try_recv().unwrap().state, CaptureState::Dropped);
        assert_eq!(legacy.try_recv().unwrap().body, b"x");
        let empty = lane.begin(ctx(), meta());
        empty.finish();
        tokio::task::yield_now().await;
        assert_eq!(obs.try_recv().unwrap().state, CaptureState::Complete);
        assert_eq!(legacy.try_recv().unwrap().body, b"");
    }
    #[tokio::test]
    async fn global_below_per_capture_drops_without_reserving_and_metadata_is_bounded() {
        let (lt, _) = mpsc::channel(8);
        let (ot, mut obs) = mpsc::channel(8);
        let lane = CaptureLane::new(
            lt,
            ot,
            CaptureLimits {
                per_capture_bytes: 4,
                retained_bytes: 3,
                queue: 1,
            },
        );
        let context = CaptureContext {
            profile_scope: "p".repeat(300),
            method: "M".repeat(40),
            path: "x".repeat(5000),
            ..ctx()
        };
        let metadata = CaptureMetadata {
            host: "h".repeat(300),
            content_type: Some("t".repeat(300)),
            content_encoding: Some("e".repeat(80)),
            ..meta()
        };
        let a = lane.begin(context, metadata);
        assert_eq!(lane.retained_bytes(), 0);
        assert_eq!(a.body.capacity(), 0);
        assert_eq!(a.context.path.len(), PATH_MAX);
        assert_eq!(a.metadata.host.len(), HOST_MAX);
        assert_eq!(
            a.metadata.content_type.as_ref().unwrap().len(),
            CONTENT_TYPE_MAX
        );
        assert_eq!(
            a.metadata.content_encoding.as_ref().unwrap().len(),
            CONTENT_ENCODING_MAX
        );
        assert_eq!(obs.try_recv().unwrap().state, CaptureState::Dropped);
    }
    #[tokio::test]
    async fn limits_must_be_nonzero() {
        let (lt, _) = mpsc::channel(1);
        let (ot, _) = mpsc::channel(1);
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| CaptureLane::new(
                lt,
                ot,
                CaptureLimits {
                    per_capture_bytes: 0,
                    retained_bytes: 1,
                    queue: 1
                }
            )))
            .is_err()
        );
    }
}
