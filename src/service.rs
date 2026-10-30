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
pub struct ServiceHandle {
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
fn supervise_writer<F>(operation: F, cleanup: impl FnOnce())
where
    F: FnOnce(),
{
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)).is_err() {
        cleanup();
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
    pub fn status(&self) -> ServiceStatus {
        lock_status(&self.status).clone()
    }
    pub async fn shutdown(self) -> IngestStats {
        let d = self.deadline;
        self.shutdown_timeout(d).await
    }
    pub async fn shutdown_timeout(mut self, budget: Duration) -> IngestStats {
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
    mut proxy_config: ProxyConfig,
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
    let writer_abort_registry = abort_registry.clone();
    let writer_stop_service = stop.clone();
    let (writer_ready_tx, writer_ready_rx) = oneshot::channel();
    let writer_status = status.clone();
    let flag = Arc::new(AtomicBool::new(false));
    let wf = flag.clone();
    let abandon_queue = Arc::new(AtomicBool::new(false));
    let wa = abandon_queue.clone();
    let abandoned = Arc::new(AtomicU64::new(0));
    let writer_abandoned = abandoned.clone();
    let db = config.database_path.clone();
    let writer = Builder::new()
        .name("capture-ingestion".into())
        .spawn(move || {
            supervise_writer(
                || {
                    let mut store = match Store::open(&db) {
                        Ok(s) => s,
                        Err(e) => {
                            let _ = writer_ready_tx.send(Err(e.to_string()));
                            return;
                        }
                    };
                    let _ = store.connection.busy_timeout(Duration::from_millis(100));
                    let interrupt = Arc::new(store.connection.get_interrupt_handle());
                    let _ = writer_ready_tx.send(Ok(interrupt));
                    loop {
                        if wf.load(Ordering::Acquire) {
                            writer_receiver
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .close()
                        }
                        if wa.load(Ordering::Acquire) {
                            let mut rx = writer_receiver.lock().unwrap_or_else(|p| p.into_inner());
                            rx.close();
                            let queued = rx.len() as u64;
                            while rx.try_recv().is_ok() {}
                            writer_abandoned.fetch_add(queued, Ordering::AcqRel);
                            break;
                        }
                        // Holding the receiver lock through accepted accounting makes dequeue
                        // and deadline abandonment mutually exclusive, with no accounting gap.
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
                                let out = store.ingest_capture(ev);
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
                            Err(mpsc::error::TryRecvError::Disconnected) => break,
                            Err(mpsc::error::TryRecvError::Empty) => {
                                std::thread::sleep(Duration::from_millis(2))
                            }
                        }
                    }
                },
                || {
                    wf.store(true, Ordering::Release);
                    wa.store(true, Ordering::Release);
                    {
                        let mut rx = writer_receiver.lock().unwrap_or_else(|p| p.into_inner());
                        rx.close();
                        let queued = rx.len() as u64;
                        while rx.try_recv().is_ok() {}
                        writer_abandoned.fetch_add(queued, Ordering::AcqRel);
                    }
                    let mut status = lock_status(&writer_status);
                    status.database_healthy = Some(false);
                    if !matches!(status.state, ServiceState::Failed(_)) {
                        status.state = ServiceState::Failed("capture writer panicked".into());
                    }
                    drop(status);
                    let _ = writer_stop_service.send(true);
                    for abort in writer_abort_registry
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .iter()
                    {
                        abort.abort();
                    }
                },
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
    let (proxy, pt) = match proxy::run(proxy_config, tx.clone()).await {
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
    let api = tokio::spawn(async move {
        tokio::select! {_=axum::serve(listener,server::router_with_identity(api_store,config.bearer_token,identity))=>{},_=stop_rx.changed()=>{}}
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
        drop(tx);
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
        for listener in listeners {
            let task = tokio::spawn(async move {
                let _ = listener.accept().await;
            });
            registry.lock().unwrap().push(task.abort_handle());
            std::mem::forget(task);
        }
        let cleanup_status = status.clone();
        let cleanup_receiver = receiver.clone();
        let cleanup_stop = writer_stop.clone();
        let cleanup_abandon = abandon.clone();
        let cleanup_count = abandoned.clone();
        let cleanup_registry = registry.clone();
        supervise_writer(
            || panic!("intentional writer panic"),
            || {
                cleanup_stop.store(true, Ordering::Release);
                cleanup_abandon.store(true, Ordering::Release);
                let mut rx = cleanup_receiver.lock().unwrap();
                rx.close();
                let count = rx.len() as u64;
                while rx.try_recv().is_ok() {}
                cleanup_count.fetch_add(count, Ordering::AcqRel);
                let mut s = lock_status(&cleanup_status);
                s.database_healthy = Some(false);
                if !matches!(s.state, ServiceState::Failed(_)) {
                    s.state = ServiceState::Failed("capture writer panicked".into());
                }
                drop(s);
                let _ = stop.send(true);
                for abort in cleanup_registry.lock().unwrap().iter() {
                    abort.abort();
                }
            },
        );
        assert!(writer_stop.load(Ordering::Acquire));
        assert!(abandon.load(Ordering::Acquire));
        assert!(*stop_rx.borrow());
        assert_eq!(abandoned.load(Ordering::Acquire), 1);
        assert_eq!(status.lock().unwrap().database_healthy, Some(false));
        assert_eq!(
            status.lock().unwrap().state,
            ServiceState::Failed("capture writer panicked".into())
        );
        let store = Store::in_memory().unwrap();
        let interrupt = Arc::new(store.connection.get_interrupt_handle());
        let handle = ServiceHandle {
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
