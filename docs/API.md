# Public API (design contract; not yet implemented)

Chat Hub is local-first and standalone by default. The macOS app embeds the shared Rust + SQLite core and works offline; it does not require a server, account, network connection, or remote service. The current localhost in-app HTTP server is an implementation detail, not a deployment requirement or promise of a remote dependency. Existing PR1 routes (`/api/health`, `/api/search`, `/api/conversations/{id}`) are early internal endpoints, not this stable contract.

## Versioning and transport

The stable resource API is rooted at `/api/v1`. Breaking changes require a new major path (`/api/v2`); additive optional fields and endpoints are compatible. JSON uses UTF-8 and snake_case field names. Stable errors have `{"error":{"code":"...","message":"..."}}`; callers must not depend on message text. List/search responses are paginated with `items` and an opaque `next_cursor` (null at end). Ordering and cursor semantics must be stable for a given API version.

Authentication and bind policy are deployment-specific. Desktop loopback calls use local credentials and must remain loopback-only. A future headless companion requires explicit authentication, configurable bind address, and deliberate exposure; never bind publicly without authentication/TLS or claim the existing loopback bearer token secures an untrusted network. Do not put tokens in URLs or logs.

## Contract endpoints

All routes below are authenticated except where a deployment explicitly documents a safe health probe. JSON examples are shape illustrations, not implementation claims.

- `GET /api/v1/status`: readiness and storage status, e.g. `{"status":"ok"}`; unavailable storage is not ready.
- `GET /api/v1/version`: `{"version":"...","api_version":1}` for running core/service version and supported API version.
- `GET /api/v1/conversations?limit=&cursor=`: list conversation records; stable deterministic ordering, opaque continuation cursor.
- `GET /api/v1/search?q=&limit=&cursor=`: search records, same pagination envelope and ordering guarantees. Empty query behavior and limits must be documented by implementation; reject invalid limits rather than silently returning unbounded results.
- `GET /api/v1/conversations/{id}`: full conversation/messages or 404. IDs are canonical normalized identity keys, not titles or array positions.
- `GET /api/v1/tags` and `GET /api/v1/notes?conversation_id=`: user metadata listing; records include identity, value, and update timestamp.
- `GET /api/v1/links?conversation_id=`: user-created links/relationships, with stable link identity and target semantics.

Mutations and sync endpoints are deliberately not frozen here beyond the sync contract in [SYNC.md](SYNC.md). Define request validation, maximum payload/body sizes, conflict responses, and deletion semantics before implementation. Never expose captured conversation content in status/version responses.

## PR2 / headless companion guardrails (future PR, after updater)

The companion is optional and uses the exact same Rust core and SQLite schema/migrations as the desktop app; it is not a second implementation or a required remote backend. Add it only in a later PR after updater work/acceptance is complete. Provide `--service` explicitly; normal app launch stays the standalone desktop experience. A container image may be published to GHCR for amd64 and arm64, with documented configurable authentication, bind address and persistent data volume, plus health and version checks. The mounted volume is the authoritative durable database; document backup, permissions, migration and single-writer behavior. Container health must distinguish process-up from database/API-ready. Do not promise concurrent desktop and companion access to one SQLite file across machines or network filesystems.

Desktop application and companion image have independent release artifacts/tags but share one core version/schema lineage. Image tags must be traceable to the shared-core release; pin immutable digests for deployments and document upgrade/rollback constraints. The desktop updater updates the app only; it must not silently update, install, or operate a companion image. No image workflow, container implementation, or remote-default behavior is in scope for this documentation task.
