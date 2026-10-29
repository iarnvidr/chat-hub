# Architecture

Chat Hub is intended to be a local-first conversation hub, not a replacement chat frontend. Provider websites remain the place users browse and send messages. The current PR1 is a service/proxy foundation; provider ingestion and user-facing library features remain future work.

## Implemented in PR1

- Rust CLI/service entry points: `status`, `search`, `read`, and `serve`.
- SQLite initialization/migration, a minimal conversations table, and title-based lookup.
- Local Axum JSON endpoints: bearer-protected health, search, and conversation-read routes.
- Loopback-only API and proxy bind validation; service configuration is read from environment variables (see README).
- HTTP CONNECT proxy with selective TLS interception. `chatgpt.com` and its subdomains are the capture allowlist, subject to explicit auth/sign-in exclusions; other hosts tunnel without TLS decryption. A per-install CA is created/persisted but is not automatically installed as trusted.
- Synthetic Linux tests for CONNECT/TLS response forwarding and capture-event emission.

The service currently logs/consumes proxy events but does not ingest them into SQLite. The schema and search are minimal; search is title matching, not conversation-body FTS. There is not yet a normalized message/attachment model, import pipeline, provider extractor, UI, or integration with a client. These are intentionally later PRs.

## Boundaries and limitations

Proxy traffic processing is still being improved, particularly streaming HTTP framing and allowlist behavior. The synthetic Linux test does not establish correctness with a real provider. There has been no live ChatGPT/account test, no macOS/WKWebView validation, and no claim of provider compatibility. Authentication hosts must remain tunneled and undecrypted. CA trust installation/removal remains an explicit user/security concern.

This design is informed by [Muninn](https://github.com/iarnvidr/muninn), an Apache-2.0-licensed project with a local service and capture pipeline. See [NOTICE](../NOTICE) for attribution; this is not a claim of feature parity.
