//! HTTP admission and transport for the shared synchronous importer. The import
//! and DB permits belong to the blocking operation after admission, so a client
//! disconnect never admits a competing import while durable commits continue.
use super::{error, invalid_request, unavailable, ApiError, ApiState};
use crate::importer::{self, ImportReceipt, MAX_IMPORT_BYTES};
use axum::{
    body::to_bytes,
    extract::{Request, State},
    http::{header, StatusCode},
    Json,
};

fn payload_too_large() -> ApiError {
    error(
        StatusCode::PAYLOAD_TOO_LARGE,
        "payload_too_large",
        "Import exceeds the 64 MiB limit.",
    )
}
fn body_error(error: axum::Error) -> ApiError {
    let mut source: &(dyn std::error::Error + 'static) = &error;
    loop {
        if source.is::<http_body_util::LengthLimitError>() {
            return payload_too_large();
        }
        let Some(next) = source.source() else {
            break;
        };
        source = next;
    }
    invalid_request()
}

pub(super) async fn import_chatgpt(
    State(s): State<ApiState>,
    request: Request,
) -> Result<Json<ImportReceipt>, ApiError> {
    let content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.split(';').next());
    if !content_type.is_some_and(|h| h.trim().eq_ignore_ascii_case("application/json")) {
        return Err(error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "Expected application/json.",
        ));
    }
    if let Some(length) = request.headers().get(header::CONTENT_LENGTH) {
        let length = length
            .to_str()
            .ok()
            .and_then(|h| h.parse::<u64>().ok())
            .ok_or_else(invalid_request)?;
        if length > MAX_IMPORT_BYTES as u64 {
            return Err(payload_too_large());
        }
    }
    let store = s.import_store.clone().ok_or_else(unavailable)?;
    let import_permit = s.import_ops.clone().try_acquire_owned().map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "busy",
            "An import is already in progress.",
        )
    })?;
    // Enforce actual streamed UTF-8 bytes even with absent/incorrect length.
    // The shared importer validates UTF-8 and the global JSON envelope before
    // processing any items. Axum's default 2 MiB extractor limit is not used.
    let bytes = to_bytes(request.into_body(), MAX_IMPORT_BYTES)
        .await
        .map_err(body_error)?;
    let db_permit = s
        .db_ops
        .clone()
        .try_acquire_owned()
        .map_err(|_| unavailable())?;
    tokio::task::spawn_blocking(move || {
        let _import_permit = import_permit;
        let _db_permit = db_permit;
        let mut store = store.lock().map_err(|_| unavailable())?;
        importer::import_chatgpt(&mut store, &bytes)
            .map(Json)
            .map_err(|e| match e.code {
                "invalid_request" => error(StatusCode::BAD_REQUEST, e.code, e.message),
                "payload_too_large" => payload_too_large(),
                _ => error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "Import failed.",
                ),
            })
    })
    .await
    .map_err(|_| unavailable())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, Bytes},
        http::Request,
    };
    use std::{
        convert::Infallible,
        pin::Pin,
        sync::{Arc, Mutex},
        task::{Context, Poll},
    };
    use tokio::sync::Semaphore;

    struct ReadyBody {
        data: Option<Bytes>,
        entered: Option<tokio::sync::oneshot::Sender<()>>,
    }
    impl hyper::body::Body for ReadyBody {
        type Data = Bytes;
        type Error = Infallible;
        fn poll_frame(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<hyper::body::Frame<Bytes>, Infallible>>> {
            if let Some(entered) = self.entered.take() {
                let _ = entered.send(());
            }
            Poll::Ready(
                self.data
                    .take()
                    .map(|bytes| Ok(hyper::body::Frame::data(bytes))),
            )
        }
    }
    struct OversizeBody(usize);
    impl hyper::body::Body for OversizeBody {
        type Data = Bytes;
        type Error = Infallible;
        fn poll_frame(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<hyper::body::Frame<Bytes>, Infallible>>> {
            if self.0 == 0 {
                return Poll::Ready(None);
            }
            self.0 -= 1;
            // Reuse a static chunk so the streamed boundary test does not
            // allocate 64 MiB of source data or depend on Content-Length.
            static CHUNK: [u8; 1024 * 1024] = [b'x'; 1024 * 1024];
            Poll::Ready(Some(Ok(hyper::body::Frame::data(Bytes::from_static(
                &CHUNK,
            )))))
        }
    }
    fn state(store: Arc<Mutex<crate::store::Store>>) -> ApiState {
        ApiState {
            capture: None,
            token: Arc::from("secret"),
            reader: None,
            db_ops: Arc::new(Semaphore::new(16)),
            import_store: Some(store),
            import_ops: Arc::new(Semaphore::new(1)),
        }
    }
    fn request(body: Body) -> axum::extract::Request {
        Request::builder()
            .method("POST")
            .uri("/api/v1/imports/chatgpt")
            .header("content-type", "application/json")
            .body(body)
            .unwrap()
    }

    #[tokio::test]
    async fn streamed_size_limit_rejects_before_any_durable_ingestion() {
        let store = Arc::new(Mutex::new(crate::store::Store::in_memory().unwrap()));
        let s = state(store.clone());
        let result = import_chatgpt(State(s.clone()), request(Body::new(OversizeBody(65)))).await;
        let err = result.err().unwrap();
        assert_eq!(err.0, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(err.1, "payload_too_large");
        assert_eq!(store.lock().unwrap().library_revision().unwrap(), 0);
        assert_eq!(s.import_ops.available_permits(), 1);
    }

    #[tokio::test]
    async fn cancelled_http_keeps_import_admission_until_real_commit_finishes() {
        let store = Arc::new(Mutex::new(crate::store::Store::in_memory().unwrap()));
        let s = state(store.clone());
        let (locked, held) = tokio::sync::oneshot::channel();
        let (release, resume) = std::sync::mpsc::channel();
        let blocked_store = store.clone();
        let blocker = std::thread::spawn(move || {
            let _held = blocked_store.lock().unwrap();
            locked.send(()).unwrap();
            resume
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
        });
        held.await.unwrap();
        let (entered, body_read) = tokio::sync::oneshot::channel();
        let body = Body::new(ReadyBody {
            data: Some(Bytes::from_static(include_bytes!(
                "../../../tests/fixtures/chatgpt-export/array.json"
            ))),
            entered: Some(entered),
        });
        let first = tokio::spawn(import_chatgpt(State(s.clone()), request(body)));
        // All body frames are immediately ready; the handler runs through
        // spawn_blocking before yielding. Its DB lock is intentionally held.
        body_read.await.unwrap();
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        let second = import_chatgpt(State(s.clone()), request(Body::from("[]"))).await;
        let err = second.err().unwrap();
        assert_eq!(err.0, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(err.1, "busy");
        release.send(()).unwrap();
        blocker.join().unwrap();
        let _finished = s.import_ops.acquire().await.unwrap();
        assert!(store.lock().unwrap().library_revision().unwrap() > 0);
    }
}
