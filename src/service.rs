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
    sender: mpsc::WeakSender<CaptureEvent>,
    interrupt: Arc<rusqlite::InterruptHandle>,
    deadline: Duration,
}
impl ServiceHandle {
    pub fn status(&self) -> ServiceStatus {
        self.status.lock().unwrap().clone()
    }
    pub async fn shutdown(self) -> IngestStats {
        let d = self.deadline;
        self.shutdown_timeout(d).await
    }
    pub async fn shutdown_timeout(mut self, budget: Duration) -> IngestStats {
        self.status.lock().unwrap().state = ServiceState::Stopping;
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
                let queued = self
                    .sender
                    .upgrade()
                    .map(|tx| tx.max_capacity().saturating_sub(tx.capacity()) as u64)
                    .unwrap_or(0);
                let mut s = self.status.lock().unwrap();
                s.state = ServiceState::Failed(
                    "shutdown deadline exceeded; SQLite interrupted; writer still finishing".into(),
                );
                s.ingest.unprocessed = s
                    .ingest
                    .accepted
                    .saturating_sub(s.ingest.stored + s.ingest.ignored + s.ingest.failed)
                    .saturating_add(queued);
                return s.ingest.clone();
            }
        }
        let mut s = self.status.lock().unwrap();
        s.state = ServiceState::Stopped;
        s.ingest.unprocessed = s
            .ingest
            .accepted
            .saturating_sub(s.ingest.stored + s.ingest.ignored + s.ingest.failed);
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
    let (tx, mut rx) = mpsc::channel(capacity);
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
    let (writer_ready_tx, writer_ready_rx) = oneshot::channel();
    let writer_status = status.clone();
    let flag = Arc::new(AtomicBool::new(false));
    let wf = flag.clone();
    let abandon_queue = Arc::new(AtomicBool::new(false));
    let wa = abandon_queue.clone();
    let db = config.database_path.clone();
    let writer = Builder::new()
        .name("capture-ingestion".into())
        .spawn(move || {
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
                    rx.close()
                }
                if wa.load(Ordering::Acquire) {
                    // Expired shutdown must not start more SQLite work. Count queued
                    // events as unprocessed before dropping them from the receiver.
                    let queued = rx.len() as u64;
                    if queued != 0 {
                        let mut s = writer_status.lock().unwrap();
                        s.ingest.unprocessed = s.ingest.unprocessed.saturating_add(queued);
                    }
                    while rx.try_recv().is_ok() {}
                    break;
                }
                match rx.try_recv() {
                    Ok(ev) => {
                        let mut s = writer_status.lock().unwrap();
                        s.ingest.accepted += 1;
                        drop(s);
                        let out = store.ingest_capture(ev);
                        let mut s = writer_status.lock().unwrap();
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
            let mut state = api_status.lock().unwrap();
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
            let mut state = proxy_status.lock().unwrap();
            if !matches!(state.state, ServiceState::Stopping | ServiceState::Stopped) {
                state.state = ServiceState::Failed("proxy listener exited unexpectedly".into());
            }
            drop(state);
            proxy_flag.store(true, Ordering::Release);
            let _ = proxy_stop.send(true);
        }
    });
    {
        let mut s = status.lock().unwrap();
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
            sender: tx.downgrade(),
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
