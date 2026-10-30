# Shared service and proposed normalized capture contract

## Implemented service

`service::start(Config, capacity)` / `start_with_proxy` start the loopback API and interception proxy, open separate API and ingestion `Store` connections, and run a bounded Tokio `mpsc` capture queue with one dedicated SQLite writer thread. Capacity must be positive; API/proxy bind addresses must be loopback and the bearer token nonempty. Proxy capture events currently contain exactly the legacy fields `host`, `path`, `status`, and `body`; captured bodies are bounded to 2 MiB. `try_submit` is available when callers need immediate queue-full feedback. The API router exposes authenticated health and existing conversation endpoints; runtime identity includes PID and a generated instance ID.

Status tracks lifecycle, listener addresses, database health, and aggregate ingestion counts; it does not expose capture bodies or credentials. `accepted` counts events dequeued by the writer, not successful channel submissions; stored, ignored, failed and unprocessed are distinct outcomes. The writer uses a 100 ms SQLite busy timeout. API reads/writes still use their own Store connection and existing synchronous SQLite/mutex path on Tokio; the writer separation does not remove that API-side blocking constraint. Ingestion errors increment `failed`; they are not a claim that an atomic cross-connection transaction succeeded.

Shutdown requests listener stop, closes the writer receiver and allows queued work to drain within the requested budget (default five seconds). On expiry it interrupts SQLite and marks remaining work unprocessed where observable, but there is no hard guarantee that an in-progress SQLite operation or writer thread is killed by the deadline. Accepted proxy connections / their spawned client tasks are not fully tracked or guaranteed drained by service shutdown. Queue-full and drain-deadline behavior tests are under repair; do not treat them as passing evidence. Desktop/Tauri wiring is a primary-owned follow-up: no desktop parity or native integration proof is claimed here.

## Proposed normalized contract — pending agreement, not implemented

This is a proposed data contract only. It makes no schema/store-model change and must not be described as normalized persistence already implemented.

- Identify a conversation by the composite `(provider, account scope, provider conversation ID)` identity. Provider IDs alone are not globally unique; do not merge accounts ambiguously. Define canonical IDs deterministically from explicit identity inputs.
- Pass stable entry context explicitly into capture/parse calls (including provider/account scope); do not resolve identity from a mutable late global. Agree the exact shape and lifetime of this context before implementation.
- Preserve the full message graph, including parent relationships, explicit null-message nodes, and current-branch metadata. Preserve known parts as typed values and unknown provider-specific parts as typed opaque payloads rather than dropping them.
- Attach observation provenance, observation time, body hash, parser version, and completeness to captured observations. Define these fields and hash semantics before persistence.
- Merge partial or late observations non-destructively and idempotently: absence in an observation is not deletion, older snapshots missing nodes do not remove them, and a more complete observation cannot be downgraded by an incomplete one. Explicit deletion, if supported, needs a distinct agreed tombstone semantics.

### Compatibility and integration dependency

Preserve the existing `CaptureEvent` / `run` compatibility surface and its four fields (`host`, `path`, `status`, `body`); do not duplicate those fields in a new envelope. The current service has no immutable traffic context attached to events. Supplying that context depends on the separate PR 8 integration; until it lands and the contract is agreed, do not claim account-aware capture or normalization. These are pending decisions, not current APIs, schema guarantees, or migration instructions.
