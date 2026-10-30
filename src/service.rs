//! Shared service lifetime and bounded capture ingestion.
use crate::{
    config::Config,
    proxy::{self, CaptureEvent, ProxyConfig},
    server,
    store::Store,
};
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread::{Builder, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle as TokioJoinHandle,
};
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServiceState {
    Starting,
    Ready,
    Stopping,
    Stopped,
    Failed(String),
}
#[derive(Clone, Debug, Default)]
pub struct IngestStats {
    // Legacy CaptureEvent ingress only. Typed live attempts have their own
    // conservation snapshot, including pending commits after shutdown timeout.
    /// Events dequeued by the ingestion writer (not merely accepted by the channel).
    pub accepted: u64,
    pub stored: u64,
    pub ignored: u64,
    pub failed: u64,
    pub unprocessed: u64,
}
#[derive(Clone)]
pub struct ServiceStatus {
    pub state: ServiceState,
    pub identity: server::RuntimeIdentity,
    pub api_addr: Option<SocketAddr>,
    pub proxy_addr: Option<SocketAddr>,
    pub database_healthy: Option<bool>,
    pub ingest: IngestStats,
}

fn readiness_provider(status: Arc<Mutex<ServiceStatus>>) -> server::ReadinessProvider {
    Arc::new(move || {
        let state = match status.try_lock() {
            Ok(snapshot) => snapshot.state.clone(),
            Err(_) => {
                return server::ServiceReadiness {
                    ready: false,
                    code: Some("status_unavailable"),
                }
            }
        };
        server::ServiceReadiness {
            ready: state == ServiceState::Ready,
            code: match state {
                ServiceState::Ready => None,
                ServiceState::Starting => Some("service_starting"),
                ServiceState::Stopping => Some("service_stopping"),
                ServiceState::Stopped => Some("service_stopped"),
                ServiceState::Failed(_) => Some("service_failed"),
            },
        }
    })
}
#[cfg(test)]
mod api_readiness_tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    async fn status(app: &axum::Router) -> (StatusCode, serde_json::Value) {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/status")
                    .header("authorization", "Bearer secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let code = response.status();
        let body = to_bytes(response.into_body(), 8192).await.unwrap();
        (code, serde_json::from_slice(&body).unwrap())
    }

    #[tokio::test]
    async fn actual_lifecycle_and_real_store_probe_are_independent() {
        let lifecycle = Arc::new(Mutex::new(ServiceStatus {
            state: ServiceState::Ready,
            identity: server::RuntimeIdentity {
                pid: 1,
                instance_id: "synthetic-service".into(),
            },
            api_addr: None,
            proxy_addr: None,
            database_healthy: Some(true),
            ingest: IngestStats::default(),
        }));
        let store = Arc::new(Mutex::new(Store::in_memory().unwrap()));
        let app = server::status_test_router(store.clone(), readiness_provider(lifecycle.clone()));
        let (code, body) = status(&app).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(body["service"]["status"], "ready");
        assert_eq!(body["database"]["status"], "healthy");

        lifecycle.lock().unwrap().state = ServiceState::Failed("synthetic worker detail".into());
        let (code, body) = status(&app).await;
        assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["service"]["code"], "service_failed");
        assert_eq!(body["database"]["status"], "healthy");
        assert!(!body.to_string().contains("synthetic worker detail"));

        lifecycle.lock().unwrap().state = ServiceState::Ready;
        // Actual loss of access to the store, rather than a canned DB fixture.
        let poisoned = store.clone();
        assert!(std::thread::spawn(move || {
            let _guard = poisoned.lock().unwrap();
            panic!("synthetic store lock failure");
        })
        .join()
        .is_err());
        let (code, body) = status(&app).await;
        assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["service"]["status"], "ready");
        assert_eq!(body["database"]["status"], "failed");
        assert_eq!(body["database"]["code"], "database_unavailable");
    }

    #[test]
    fn readiness_provider_never_waits_for_contended_status_lock() {
        let lifecycle = Arc::new(Mutex::new(ServiceStatus {
            state: ServiceState::Ready,
            identity: server::RuntimeIdentity {
                pid: 1,
                instance_id: "synthetic".into(),
            },
            api_addr: None,
            proxy_addr: None,
            database_healthy: None,
            ingest: IngestStats::default(),
        }));
        let provider = readiness_provider(lifecycle.clone());
        let _held = lifecycle.lock().unwrap();
        let result = provider();
        assert!(!result.ready);
        assert_eq!(result.code, Some("status_unavailable"));
    }
}

pub struct ServiceHandle {
    #[cfg(test)]
    live_sink: crate::live_capture::LiveSink,
    capture: crate::live_capture::CaptureStatus,
    live_receiver: Arc<Mutex<mpsc::Receiver<crate::live_capture::Work>>>,
    status: Arc<Mutex<ServiceStatus>>,
    shutdown: watch::Sender<bool>,
    tasks: Vec<TokioJoinHandle<()>>,
    listener_abort_handles: Vec<tokio::task::AbortHandle>,
    writer: Option<JoinHandle<()>>,
    writer_stop: Arc<AtomicBool>,
    abandon_queue: Arc<AtomicBool>,
    abandoned: Arc<AtomicU64>,
    receiver: Arc<Mutex<mpsc::Receiver<CaptureEvent>>>,
    interrupt: Arc<rusqlite::InterruptHandle>,
    deadline: Duration,
}
#[derive(Clone)]
struct WriterFailureContext {
    capture: crate::live_capture::CaptureStatus,
    live_receiver: Arc<Mutex<mpsc::Receiver<crate::live_capture::Work>>>,
    stop: watch::Sender<bool>,
    status: Arc<Mutex<ServiceStatus>>,
    writer_stop: Arc<AtomicBool>,
    abandon_queue: Arc<AtomicBool>,
    abandoned: Arc<AtomicU64>,
    receiver: Arc<Mutex<mpsc::Receiver<CaptureEvent>>>,
    abort_registry: Arc<Mutex<Vec<tokio::task::AbortHandle>>>,
}

impl WriterFailureContext {
    fn actual_cleanup(&self) {
        self.capture.stop();
        {
            let mut rx = self.live_receiver.lock().unwrap_or_else(|p| p.into_inner());
            rx.close();
            while rx.try_recv().is_ok() {}
        }
        self.writer_stop.store(true, Ordering::Release);
        self.abandon_queue.store(true, Ordering::Release);
        {
            let mut rx = self.receiver.lock().unwrap_or_else(|p| p.into_inner());
            rx.close();
            let queued = rx.len() as u64;
            while rx.try_recv().is_ok() {}
            self.abandoned.fetch_add(queued, Ordering::AcqRel);
        }
        let mut status = lock_status(&self.status);
        status.database_healthy = Some(false);
        if !matches!(status.state, ServiceState::Failed(_)) {
            status.state = ServiceState::Failed("capture writer panicked".into());
        }
        drop(status);
        let _ = self.stop.send(true);
        for abort in self
            .abort_registry
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
        {
            abort.abort();
        }
    }
}

fn supervise_writer<F>(operation: F, failure: WriterFailureContext)
where
    F: FnOnce(),
{
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)).is_err() {
        failure.actual_cleanup();
    }
}

fn lock_status(status: &Mutex<ServiceStatus>) -> std::sync::MutexGuard<'_, ServiceStatus> {
    match status.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            let mut guard = poisoned.into_inner();
            guard.state = ServiceState::Failed("service status lock poisoned".into());
            guard
        }
    }
}
impl ServiceHandle {
    pub fn capture_status(&self) -> crate::live_capture::CaptureStatus {
        self.capture.clone()
    }
    pub fn status(&self) -> ServiceStatus {
        lock_status(&self.status).clone()
    }
    pub async fn shutdown(self) -> IngestStats {
        let d = self.deadline;
        self.shutdown_timeout(d).await
    }
    pub async fn shutdown_timeout(mut self, budget: Duration) -> IngestStats {
        self.capture.stop();
        {
            let mut state = lock_status(&self.status);
            if !matches!(state.state, ServiceState::Failed(_)) {
                state.state = ServiceState::Stopping;
            }
        }
        let _ = self.shutdown.send(true);
        let end = Instant::now() + budget;
        // The proxy monitor awaits the original accept task, which does not
        // observe the service stop watch. Abort that listener before joining monitors.
        for handle in self.listener_abort_handles.drain(..) {
            handle.abort();
        }
        self.writer_stop.store(true, Ordering::Release);
        // Give the writer the remaining budget to drain; the sole writer closes its receiver on stop.
        for mut t in self.tasks.drain(..) {
            let remain = end.saturating_duration_since(Instant::now());
            if tokio::time::timeout(remain, &mut t).await.is_err() {
                t.abort();
            }
        }
        if let Some(w) = self.writer.take() {
            while !w.is_finished() && Instant::now() < end {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            if w.is_finished() {
                let _ = w.join();
            } else {
                self.abandon_queue.store(true, Ordering::Release);
                self.interrupt.interrupt();
                {
                    let mut rx = self.live_receiver.lock().unwrap_or_else(|p| p.into_inner());
                    rx.close();
                    while rx.try_recv().is_ok() {}
                }
                let queued = {
                    let mut rx = self.receiver.lock().unwrap_or_else(|p| p.into_inner());
                    rx.close();
                    let n = rx.len() as u64;
                    while rx.try_recv().is_ok() {}
                    n
                };
                self.abandoned.fetch_add(queued, Ordering::AcqRel);
                let mut s = lock_status(&self.status);
                s.state = ServiceState::Failed(
                    "shutdown deadline exceeded; SQLite interrupted; writer still finishing".into(),
                );
                s.ingest.unprocessed = self.abandoned.load(Ordering::Acquire).saturating_add(
                    s.ingest
                        .accepted
                        .saturating_sub(s.ingest.stored + s.ingest.ignored + s.ingest.failed),
                );
                return s.ingest.clone();
            }
        }
        let mut s = lock_status(&self.status);
        if !matches!(s.state, ServiceState::Failed(_)) {
            s.state = ServiceState::Stopped;
        }
        s.ingest.unprocessed = self.abandoned.load(Ordering::Acquire).saturating_add(
            s.ingest
                .accepted
                .saturating_sub(s.ingest.stored + s.ingest.ignored + s.ingest.failed),
        );
        s.ingest.clone()
    }
}
impl Drop for ServiceHandle {
    fn drop(&mut self) {
        self.capture.stop();
        {
            let mut rx = self.live_receiver.lock().unwrap_or_else(|p| p.into_inner());
            rx.close();
            while rx.try_recv().is_ok() {}
        }
        let _ = self.shutdown.send(true);
        self.writer_stop.store(true, Ordering::Release);
        self.abandon_queue.store(true, Ordering::Release);
        self.interrupt.interrupt();
        for t in self.tasks.drain(..) {
            t.abort();
        }
        for handle in self.listener_abort_handles.drain(..) {
            handle.abort();
        }
    }
}
pub async fn start(
    config: Config,
    capacity: usize,
) -> Result<(ServiceHandle, mpsc::Sender<CaptureEvent>), String> {
    start_with_proxy(config, capacity, ProxyConfig::loopback("")).await
}
pub async fn start_with_proxy(
    config: Config,
    capacity: usize,
    proxy_config: ProxyConfig,
) -> Result<(ServiceHandle, mpsc::Sender<CaptureEvent>), String> {
    start_internal(
        config,
        capacity,
        proxy_config,
        Duration::from_millis(100),
        #[cfg(test)]
        None,
        #[cfg(test)]
        None,
        #[cfg(test)]
        false,
    )
    .await
}

async fn start_internal(
    config: Config,
    capacity: usize,
    mut proxy_config: ProxyConfig,
    writer_busy_timeout: Duration,
    #[cfg(test)] writer_gate: Option<WriterTestGate>,
    #[cfg(test)] writer_exit: Option<std::sync::mpsc::Sender<()>>,
    #[cfg(test)] writer_panics: bool,
) -> Result<(ServiceHandle, mpsc::Sender<CaptureEvent>), String> {
    if capacity == 0 {
        return Err("capture capacity must be positive".into());
    }
    if !config.bind_addr.ip().is_loopback() || !config.proxy_addr.ip().is_loopback() {
        return Err("API and proxy addresses must use loopback addresses".into());
    }
    if config.bearer_token.is_empty() {
        return Err("bearer token must not be empty".into());
    }
    let path = config.database_path.clone();
    if !path.is_empty() && path != ":memory:" {
        let db_path = std::path::Path::new(&path);
        if let Some(parent) = db_path.parent().filter(|p| !p.as_os_str().is_empty()) {
            let parent = parent.to_path_buf();
            tokio::task::spawn_blocking(move || std::fs::create_dir_all(parent))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
        }
    }
    let api_store = tokio::task::spawn_blocking(move || {
        Store::open(&path).and_then(|s| {
            s.health()?;
            Ok(s)
        })
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    let listener = tokio::net::TcpListener::bind(config.bind_addr)
        .await
        .map_err(|e| e.to_string())?;
    let api_addr = listener.local_addr().map_err(|e| e.to_string())?;
    let (tx, rx) = mpsc::channel(capacity);
    let (live_sink, live_rx) = crate::live_capture::channel(config.capture_enabled);
    let capture = live_sink.status.clone();
    let live_receiver = Arc::new(Mutex::new(live_rx));
    let writer_live = live_receiver.clone();
    let capture_enabled = config.capture_enabled;
    let receiver = Arc::new(Mutex::new(rx));
    let writer_receiver = receiver.clone();
    static INSTANCE_COUNTER: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = INSTANCE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let identity = server::RuntimeIdentity {
        pid: std::process::id(),
        instance_id: format!("{:x}-{:x}-{:x}", now, counter, std::process::id()),
    };
    let status = Arc::new(Mutex::new(ServiceStatus {
        state: ServiceState::Starting,
        identity: identity.clone(),
        api_addr: Some(api_addr),
        proxy_addr: None,
        database_healthy: Some(true),
        ingest: IngestStats::default(),
    }));
    let (stop, _) = watch::channel(false);
    let abort_registry: Arc<Mutex<Vec<tokio::task::AbortHandle>>> =
        Arc::new(Mutex::new(Vec::new()));
    let (writer_ready_tx, writer_ready_rx) = oneshot::channel();
    let writer_status = status.clone();
    let flag = Arc::new(AtomicBool::new(false));
    let wf = flag.clone();
    let abandon_queue = Arc::new(AtomicBool::new(false));
    let wa = abandon_queue.clone();
    let abandoned = Arc::new(AtomicU64::new(0));
    let writer_abandoned = abandoned.clone();
    let writer_failure = WriterFailureContext {
        capture: capture.clone(),
        live_receiver: live_receiver.clone(),
        stop: stop.clone(),
        status: status.clone(),
        writer_stop: flag.clone(),
        abandon_queue: abandon_queue.clone(),
        abandoned: abandoned.clone(),
        receiver: writer_receiver.clone(),
        abort_registry: abort_registry.clone(),
    };
    let db = config.database_path.clone();
    let writer = Builder::new()
        .name("capture-ingestion".into())
        .spawn(move || {
            #[cfg(test)]
            let _writer_exit = writer_exit.map(WriterExitSignal);
            supervise_writer(
                || {
                    let mut store = match Store::open(&db) {
                        Ok(s) => s,
                        Err(e) => {
                            let _ = writer_ready_tx.send(Err(e.to_string()));
                            return;
                        }
                    };
                    let _ = store.connection().busy_timeout(writer_busy_timeout);
                    let interrupt = Arc::new(store.connection().get_interrupt_handle());
                    let _ = writer_ready_tx.send(Ok(interrupt));
                    loop {
                        if wf.load(Ordering::Acquire) {
                            writer_live
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .close();
                            writer_receiver
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .close()
                        }
                        if wa.load(Ordering::Acquire) {
                            let mut live = writer_live.lock().unwrap_or_else(|p| p.into_inner());
                            live.close();
                            while live.try_recv().is_ok() {}
                            let mut rx = writer_receiver.lock().unwrap_or_else(|p| p.into_inner());
                            rx.close();
                            let queued = rx.len() as u64;
                            while rx.try_recv().is_ok() {}
                            writer_abandoned.fetch_add(queued, Ordering::AcqRel);
                            break;
                        }
                        // Holding the receiver lock through accepted accounting makes dequeue
                        // and deadline abandonment mutually exclusive, with no accounting gap.
                        let live = writer_live
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .try_recv();
                        if let Ok(work) = live {
                            #[cfg(not(test))]
                            work.ingest(&mut store);
                            #[cfg(test)]
                            work.ingest_after(&mut store, || {
                                if let Some(gate) = &writer_gate {
                                    let (lock, cv) = &**gate;
                                    let mut released = lock.lock().unwrap();
                                    while !*released {
                                        released = cv.wait(released).unwrap();
                                    }
                                }
                                assert!(!writer_panics, "synthetic live writer panic");
                            });
                            continue;
                        }
                        let received = {
                            let mut rx = writer_receiver.lock().unwrap_or_else(|p| p.into_inner());
                            let received = rx.try_recv();
                            if received.is_ok() {
                                lock_status(&writer_status).ingest.accepted += 1;
                            }
                            received
                        };
                        match received {
                            Ok(ev) => {
                                #[cfg(test)]
                                if let Some(gate) = &writer_gate {
                                    let (lock, cv) = &**gate;
                                    let mut released = lock.lock().unwrap();
                                    while !*released {
                                        released = cv.wait(released).unwrap();
                                    }
                                }
                                // Legacy ingress remains item-count bounded by capacity; its
                                // bodies are outside the typed live 16 MiB retained budget.
                                // Disabled capture never archives direct legacy submissions.
                                let out = if !capture_enabled
                                    || ev.body.len() > crate::live_capture::ITEM_BYTES
                                {
                                    Ok(false)
                                } else {
                                    store.ingest_capture(ev)
                                };
                                let mut s = lock_status(&writer_status);
                                match out {
                                    Ok(true) => s.ingest.stored += 1,
                                    Ok(false) => s.ingest.ignored += 1,
                                    Err(_) => {
                                        s.ingest.failed += 1;
                                        s.database_healthy = Some(false);
                                    }
                                }
                            }
                            Err(mpsc::error::TryRecvError::Disconnected) => {
                                if wf.load(Ordering::Acquire)
                                    || writer_live
                                        .lock()
                                        .unwrap_or_else(|p| p.into_inner())
                                        .is_closed()
                                {
                                    break;
                                }
                                std::thread::sleep(Duration::from_millis(2));
                            }
                            Err(mpsc::error::TryRecvError::Empty) => {
                                std::thread::sleep(Duration::from_millis(2))
                            }
                        }
                    }
                },
                writer_failure,
            )
        })
        .map_err(|e| e.to_string())?;
    let interrupt = match writer_ready_rx.await.map_err(|e| e.to_string())? {
        Ok(handle) => handle,
        Err(e) => {
            let _ = writer.join();
            return Err(e);
        }
    };
    proxy_config.ca_dir = config.ca_dir.clone();
    proxy_config.bind = config.proxy_addr;
    #[cfg(test)]
    let test_live_sink = live_sink.clone();
    let (proxy, pt) = match proxy::run_live(proxy_config, live_sink).await {
        Ok(x) => x,
        Err(e) => {
            flag.store(true, Ordering::Release);
            let _ = writer.join();
            return Err(e.to_string());
        }
    };
    let mut stop_rx = stop.subscribe();
    let api_status = status.clone();
    let api_stop = stop.clone();
    let api_flag = flag.clone();
    let api_readiness = readiness_provider(status.clone());
    let api_capture = capture.clone();
    let api = tokio::spawn(async move {
        tokio::select! {_=axum::serve(listener,server::router_with_capture_status(api_store,config.bearer_token,identity,api_readiness,Arc::new(move || api_capture.snapshot())))=>{},_=stop_rx.changed()=>{}}
        if !*api_stop.borrow() {
            api_flag.store(true, Ordering::Release);
            let mut state = lock_status(&api_status);
            if !matches!(state.state, ServiceState::Stopping | ServiceState::Stopped) {
                state.state = ServiceState::Failed("API listener exited unexpectedly".into());
            }
            drop(state);
            let _ = api_stop.send(true);
        }
    });
    let proxy_status = status.clone();
    let proxy_stop = stop.clone();
    let proxy_flag = flag.clone();
    let proxy_abort = pt.abort_handle();
    let monitor = tokio::spawn(async move {
        let mut pt = pt;
        let _ = (&mut pt).await;
        if !*proxy_stop.borrow() {
            let mut state = lock_status(&proxy_status);
            if !matches!(state.state, ServiceState::Stopping | ServiceState::Stopped) {
                state.state = ServiceState::Failed("proxy listener exited unexpectedly".into());
            }
            drop(state);
            proxy_flag.store(true, Ordering::Release);
            let _ = proxy_stop.send(true);
        }
    });
    let api_abort = api.abort_handle();
    abort_registry
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .extend([api_abort, proxy_abort.clone()]);
    if matches!(lock_status(&status).state, ServiceState::Failed(_)) {
        for abort in abort_registry
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
        {
            abort.abort();
        }
    }
    {
        let mut s = lock_status(&status);
        s.proxy_addr = Some(proxy.addr);
        if s.state == ServiceState::Starting {
            s.state = ServiceState::Ready;
        }
    }
    Ok((
        ServiceHandle {
            #[cfg(test)]
            live_sink: test_live_sink,
            capture,
            live_receiver,
            status,
            shutdown: stop,
            tasks: vec![api, monitor],
            listener_abort_handles: vec![proxy_abort],
            writer: Some(writer),
            writer_stop: flag,
            abandon_queue,
            abandoned,
            receiver,
            interrupt,
            deadline: Duration::from_secs(5),
        },
        tx,
    ))
}
pub fn try_submit(
    sender: &mpsc::Sender<CaptureEvent>,
    event: CaptureEvent,
) -> Result<(), mpsc::error::TrySendError<CaptureEvent>> {
    sender.try_send(event)
}

#[cfg(test)]
type WriterTestGate = Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>;
#[cfg(test)]
struct WriterExitSignal(std::sync::mpsc::Sender<()>);
#[cfg(test)]
impl Drop for WriterExitSignal {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

#[cfg(test)]
mod deadline_tests {
    use super::*;

    #[tokio::test]
    async fn actual_live_writer_panic_drains_queue_without_handle_drop() {
        let root = std::env::temp_dir().join(format!(
            "live-panic-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let config = Config {
            capture_enabled: true,
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            proxy_addr: "127.0.0.1:0".parse().unwrap(),
            bearer_token: "synthetic".into(),
            database_path: root.join("db.sqlite").to_string_lossy().into_owned(),
            ca_dir: root.join("ca"),
        };
        let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        struct Release(WriterTestGate);
        impl Drop for Release {
            fn drop(&mut self) {
                let (lock, cv) = &*self.0;
                *lock.lock().unwrap_or_else(|p| p.into_inner()) = true;
                cv.notify_all();
            }
        }
        let release = Release(gate.clone());
        let (exit, exited) = std::sync::mpsc::channel();
        let (handle, _legacy) = start_internal(
            config.clone(),
            1,
            ProxyConfig::loopback(""),
            Duration::from_millis(100),
            Some(gate),
            Some(exit),
            true,
        )
        .await
        .unwrap();
        let submit = || {
            let mut a = handle
                .live_sink
                .begin(
                    crate::capture_lane::CaptureContext::at_entry(
                        "provisional:default",
                        "GET",
                        "/backend-api/conversation/panic",
                    ),
                    crate::capture_lane::CaptureMetadata {
                        host: "chatgpt.com".into(),
                        status: 200,
                        ..Default::default()
                    },
                )
                .unwrap();
            a.observe(br#"{"update_time":1,"mapping":{}}"#);
            a.finish();
        };
        submit();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let snapshot = serde_json::to_value(handle.capture_status().snapshot()).unwrap();
                if snapshot["counters"]["writer_in_flight"] == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        submit();
        let queued = serde_json::to_value(handle.capture_status().snapshot()).unwrap();
        drop(release);
        exited.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(queued["counters"]["queued"], 1);
        // Keep the actual handle alive: production supervisor cleanup must act.
        let final_state = serde_json::to_value(handle.capture_status().snapshot()).unwrap();
        assert_eq!(final_state["counters"]["started"], 2);
        assert_eq!(final_state["counters"]["store_failed"], 1);
        assert_eq!(final_state["counters"]["shutdown_unprocessed"], 1);
        for key in ["active", "queued", "writer_in_flight", "retained_bytes"] {
            assert_eq!(final_state["counters"][key], 0);
        }
        assert!(matches!(handle.status().state, ServiceState::Failed(_)));
        assert!(handle.live_receiver.lock().unwrap().is_closed());
        let _ = handle.shutdown().await;
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn live_writer_pending_across_deadline_is_not_double_counted() {
        let root = std::env::temp_dir().join(format!(
            "live-deadline-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let config = Config {
            capture_enabled: true,
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            proxy_addr: "127.0.0.1:0".parse().unwrap(),
            bearer_token: "synthetic".into(),
            database_path: root.join("db.sqlite").to_string_lossy().into_owned(),
            ca_dir: root.join("ca"),
        };
        let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        struct Release(WriterTestGate);
        impl Drop for Release {
            fn drop(&mut self) {
                let (lock, cv) = &*self.0;
                *lock.lock().unwrap_or_else(|p| p.into_inner()) = true;
                cv.notify_all();
            }
        }
        let release = Release(gate.clone());
        let (exit, exited) = std::sync::mpsc::channel();
        let (handle, _legacy) = start_internal(
            config.clone(),
            1,
            ProxyConfig::loopback(""),
            Duration::from_millis(100),
            Some(gate),
            Some(exit),
            false,
        )
        .await
        .unwrap();
        let capture = handle.capture_status();
        let mut accumulator = handle
            .live_sink
            .begin(
                crate::capture_lane::CaptureContext::at_entry(
                    "provisional:default",
                    "GET",
                    "/backend-api/conversation/pending",
                ),
                crate::capture_lane::CaptureMetadata {
                    host: "chatgpt.com".into(),
                    status: 200,
                    ..Default::default()
                },
            )
            .unwrap();
        accumulator.observe(br#"{"update_time":1,"mapping":{}}"#);
        accumulator.finish();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let snapshot = serde_json::to_value(capture.snapshot()).unwrap();
                if snapshot["counters"]["writer_in_flight"] == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let _ = handle.shutdown_timeout(Duration::from_millis(1)).await;
        let pending = serde_json::to_value(capture.snapshot()).unwrap();
        drop(release);
        exited.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(pending["counters"]["active"], 1);
        assert_eq!(pending["counters"]["shutdown_unprocessed"], 0);
        let final_state = serde_json::to_value(capture.snapshot()).unwrap();
        assert_eq!(final_state["counters"]["started"], 1);
        assert_eq!(final_state["counters"]["committed_imported"], 1);
        assert_eq!(final_state["counters"]["shutdown_unprocessed"], 0);
        assert_eq!(final_state["counters"]["active"], 0);
        assert_eq!(final_state["counters"]["retained_bytes"], 0);
        let store = Store::open(&config.database_path).unwrap();
        assert_eq!(store.library_snapshot().unwrap().conversations.len(), 1);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn blocked_writer_hits_deadline_after_sender_drop_and_accounts_queue() {
        let root = std::env::temp_dir().join(format!(
            "chat-hub-deadline-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let db = root.join("db.sqlite");
        let config = Config {
            capture_enabled: false,
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            bearer_token: "test".into(),
            database_path: db.display().to_string(),
            ca_dir: root.join("ca"),
            proxy_addr: "127.0.0.1:0".parse().unwrap(),
        };
        let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let (writer_exit_tx, writer_exit_rx) = std::sync::mpsc::channel();
        struct ReleaseGate(WriterTestGate);
        impl Drop for ReleaseGate {
            fn drop(&mut self) {
                let (lock, cv) = &*self.0;
                *lock.lock().unwrap_or_else(|p| p.into_inner()) = true;
                cv.notify_all();
            }
        }
        let release_gate = ReleaseGate(gate.clone());
        let (handle, tx) = start_internal(
            config,
            2,
            ProxyConfig::loopback(""),
            Duration::from_secs(30),
            Some(gate.clone()),
            Some(writer_exit_tx),
            false,
        )
        .await
        .unwrap();
        let status = handle.status.clone();
        let capture = |path: &str| CaptureEvent {
            host: "example.test".into(),
            path: path.into(),
            status: 200,
            body: br#"{"title":"t","mapping":{}}"#.to_vec(),
        };
        tx.send(capture("/backend-api/conversation/blocked"))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while handle.status().ingest.accepted != 1 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("writer did not dequeue first capture");
        tx.try_send(capture("/backend-api/conversation/queued"))
            .unwrap();
        drop(tx);
        let stats = handle.shutdown_timeout(Duration::from_millis(1)).await;
        assert!(
            matches!(lock_status(&status).state, ServiceState::Failed(ref reason) if reason.contains("shutdown deadline exceeded")),
            "unexpected service state: {:?}",
            lock_status(&status).state
        );
        assert_eq!(stats.accepted, 1, "{stats:?}");
        assert_eq!(stats.unprocessed, 2, "{stats:?}");
        assert_eq!(
            stats.stored + stats.ignored + stats.failed + stats.unprocessed,
            2,
            "{stats:?}"
        );
        // Deadline has returned while writer is explicitly held in-flight by the private gate.
        // Release before waiting for the actual writer-thread exit acknowledgment.
        drop(release_gate);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if writer_exit_rx.try_recv().is_ok() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("writer thread did not exit after gate release");
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod status_poison_tests {
    use super::*;

    #[test]
    fn poisoned_status_is_reported_as_failed_without_panicking() {
        let identity = server::RuntimeIdentity {
            pid: 1,
            instance_id: "test".into(),
        };
        let status = Arc::new(Mutex::new(ServiceStatus {
            state: ServiceState::Ready,
            identity,
            api_addr: None,
            proxy_addr: None,
            database_healthy: Some(true),
            ingest: IngestStats::default(),
        }));
        let poisoned = status.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poisoned.lock().unwrap();
            panic!("intentional status lock poison");
        })
        .join();
        let snapshot = lock_status(&status).clone();
        assert_eq!(
            snapshot.state,
            ServiceState::Failed("service status lock poisoned".into())
        );
    }
}

#[cfg(test)]
mod writer_panic_tests {
    use super::*;

    #[tokio::test]
    async fn writer_panic_fails_service_accounts_queue_and_closes_listeners() {
        let identity = server::RuntimeIdentity {
            pid: 1,
            instance_id: "panic-test".into(),
        };
        let status = Arc::new(Mutex::new(ServiceStatus {
            state: ServiceState::Ready,
            identity,
            api_addr: None,
            proxy_addr: None,
            database_healthy: Some(true),
            ingest: IngestStats {
                accepted: 1,
                ..IngestStats::default()
            },
        }));
        let (stop, stop_rx) = watch::channel(false);
        let (tx, rx) = mpsc::channel(2);
        tx.try_send(CaptureEvent {
            host: "example.test".into(),
            path: "/queued".into(),
            status: 200,
            body: vec![],
        })
        .unwrap();
        let receiver = Arc::new(Mutex::new(rx));
        let writer_stop = Arc::new(AtomicBool::new(false));
        let abandon = Arc::new(AtomicBool::new(false));
        let abandoned = Arc::new(AtomicU64::new(0));
        let registry: Arc<Mutex<Vec<tokio::task::AbortHandle>>> = Arc::new(Mutex::new(Vec::new()));
        let listeners = [
            tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(),
            tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(),
        ];
        let addresses: Vec<_> = listeners.iter().map(|l| l.local_addr().unwrap()).collect();
        let mut listener_tasks = Vec::new();
        for listener in listeners {
            let task = tokio::spawn(async move {
                let _ = listener.accept().await;
            });
            registry.lock().unwrap().push(task.abort_handle());
            listener_tasks.push(task);
        }
        supervise_writer(
            || panic!("intentional writer panic"),
            WriterFailureContext {
                capture: crate::live_capture::CaptureStatus::new(false),
                live_receiver: Arc::new(Mutex::new(crate::live_capture::channel(false).1)),
                stop: stop.clone(),
                status: status.clone(),
                writer_stop: writer_stop.clone(),
                abandon_queue: abandon.clone(),
                abandoned: abandoned.clone(),
                receiver: receiver.clone(),
                abort_registry: registry.clone(),
            },
        );
        for listener_task in listener_tasks {
            let result = tokio::time::timeout(Duration::from_secs(1), listener_task)
                .await
                .expect("listener cancellation did not complete within one second");
            assert!(
                result
                    .expect_err("aborted listener task unexpectedly succeeded")
                    .is_cancelled(),
                "listener task did not report cancellation"
            );
        }
        assert!(writer_stop.load(Ordering::Acquire));
        assert!(abandon.load(Ordering::Acquire));
        assert!(*stop_rx.borrow());
        assert_eq!(abandoned.load(Ordering::Acquire), 1);
        assert!(matches!(
            tx.try_send(CaptureEvent {
                host: "example.test".into(),
                path: "/closed".into(),
                status: 200,
                body: vec![],
            }),
            Err(mpsc::error::TrySendError::Closed(_))
        ));
        assert_eq!(status.lock().unwrap().database_healthy, Some(false));
        assert_eq!(
            status.lock().unwrap().state,
            ServiceState::Failed("capture writer panicked".into())
        );
        let store = Store::in_memory().unwrap();
        let interrupt = Arc::new(store.connection().get_interrupt_handle());
        let handle = ServiceHandle {
            live_sink: crate::live_capture::channel(false).0,
            capture: crate::live_capture::CaptureStatus::new(false),
            live_receiver: Arc::new(Mutex::new(crate::live_capture::channel(false).1)),
            status: status.clone(),
            shutdown: stop,
            tasks: vec![],
            listener_abort_handles: registry.lock().unwrap().clone(),
            writer: None,
            writer_stop,
            abandon_queue: abandon,
            abandoned,
            receiver,
            interrupt,
            deadline: Duration::from_millis(1),
        };
        let stats = handle.shutdown_timeout(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(stats.unprocessed, 2);
        assert!(matches!(
            status.lock().unwrap().state,
            ServiceState::Failed(_)
        ));
        for addr in addresses {
            let rebound = tokio::net::TcpListener::bind(addr).await;
            assert!(rebound.is_ok(), "listener remained bound at {addr}");
        }
    }
}
