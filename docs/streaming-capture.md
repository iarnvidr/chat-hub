# Streaming capture: architecture and current limits

This document describes the current proxy capture path. It is a bounded, best-effort observation path alongside forwarding; it is not a durable ingestion pipeline and does not claim that a browser accepted or rendered a response.

## Interception and forwarding

The loopback CONNECT proxy intercepts TLS only for the exact host `chatgpt.com` (case-insensitive). Other hosts are tunneled without TLS interception. For intercepted HTTP/1.1 traffic, requests are forwarded upstream as received; the proxy does not buffer or rewrite request bodies for capture. Responses are wrapped in a tee body: each upstream data frame is observed for capture while that frame, along with non-data frames/trailers, is forwarded unchanged. Capture therefore does not reframe or transform the response sent onward.

The request context is initialized at entry, before forwarding, with method and path. The current proxy uses the provisional profile scope `provisional:unknown`; the context API also permits a caller to inject a profile scope at entry. The provisional value is a temporary single-profile namespace, not an account identity and not safe for merging multi-account data. The proxy does not currently resolve a global/account profile. Context values are bounded (profile scope 256 UTF-8 bytes, method 32, path 4096); a timestamp and correlation number are also assigned.

## Bounded capture lane

`CaptureLane` defaults to a 2 MiB maximum per capture, a 16 MiB aggregate retained-byte budget, and an internal queue capacity of 32. It has one worker consuming queued captures. A full per-capture slot is reserved when accumulation begins, rather than charging only bytes already observed. Thus active accumulators and completed captures waiting in the worker queue both count against the retained budget; the reservation travels with queued work and is released when processing completes or work is discarded. If a full reservation cannot be obtained, that capture is marked `Dropped` immediately. This conservative accounting bounds allocation and retained capture memory, at the cost of rejecting work even when a body would have been small.

The queue of legacy `CaptureEvent`s supplied by the caller is a separate bound, controlled by that caller; it is not included in the lane's 32-item queue or 16 MiB budget. The worker uses nonblocking `try_send` for that legacy event. If its queue is full or closed, the result is `Dropped`; it does not clone/retry the body. The observation channel is also nonblocking. A failed observation send increments `lost_observations`; this counter makes reporting loss possible, but cannot deliver an observation when that channel is unavailable. State counters are best-effort runtime counters, not durable records.

`CaptureEvent` remains the existing compatible shape (`host`, `path`, `status`, `body`). `CaptureObservation` separately provides context, response metadata, and a terminal state.

## Completion and terminal states

`Complete` means the proxy observed upstream response-body end-of-stream and the capture was accepted into the lane for worker processing. It does not prove the browser accepted the final bytes: downstream delivery can still fail after upstream EOS. For normal identity-encoded content, the worker attempts legacy event delivery; a full/closed legacy queue changes the observed result to `Dropped`.

The capture states currently include:

- `Interrupted`: the tee/capture is dropped before upstream body EOS, including an upstream body error followed by teardown. An upstream error is forwarded as an error; capture is not declared complete merely because the response began.
- `Oversized`: observed data exceeds the per-capture limit. Capture is abandoned and its reservation released; response forwarding continues unchanged.
- `Dropped`: capacity could not be reserved, the internal lane queue rejected completed work, or legacy event delivery failed.
- `UnsupportedEncoding`: response metadata declares a non-identity content encoding. The captured bytes remain wire-encoded; there is no gzip (or other content-encoding) decoder. Such a body is not emitted as a legacy JSON snapshot.

Capture currently retains data frames only. It does not build a general representation of trailers or other frame metadata; those frames are still forwarded. Successful legacy events are raw response body bytes with response host/path/status. Any JSON interpretation/snapshot is limited to available captured body data; there is no new-turn detection or conversation stitching in this path.

## Scope and pending work

## Integration status and boundaries

The production-ring work is implemented in both explicit builders (`src/proxy.rs` and `src/capture_lane.rs`). This is equivalent to PR6 commit `e18dba5` and preserves the existing `CaptureEvent` / `run` compatibility API. This statement is limited to that production-ring work; it does not claim that other PR6 work is included.

`run_with_capture_context(config, events, limits, observations, profile_scope)` allows a caller to supply a fixed profile scope. It is injected at request entry (with method/path, timestamp, and correlation) and bounded to 256 UTF-8 bytes. It is a bounded-scope construction entry point, not account resolution or a multi-account-safe identity system; `run_with_capture` and `run` continue to use `provisional:unknown`.

Non-identity content encodings currently produce `UnsupportedEncoding`; the bytes are not decoded, and this behavior is an acknowledged pending limitation. No integration tests are claimed here. The lane is in-memory and best effort: no durable storage, complete conversation stitching, or downstream-delivery guarantee. Runtime diagnostics expose counters and response/context metadata only; they do not include captured body contents or credentials.
