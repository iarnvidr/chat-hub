# Shared service and normalization contract

**Status:** design contract, not implemented. **Version:** 0.1 (proposed). This document defines acceptance criteria for the next shared-service/model work; it does not authorize schema, proxy, or capture-worker changes. The current desktop native run at HEAD `3efa723` is a separate pending validation; this documentation commit changes no runtime behavior and is not evidence that run passed.

## Goal and compatibility boundary

CLI and desktop are two entry points to one local service and one durable conversation library. Both must use the same core configuration, data-directory resolution, SQLite database, and ordered schema migrations. CLI environment configuration and desktop app-data defaults may differ only in how they resolve the same explicit core configuration; do not fork service or migration semantics. Opening either entry point against the same configured data directory must expose the same records and migration result.

Preserve the current CLI commands (`status`, `search`, `read`, `serve`), API routes, and legacy four-field capture event/run integration during additive rollout. New capture metadata/context is additive; it must not silently alter old event meaning or make old callers fail to compile. This contract does not promise provider compatibility or permit proxy-scope expansion.

## Current implementation and gaps (source locations)

Line numbers refer to the source at base HEAD `3efa723`; recheck them when implementing.

| Location | Current behavior | Gap against this contract |
|---|---|---|
| `src/config.rs:4-10,13-46` | Central `APP_NAME`, env-derived bind/token/database/CA/proxy config; database defaults to relative `chat-hub.sqlite3`. | No shared explicit data-directory contract; desktop resolves its own app-data path and database filename. |
| `src/store.rs:5-22` | `Store::open` runs inline `CREATE TABLE IF NOT EXISTS` and inserts migration versions 1/2; minimal conversation/message tables. | No ordered migration runner, migration ownership, or transactional legacy namespace conversion. |
| `src/store.rs:25-39` | Health executes `SELECT 1`; search is conversation-title `LIKE`; read returns basic conversation/message fields. | No readiness contract beyond DB probe; no identity scope/full normalized records. |
| `src/capture.rs:6-24,36-70` | Recognizes successful ChatGPT detail response; fills missing conversation ID from path; stores string parts and parent IDs; upserts messages. | No profile/account scope, source/observation metadata, unknown-part preservation, explicit absent-node reconciliation, or revision/late-arrival policy. Existing message primary key is globally `id`. |
| `src/proxy.rs` (`CaptureEvent`; `run`) | Capture event currently carries host, status, path, and body; bounded proxy forwarding and tests exist. | No typed additive capture context/metadata/observation lifecycle in this contract's input. Proxy ownership remains with its separately coordinated workstream. |
| `src/main.rs:39-75` | CLI `serve` creates a bounded channel of 128, spawns a receiver that counts/logs one event, and serves API; no durable capture ingestion there. | Queue has no persistence, drain guarantee, observable health, or coordinated shutdown. |
| `src-tauri/src/main.rs:344-418` | Desktop chooses app-data `chat-hub.sqlite3`, starts channel capacity 128 and ingestion task, starts proxy and API. | Separate startup composition; receiver ignores ingestion errors; API bind is probed then dropped/rebound; tasks are not coordinated for readiness/shutdown. |
| `src/server.rs:18-34,82-89` | Authenticated health reports DB `SELECT 1`; `serve` binds internally; routes distinguish some internal errors. | No explicit retained-listener readiness or separate service/dependency health diagnostics; error taxonomy/metrics are not defined. |
| `tests/capture_ingest.rs`, `tests/proxy_e2e.rs`, `src/store.rs` tests | Existing tests cover simple capture ingestion, synthetic forwarding, and minimal store behavior. | No cross-entrypoint equivalence, crash/restart queue, migration fixture, lossless unknown parts, revision/merge, or shutdown-drain gate. |

## Normative data contract

### Identity and scope

Canonical source identity is the tuple `(provider, account_scope, source_id)`. Local database IDs are unique within the appropriate identity scope and must not be inferred from source IDs alone. A local conversation belongs to exactly one scoped identity. Message/node identity is likewise conversation-scoped; provider node IDs must never collide across conversations or accounts. Account scope is an explicit stable identifier when known. When not known, use a provisional namespace (for example `provisional:unknown` plus a capture/profile scope); never merge merely because provider IDs match.

An immutable profile/capture context is selected at request/connection entry and carried with observations through the full request. It must not be looked up later through mutable global “current account” state. Ambiguous account attribution remains provisional and visible; it is not permission to merge accounts. A future injectable profile-at-entry interface is required before multi-account capture; do not implement late global account selection.

### Conversation graph and fidelity

Persist the full provider parent graph, including nodes whose message payload is null. Preserve selected-branch/current-node state separately from graph structure; it is scoped to the conversation and must not destructively prune alternate branches. Preserve typed known content parts and unknown parts losslessly (including original structured payload). Flattened text may be a derived search/display view only, never the sole canonical representation.

Each capture observation records source timestamp when supplied, local observation time in milliseconds, source revision/hash where available, parser/provenance identity and version, and a completeness outcome (complete, partial, rejected, unsupported, or equivalent explicitly enumerated states). Missing nodes in a partial observation do not mean deletion. A complete snapshot may reconcile source-owned nodes, but must not erase locally retained nodes absent from an incomplete/partial payload. Older or late observations may enrich missing information but must not roll back newer content or selected-branch state. Define and test deterministic conflict/merge rules using source revision/hash and source/observation timestamps; timestamps alone are not a universal ordering guarantee.

### Capture envelope and compatibility

Additive capture types are proposed: `CaptureContext` (immutable profile scope/provisional namespace, observed-ms, correlation/method/path context), `CaptureMetadata` (host, status, content-type, encoding), and `CaptureObservation` with explicit lifecycle/completeness state and documented size/count/time limits. Preserve the existing four-field `CaptureEvent`/run contract unchanged during transition; adapters can construct richer observations where information exists. Do not store or emit bodies in metrics/logs. Limits must bound queued items, bytes per item, total staging bytes, and drain duration; oversize/unsupported outcomes must be explicit and observable, not silent truncation.

### Durable store and migration

SQLite is authoritative. Any bounded in-memory staging queue is a performance/backpressure layer only and cannot be the sole acknowledged copy. Persist accepted observations transactionally before acknowledging durable ingestion, or expose a clearly documented non-durable admission result distinct from success. A restart must recover all acknowledged work. Queue saturation must backpressure/reject with a typed outcome and metrics rather than unbounded memory growth.

Migrations are ordered, transactional, versioned, and shared by CLI and desktop. Legacy rows must migrate into an explicit provisional identity namespace without guessed account merges. Preserve source payload fidelity where available; migration must be idempotent, recoverable after interruption, and leave a backup/rollback strategy appropriate to the chosen migration. Derived indexes/FTS are rebuildable and never authoritative. Do not claim that current migration versions 1/2 satisfy this contract.

## Service lifecycle and observability

Bind API/proxy listeners once, retain the bound listener, and only publish readiness after required listeners, database/migrations, and worker/queue components are initialized. A bind failure is a startup/service error, not a ready process. Separate process/service readiness from database health and per-component health; report typed failure categories without secrets or capture bodies. The authenticated API must retain loopback-only binding and existing authorization protections.

Bound writer behavior must be explicit: one serialized writer (or equivalent transaction-safe ownership), bounded queue capacity and byte budget, transaction boundary per observation/batch, observable accepted/committed/rejected/failed counts and queue depth/bytes, and surfaced persistent write errors. Graceful shutdown stops admission, drains accepted work up to a documented deadline, commits/checkpoints as designed, then joins tasks and closes resources. Deadline expiry and failed drain are observable and must not be reported as clean shutdown. Startup recovery reports pending staged work if staging is durable.

## Acceptance fixtures and gates (required before implementation is called complete)

1. **Migration:** fixtures for current v1/v2 database, empty DB, interrupted/failed migration, and already-migrated DB. Verify idempotence, rollback/reopen behavior, legacy IDs in provisional scope, foreign-key integrity, and rebuildable indexes.
2. **Normalization:** fixtures for repeated source IDs under different provider/account scopes, unknown account scope, null-message graph nodes, alternate branches/selected branch, typed and unknown parts, partial then complete payloads, absent nodes, older/late revisions, duplicate observations, conflicting same-revision observations, and malformed/unsupported inputs. Assert no ambiguous merge, no lossy flattening, deterministic reconciliation, and no backwards selection/content overwrite.
3. **Capture/backpressure:** additive metadata/context round-trip while legacy four-field callers still work; bounds at and above item/queue limits; explicit oversize/rejection; no body contents in logs/metrics; provider-independent synthetic tests only.
4. **Lifecycle:** bind conflict, DB unavailable/corrupt, migration failure, writer failure, full queue, process crash/restart, orderly drain, drain timeout, and task termination. Readiness must not precede successful initialization; health distinguishes DB failure from other service failures.
5. **Entrypoint equivalence:** run identical fixtures/configured data directory through CLI and desktop composition; compare schema version, identities, normalized records, search/read results, and recovery after restart. Verify both use the same core config and migration implementation.
6. **Regression/security:** all existing Rust tests and formatting/lint checks; synthetic proxy forwarding tests remain green. No real account/body data is required or acceptable in fixtures.

## Sequencing, ownership, and decisions

First agree this shared contract and answer the open decisions below. Then define schema and migration ownership in one coordinated model/library change; only after its API is agreed may independent CLI/desktop integration and traffic/capture work proceed. Capture/proxy forwarding and tests remain owned by the separate capture-lane workstream; this document neither edits nor assigns its implementation. No parallel worker may independently invent schema or migrate tables. This document is a design deliverable, not implementation or a readiness claim.

### Decisions required before schema/API implementation

1. What stable account-scope identifier is available from each provider, and what exact provisional namespace/collision handling applies when it is absent?
2. Which source revision/hash and timestamp signals are trustworthy for ordering; what is the deterministic tie-break for conflicting equal-revision observations?
3. Which provider payload fields are canonical and how are unknown parts retained/versioned without unsafe size growth?
4. What are the numeric per-observation, queue-byte, staging-retention, and shutdown-drain limits and the user-visible behavior on exhaustion?
5. Which component owns migrations, backups/rollback, and the cross-entrypoint equivalence fixture so both entrypoints cannot drift?

Until these are resolved, do not claim a complete normalized model, durable ingestion, graceful drain, or shared service lifecycle.
