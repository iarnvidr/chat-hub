# Known issues

PR1 implements a localhost API/CLI, SQLite initialization, and a selective CONNECT TLS proxy, but is not yet an end-to-end conversation hub.

- Proxy HTTP streaming/framing and allowlist behavior are under active correction; current proxy behavior is not production-ready.
- The capture scope is `chatgpt.com` and its subdomains, except recognized auth/sign-in hosts. Those and all other hosts tunnel without TLS decryption. No real provider traffic has been validated.
- Proxy capture events are not ingested into SQLite. No provider extraction, data export import, conversation/message model, or usable conversation UI exists yet; these are later PRs.
- Search is currently title matching, not full-text search over conversation content.
- The API uses plaintext HTTP with bearer authentication on loopback. It is trusted-local-only, not suitable for exposure to untrusted networks or as protection from local users/processes.
- CA trust is not installed automatically; CA lifecycle and user-guided trust management need further review.
- Synthetic Linux tests are not evidence of live ChatGPT/provider compatibility. macOS and WKWebView behavior have not been tested.
