//! Selective loopback CONNECT TLS interception proxy.
use crate::ca::{allowed_capture_host, Ca};
use crate::capture_lane::{
    Accumulator, CaptureContext, CaptureLane, CaptureLimits, CaptureMetadata,
};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::{
    body::{Body as _, Frame, Incoming},
    Request, Response,
};
use hyper_util::rt::TokioIo;
use std::{net::SocketAddr, path::PathBuf, sync::Arc};
use std::{
    pin::Pin,
    task::{Context, Poll},
};

fn rustls_provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::mpsc,
};
pub const MAX_CAPTURE_BODY: usize = 2 * 1024 * 1024;
#[derive(Clone, Debug)]
pub struct ProxyConfig {
    pub ca_dir: PathBuf,
    pub bind: SocketAddr,
    #[doc(hidden)]
    pub test_upstream: Option<SocketAddr>,
    #[doc(hidden)]
    pub test_roots: Option<PathBuf>,
}
impl ProxyConfig {
    pub fn loopback(ca_dir: impl Into<PathBuf>) -> Self {
        Self {
            ca_dir: ca_dir.into(),
            bind: "127.0.0.1:0".parse().unwrap(),
            test_upstream: None,
            test_roots: None,
        }
    }
}
#[derive(Clone, Debug)]
pub struct CaptureEvent {
    pub host: String,
    pub path: String,
    pub status: u16,
    pub body: Vec<u8>,
}
#[derive(Debug)]
pub enum ProxyError {
    Io(std::io::Error),
    Ca(crate::ca::CaError),
    NonLoopback,
}
impl std::fmt::Display for ProxyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ProxyError {}
impl From<std::io::Error> for ProxyError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<crate::ca::CaError> for ProxyError {
    fn from(e: crate::ca::CaError) -> Self {
        Self::Ca(e)
    }
}
pub struct RunningProxy {
    pub addr: SocketAddr,
    pub ca: Arc<Ca>,
}
pub async fn run(
    config: ProxyConfig,
    events: mpsc::Sender<CaptureEvent>,
) -> Result<(RunningProxy, tokio::task::JoinHandle<()>), ProxyError> {
    let (observations, receiver) = mpsc::channel(CaptureLimits::default().queue);
    drop(receiver);
    run_with_capture(config, events, CaptureLimits::default(), observations).await
}
/// Start the proxy with capture and a fixed profile scope for every intercepted request.
///
/// The scope is bounded by [`CaptureContext::at_entry`] and injected when a request
/// enters the proxy, before waiting for the upstream response. Existing [`run_with_capture`]
/// remains the provisional-scope convenience API. EOS in capture observations means the
/// upstream response body completed cleanly; it does not imply downstream delivery.
pub async fn run_with_capture_context(
    config: ProxyConfig,
    events: mpsc::Sender<CaptureEvent>,
    limits: CaptureLimits,
    observations: mpsc::Sender<crate::capture_lane::CaptureObservation>,
    profile_scope: impl Into<String>,
) -> Result<(RunningProxy, tokio::task::JoinHandle<()>), ProxyError> {
    if !config.bind.ip().is_loopback() {
        return Err(ProxyError::NonLoopback);
    }
    let ca = Arc::new(Ca::load_or_create(&config.ca_dir)?);
    let listener = TcpListener::bind(config.bind).await?;
    let addr = listener.local_addr()?;
    let lane = CaptureLane::new(events.clone(), observations, limits);
    let c = ca.clone();
    let test = config.test_upstream;
    let roots = config.test_roots;
    let profile_scope = CaptureContext::at_entry(profile_scope.into(), "", "").profile_scope;
    let task = tokio::spawn(async move {
        while let Ok((s, _)) = listener.accept().await {
            let ca = c.clone();
            let lane = lane.clone();
            let roots = roots.clone();
            let profile_scope = profile_scope.clone();
            tokio::spawn(async move {
                let _ = client(s, ca, lane, test, roots, profile_scope).await;
            });
        }
    });
    Ok((RunningProxy { addr, ca }, task))
}
/// Start capture using the temporary `provisional:unknown` profile scope.
pub async fn run_with_capture(
    config: ProxyConfig,
    events: mpsc::Sender<CaptureEvent>,
    limits: CaptureLimits,
    observations: mpsc::Sender<crate::capture_lane::CaptureObservation>,
) -> Result<(RunningProxy, tokio::task::JoinHandle<()>), ProxyError> {
    run_with_capture_context(config, events, limits, observations, "provisional:unknown").await
}
/// Forwards every upstream response frame as-is while retaining bounded data-only bytes.
struct TeeBody {
    inner: Incoming,
    capture: Option<Accumulator>,
    finished: bool,
}
impl TeeBody {
    fn new(inner: Incoming, capture: Accumulator) -> Self {
        let mut body = Self {
            inner,
            capture: Some(capture),
            finished: false,
        };
        if body.inner.is_end_stream() {
            body.finish();
        }
        body
    }
    fn finish(&mut self) {
        if !self.finished {
            self.finished = true;
            if let Some(capture) = self.capture.take() {
                capture.finish();
            }
        }
    }
}
impl Drop for TeeBody {
    fn drop(&mut self) {
        if !self.finished {
            if let Some(capture) = self.capture.take() {
                capture.interrupt();
            }
        }
    }
}
impl hyper::body::Body for TeeBody {
    type Data = Bytes;
    type Error = hyper::Error;
    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }
    fn size_hint(&self) -> hyper::body::SizeHint {
        self.inner.size_hint()
    }
    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        match Pin::new(&mut self.inner).poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    if let Some(capture) = &mut self.capture {
                        capture.observe(data);
                    }
                }
                if self.inner.is_end_stream() {
                    self.finish();
                }
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(None) => {
                self.finish();
                Poll::Ready(None)
            }
            Poll::Ready(Some(Err(err))) => Poll::Ready(Some(Err(err))),
            Poll::Pending => Poll::Pending,
        }
    }
}
fn empty_body() -> UnsyncBoxBody<Bytes, hyper::Error> {
    Full::new(Bytes::new())
        .map_err(|never| match never {})
        .boxed_unsync()
}

async fn client(
    mut s: TcpStream,
    ca: Arc<Ca>,
    lane: CaptureLane,
    test: Option<SocketAddr>,
    roots: Option<PathBuf>,
    profile_scope: String,
) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut b = [0; 1];
    while buf.len() < 16384 {
        let n = s.read(&mut b).await?;
        if n == 0 {
            return Ok(());
        }
        buf.push(b[0]);
        if buf.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let line = String::from_utf8_lossy(&buf);
    let Some(authority) = line
        .lines()
        .next()
        .unwrap_or("")
        .strip_prefix("CONNECT ")
        .and_then(|x| x.split_whitespace().next())
        .map(str::to_owned)
    else {
        s.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n").await?;
        return Ok(());
    };
    let Some((host, port)) = authority.rsplit_once(':') else {
        return Ok(());
    };
    let port = port.parse::<u16>().unwrap_or(443);
    let dest = if let Some(a) = test {
        TcpStream::connect(a).await?
    } else {
        TcpStream::connect((host, port)).await?
    };
    if !allowed_capture_host(host) {
        s.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .await?;
        let (mut a, mut b) = (s, dest);
        let _ = tokio::io::copy_bidirectional(&mut a, &mut b).await;
        return Ok(());
    }
    s.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
        .await?;
    let (cert, key) = ca.leaf_identity(host).map_err(std::io::Error::other)?;
    let certs = rustls_pemfile::certs(&mut cert.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(std::io::Error::other)?;
    let key = rustls_pemfile::private_key(&mut key.as_bytes())
        .map_err(std::io::Error::other)?
        .ok_or(std::io::ErrorKind::InvalidData)?;
    let cfg = rustls::ServerConfig::builder_with_provider(rustls_provider())
        .with_safe_default_protocol_versions()
        .map_err(std::io::Error::other)?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(std::io::Error::other)?;
    let incoming = tokio_rustls::TlsAcceptor::from(Arc::new(cfg))
        .accept(s)
        .await?;
    let mut r = rustls::RootCertStore::empty();
    if let Some(path) = roots {
        let pem = std::fs::read(path)?;
        for c in rustls_pemfile::certs(&mut pem.as_slice()) {
            r.add(c.map_err(std::io::Error::other)?)
                .map_err(std::io::Error::other)?
        }
    } else {
        r.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned())
    }
    let mut cc = rustls::ClientConfig::builder_with_provider(rustls_provider())
        .with_safe_default_protocol_versions()
        .map_err(std::io::Error::other)?
        .with_root_certificates(r)
        .with_no_client_auth();
    cc.alpn_protocols = vec![b"http/1.1".to_vec()];
    let name =
        rustls::pki_types::ServerName::try_from(host.to_owned()).map_err(std::io::Error::other)?;
    let upstream = tokio_rustls::TlsConnector::from(Arc::new(cc))
        .connect(name, dest)
        .await?;
    let (sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(upstream))
        .await
        .map_err(std::io::Error::other)?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let sender = Arc::new(tokio::sync::Mutex::new(sender));
    hyper::server::conn::http1::Builder::new()
        .serve_connection(
            TokioIo::new(incoming),
            hyper::service::service_fn(move |req: Request<Incoming>| {
                let host = host.to_owned();
                let path = req
                    .uri()
                    .path_and_query()
                    .map(|u| u.as_str())
                    .unwrap_or("/")
                    .to_owned();
                let context = CaptureContext::at_entry(
                    profile_scope.clone(),
                    req.method().as_str(),
                    path.clone(),
                );
                let sender = sender.clone();
                let lane = lane.clone();
                async move {
                    let (parts, body) = req.into_parts();
                    let upstream_req = Request::from_parts(parts, body);
                    let result = sender.lock().await.send_request(upstream_req).await;
                    match result {
                        Ok(resp) => {
                            let (parts, body) = resp.into_parts();
                            let metadata = CaptureMetadata {
                                host,
                                status: parts.status.as_u16(),
                                content_type: parts
                                    .headers
                                    .get(hyper::header::CONTENT_TYPE)
                                    .and_then(|v| v.to_str().ok())
                                    .map(str::to_owned),
                                content_encoding: parts
                                    .headers
                                    .get(hyper::header::CONTENT_ENCODING)
                                    .and_then(|v| v.to_str().ok())
                                    .map(str::to_owned),
                            };
                            let capture = lane.begin(context, metadata);
                            let tee = TeeBody::new(body, capture);
                            Ok::<_, std::convert::Infallible>(Response::from_parts(
                                parts,
                                tee.boxed_unsync(),
                            ))
                        }
                        Err(_) => Ok(Response::builder().status(502).body(empty_body()).unwrap()),
                    }
                }
            }),
        )
        .await
        .map_err(std::io::Error::other)?;
    Ok(())
}
