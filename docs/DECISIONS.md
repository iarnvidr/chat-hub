# Architecture and scope decisions

## PR1 foundation and order

PR1 establishes the local Rust service/CLI, SQLite initialization, authenticated localhost API, and selective CONNECT TLS proxy before higher-level library functionality. The API and CLI provide an early stable local integration surface. Subsequent work should add capture-event ingestion and the normalized conversation/message model before provider-specific extraction/import; UI and richer organization features follow once those data flows are reliable.

## Deliberate boundaries

The proxy selectively intercepts `chatgpt.com` and its subdomains while excluding recognized authentication/sign-in endpoints. Those excluded endpoints and other hosts tunnel without TLS decryption. A per-install CA is persisted, but trust is not silently installed. Current synthetic Linux verification demonstrates a local CONNECT/TLS round trip and capture event, not compatibility with a real provider.

PR1 does not claim provider ingestion, export import, complete conversation/message modeling, full-text conversation search, or a web UI. Those are later-PR work rather than implied capabilities. Proxy HTTP streaming/framing and allowlist behavior are still being corrected and need focused verification.

The API's bearer token travels over plaintext HTTP on loopback. This is a trusted-local-only boundary, not remote-network security and not isolation from other local processes or users. No live provider/account or macOS/WKWebView behavior is claimed; those remain explicit acceptance work.
