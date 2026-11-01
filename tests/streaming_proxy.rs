mod common;

use chat_hub::{
    ca::Ca,
    capture_lane::{CaptureLimits, CaptureObservation, CaptureState},
    proxy::{self, CaptureEvent, ProxyConfig},
};
use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};
use std::{fs, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot},
};
use tokio_rustls::{TlsAcceptor, TlsConnector};

const GATE: Duration = Duration::from_secs(5);
struct Fixture {
    addr: std::net::SocketAddr,
    ca: Arc<Ca>,
    upstream_cfg: Arc<rustls::ServerConfig>,
    _dir: std::path::PathBuf,
    _task: tokio::task::JoinHandle<()>,
}
async fn fixture(
    limits: CaptureLimits,
    legacy_capacity: usize,
) -> (
    Fixture,
    mpsc::Receiver<CaptureObservation>,
    mpsc::Sender<CaptureEvent>,
    mpsc::Receiver<CaptureEvent>,
    TcpListener,
) {
    let dir = common::temp_dir("stream-proxy");
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let cert = CertificateParams::new(vec!["chatgpt.com".into()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let roots = dir.join("upstream.pem");
    fs::write(&roots, cert.pem()).unwrap();
    let uc = Arc::new(
        rustls::ServerConfig::builder_with_provider(
            rustls::crypto::ring::default_provider().into(),
        )
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            rustls_pemfile::certs(&mut cert.pem().as_bytes())
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            rustls_pemfile::private_key(&mut key.serialize_pem().as_bytes())
                .unwrap()
                .unwrap(),
        )
        .unwrap(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let la = listener.local_addr().unwrap();
    let (events_tx, events) = mpsc::channel(legacy_capacity);
    let (obs_tx, obs) = mpsc::channel(16);
    let mut cfg = ProxyConfig::loopback(dir.join("ca"));
    cfg.test_upstream = Some(la);
    cfg.test_roots = Some(roots);
    let (running, task) = proxy::run_with_capture(cfg, events_tx.clone(), limits, obs_tx)
        .await
        .unwrap();
    (
        Fixture {
            addr: running.addr,
            ca: running.ca,
            upstream_cfg: uc,
            _dir: dir,
            _task: task,
        },
        obs,
        events_tx,
        events,
        listener,
    )
}

async fn client(f: &Fixture) -> tokio_rustls::client::TlsStream<TcpStream> {
    let mut s = TcpStream::connect(f.addr).await.unwrap();
    s.write_all(b"CONNECT chatgpt.com:443 HTTP/1.1\r\nHost: chatgpt.com:443\r\n\r\n")
        .await
        .unwrap();
    let h = read_headers(&mut s).await;
    assert!(h.starts_with(b"HTTP/1.1 200"));
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(rustls::pki_types::CertificateDer::from(
            f.ca.cert_der().unwrap(),
        ))
        .unwrap();
    let cc = rustls::ClientConfig::builder_with_provider(
        rustls::crypto::ring::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    TlsConnector::from(Arc::new(cc))
        .connect(
            rustls::pki_types::ServerName::try_from("chatgpt.com").unwrap(),
            s,
        )
        .await
        .unwrap()
}
async fn read_headers<R: AsyncReadExt + Unpin>(r: &mut R) -> Vec<u8> {
    let mut v = Vec::new();
    let mut b = [0];
    loop {
        tokio::time::timeout(GATE, r.read_exact(&mut b))
            .await
            .unwrap()
            .unwrap();
        v.push(b[0]);
        if v.ends_with(b"\r\n\r\n") {
            return v;
        }
    }
}
async fn read_line<R: AsyncReadExt + Unpin>(r: &mut R) -> String {
    let mut v = Vec::new();
    loop {
        let b = read_fixed(r, 1).await[0];
        if b == b"\r"[0] {
            assert_eq!(read_fixed(r, 1).await, b"\n");
            break;
        }
        v.push(b)
    }
    String::from_utf8(v).unwrap()
}
async fn read_fixed<R: AsyncReadExt + Unpin>(r: &mut R, n: usize) -> Vec<u8> {
    let mut v = vec![0; n];
    tokio::time::timeout(GATE, r.read_exact(&mut v))
        .await
        .unwrap()
        .unwrap();
    v
}
async fn upstream(
    listener: Arc<TcpListener>,
    cfg: Arc<rustls::ServerConfig>,
) -> tokio_rustls::server::TlsStream<TcpStream> {
    let (s, _) = tokio::time::timeout(GATE, listener.accept())
        .await
        .unwrap()
        .unwrap();
    TlsAcceptor::from(cfg).accept(s).await.unwrap()
}
async fn observe(rx: &mut mpsc::Receiver<CaptureObservation>) -> CaptureObservation {
    tokio::time::timeout(GATE, rx.recv())
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn large_request_and_response_stream_before_upstream_completion() {
    let (f, mut obs, _tx, mut events, l) = fixture(CaptureLimits::default(), 8).await;
    let (release_tx, release_rx) = oneshot::channel();
    let uc = f.upstream_cfg.clone();
    let up = tokio::spawn(async move {
        let mut s = upstream(Arc::new(l), uc).await;
        let h = read_headers(&mut s).await;
        assert!(String::from_utf8_lossy(&h).contains("POST /large"));
        let n = 2 * 1024 * 1024 + 17;
        let b = read_fixed(&mut s, n).await;
        assert!(b.iter().all(|&x| x == b'x'));
        let prefix = vec![b'z'; 64];
        s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", n).as_bytes())
            .await
            .unwrap();
        s.write_all(&prefix).await.unwrap();
        s.flush().await.unwrap();
        release_rx.await.unwrap();
        s.write_all(&vec![b'z'; n - 64]).await.unwrap();
        s.shutdown().await.unwrap();
    });
    let mut c = client(&f).await;
    let n = 2 * 1024 * 1024 + 17;
    c.write_all(
        format!("POST /large HTTP/1.1\r\nHost: chatgpt.com\r\nContent-Length: {n}\r\n\r\n")
            .as_bytes(),
    )
    .await
    .unwrap();
    c.write_all(&vec![b'x'; n]).await.unwrap();
    let h = read_headers(&mut c).await;
    assert!(String::from_utf8_lossy(&h)
        .to_ascii_lowercase()
        .contains("content-length: 2097169"));
    assert_eq!(read_fixed(&mut c, 64).await, vec![b'z'; 64]);
    release_tx.send(()).unwrap();
    let body = read_fixed(&mut c, n - 64).await;
    assert!(body.iter().all(|&x| x == b'z'));
    up.await.unwrap();
    assert_eq!(observe(&mut obs).await.state, CaptureState::Oversized);
    let _ = events.try_recv();
}
#[tokio::test]
async fn full_legacy_queue_drops_capture_but_response_is_intact() {
    let (f, mut obs, tx, _events, l) = fixture(
        CaptureLimits {
            per_capture_bytes: 32,
            retained_bytes: 32,
            queue: 1,
        },
        1,
    )
    .await;
    let uc = f.upstream_cfg.clone();
    tx.try_send(CaptureEvent {
        host: "x".into(),
        path: "/".into(),
        status: 200,
        body: vec![],
    })
    .unwrap();
    let up = tokio::spawn(async move {
        let mut s = upstream(Arc::new(l), uc).await;
        let _ = read_headers(&mut s).await;
        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc")
            .await
            .unwrap();
    });
    let mut c = client(&f).await;
    c.write_all(b"GET /drop HTTP/1.1\r\nHost: chatgpt.com\r\n\r\n")
        .await
        .unwrap();
    let _ = read_headers(&mut c).await;
    assert_eq!(read_fixed(&mut c, 3).await, b"abc");
    assert_eq!(observe(&mut obs).await.state, CaptureState::Dropped);
    up.await.unwrap();
    f._task.abort();
}
#[tokio::test]
async fn truncated_upstream_response_is_interrupted_and_next_completes() {
    let (f, mut obs, _, mut _events, l) = fixture(
        CaptureLimits {
            per_capture_bytes: 8,
            retained_bytes: 64,
            queue: 8,
        },
        64,
    )
    .await;
    let uc = f.upstream_cfg.clone();
    let up = tokio::spawn(async move {
        let l = Arc::new(l);
        for i in 0..2 {
            let mut s = upstream(l.clone(), uc.clone()).await;
            let _ = read_headers(&mut s).await;
            if i == 0 {
                s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\nabc")
                    .await
                    .unwrap();
            } else {
                s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                    .await
                    .unwrap();
            }
        }
    });
    for i in 0..2 {
        let mut c = client(&f).await;
        c.write_all(b"GET /x HTTP/1.1\r\nHost: chatgpt.com\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let _ = read_headers(&mut c).await;
        if i == 1 {
            let _ = read_fixed(&mut c, 2).await;
        }
        let expected = if i == 0 {
            CaptureState::Interrupted
        } else {
            CaptureState::Complete
        };
        assert_eq!(observe(&mut obs).await.state, expected);
    }
    up.await.unwrap();
}
#[tokio::test]
async fn downstream_disconnect_interrupts_capture_and_reclaims_budget() {
    let limits = CaptureLimits {
        per_capture_bytes: 64,
        retained_bytes: 128,
        queue: 8,
    };
    let (f, mut obs, _, mut _events, listener) = fixture(limits, 8).await;
    let (release_tx, release_rx) = oneshot::channel();
    let uc = f.upstream_cfg.clone();
    let up = tokio::spawn(async move {
        let listener = Arc::new(listener);
        let mut first = upstream(listener.clone(), uc.clone()).await;
        let _ = read_headers(&mut first).await;
        first
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100000000\r\n\r\nprefix")
            .await
            .unwrap();
        // Keep upstream incomplete until the client has disconnected and capture has settled.
        release_rx.await.unwrap();
        drop(first);
        let mut next = upstream(listener, uc).await;
        let _ = read_headers(&mut next).await;
        next.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nok!!")
            .await
            .unwrap();
    });

    let mut c = client(&f).await;
    c.write_all(b"GET /disconnect HTTP/1.1\r\nHost: chatgpt.com\r\n\r\n")
        .await
        .unwrap();
    let _ = read_headers(&mut c).await;
    assert_eq!(read_fixed(&mut c, 6).await, b"prefix");
    drop(c);
    assert_eq!(observe(&mut obs).await.state, CaptureState::Interrupted);

    // Close the still-incomplete upstream, allowing the proxy's capture task to release its slot.
    release_tx.send(()).unwrap();
    let mut next = client(&f).await;
    next.write_all(b"GET /next HTTP/1.1\r\nHost: chatgpt.com\r\n\r\n")
        .await
        .unwrap();
    let _ = read_headers(&mut next).await;
    assert_eq!(read_fixed(&mut next, 4).await, b"ok!!");
    assert_eq!(observe(&mut obs).await.state, CaptureState::Complete);
    up.await.unwrap();
}

#[tokio::test]
async fn gzip_is_forwarded_and_marked_unsupported() {
    let (f, mut obs, _, _, l) = fixture(CaptureLimits::default(), 4).await;
    let uc = f.upstream_cfg.clone();
    let up = tokio::spawn(async move {
        let mut s = upstream(Arc::new(l), uc).await;
        let _ = read_headers(&mut s).await;
        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 4\r\n\r\n\x1f\x8b\x00\xff").await.unwrap();
    });
    let mut c = client(&f).await;
    c.write_all(b"GET /gzip HTTP/1.1\r\nHost: chatgpt.com\r\n\r\n")
        .await
        .unwrap();
    let h = read_headers(&mut c).await;
    assert!(String::from_utf8_lossy(&h)
        .to_ascii_lowercase()
        .contains("content-encoding: gzip"));
    assert_eq!(read_fixed(&mut c, 4).await, b"\x1f\x8b\x00\xff");
    assert_eq!(
        observe(&mut obs).await.state,
        CaptureState::UnsupportedEncoding
    );
    up.await.unwrap();
}
#[tokio::test]
async fn chunked_request_and_response_trailers_are_preserved() {
    let (f, mut obs, _, mut _events, l) = fixture(CaptureLimits::default(), 4).await;
    let uc = f.upstream_cfg.clone();
    let up = tokio::spawn(async move {
        let mut s = upstream(Arc::new(l), uc).await;
        let h = read_headers(&mut s).await;
        assert!(String::from_utf8_lossy(&h)
            .to_ascii_lowercase()
            .contains("te: trailers"));
        let mut raw = Vec::new();
        loop {
            let line = read_line(&mut s).await;
            let size = usize::from_str_radix(line.trim(), 16).unwrap();
            raw.extend_from_slice(&read_fixed(&mut s, size).await);
            if size != 0 {
                assert_eq!(read_fixed(&mut s, 2).await, b"\r\n");
            }
            if size == 0 {
                loop {
                    let trailer = read_line(&mut s).await;
                    if trailer.is_empty() {
                        break;
                    }
                    let (name, value) = trailer.split_once(':').unwrap();
                    if name.eq_ignore_ascii_case("x-req") {
                        assert_eq!(value.trim(), "yes");
                    }
                }
                break;
            }
        }
        assert_eq!(raw, b"abc");
        s.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nTrailer: X-Resp\r\n\r\n3\r\nhey\r\n0\r\nX-Resp: fine\r\n\r\n").await.unwrap();
    });
    let mut c = client(&f).await;
    c.write_all(b"POST /chunk HTTP/1.1\r\nHost: chatgpt.com\r\nTransfer-Encoding: chunked\r\nTrailer: X-Req\r\nTE: trailers\r\n\r\n3\r\nabc\r\n0\r\nX-Req: yes\r\n\r\n").await.unwrap();
    let h = read_headers(&mut c).await;
    assert!(
        String::from_utf8_lossy(&h)
            .to_ascii_lowercase()
            .contains("transfer-encoding: chunked")
            || String::from_utf8_lossy(&h)
                .to_ascii_lowercase()
                .contains("content-length")
    );
    let mut response = Vec::new();
    loop {
        let line = read_line(&mut c).await;
        let size = usize::from_str_radix(line.trim(), 16).unwrap();
        response.extend_from_slice(&read_fixed(&mut c, size).await);
        if size != 0 {
            assert_eq!(read_fixed(&mut c, 2).await, b"\r\n");
        }
        if size == 0 {
            let mut found = false;
            loop {
                let trailer = read_line(&mut c).await;
                if trailer.is_empty() {
                    break;
                }
                let (name, value) = trailer.split_once(':').unwrap();
                if name.eq_ignore_ascii_case("x-resp") {
                    assert_eq!(value.trim(), "fine");
                    found = true;
                }
            }
            assert!(found);
            break;
        }
    }
    assert_eq!(response, b"hey");
    assert!(matches!(
        observe(&mut obs).await.state,
        CaptureState::Complete | CaptureState::Interrupted
    ));
    up.await.unwrap();
}

#[tokio::test]
async fn global_retained_budget_drops_then_reclaims_for_later_capture() {
    let limits = CaptureLimits {
        per_capture_bytes: 8,
        retained_bytes: 8,
        queue: 8,
    };
    let (f, mut obs, _tx, events, listener) = fixture(limits, 8).await;
    let (release_tx, release_rx) = oneshot::channel();
    let uc = f.upstream_cfg.clone();
    let up = tokio::spawn(async move {
        let listener = Arc::new(listener);
        let mut release_rx = Some(release_rx);
        for i in 0..3 {
            let mut s = upstream(listener.clone(), uc.clone()).await;
            let _ = read_headers(&mut s).await;
            if i == 0 {
                s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\nABCD")
                    .await
                    .unwrap();
                let release = release_rx.take().unwrap();
                tokio::spawn(async move {
                    let _ = release.await;
                    s.write_all(b"EFGH").await.unwrap();
                });
            } else {
                s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nWXYZ")
                    .await
                    .unwrap();
            }
        }
    });
    let mut first = client(&f).await;
    first
        .write_all(b"GET /first HTTP/1.1\r\nHost: chatgpt.com\r\n\r\n")
        .await
        .unwrap();
    let _ = read_headers(&mut first).await;
    assert_eq!(read_fixed(&mut first, 4).await, b"ABCD");

    let mut second = client(&f).await;
    second
        .write_all(b"GET /second HTTP/1.1\r\nHost: chatgpt.com\r\n\r\n")
        .await
        .unwrap();
    let _ = read_headers(&mut second).await;
    assert_eq!(read_fixed(&mut second, 4).await, b"WXYZ");
    assert_eq!(observe(&mut obs).await.state, CaptureState::Dropped);
    release_tx.send(()).unwrap();
    assert_eq!(read_fixed(&mut first, 4).await, b"EFGH");
    assert_eq!(observe(&mut obs).await.state, CaptureState::Complete);
    let mut third = client(&f).await;
    third
        .write_all(b"GET /third HTTP/1.1\r\nHost: chatgpt.com\r\n\r\n")
        .await
        .unwrap();
    let _ = read_headers(&mut third).await;
    assert_eq!(read_fixed(&mut third, 4).await, b"WXYZ");
    assert_eq!(observe(&mut obs).await.state, CaptureState::Complete);
    up.await.unwrap();
    drop(events);
}
