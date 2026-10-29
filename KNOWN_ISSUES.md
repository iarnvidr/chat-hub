# Known issues

PR1 implements a localhost API/CLI, SQLite initialization, and a selective CONNECT TLS proxy, but is not yet an end-to-end conversation hub.

- Proxy HTTP streaming/framing and allowlist behavior are under active correction; current proxy behavior is not production-ready.
- The capture scope is `chatgpt.com` and its subdomains, except recognized auth/sign-in hosts. Those and all other hosts tunnel without TLS decryption. No real provider traffic has been validated.
- Proxy capture events are not ingested into SQLite. No provider extraction, data export import, conversation/message model, or usable conversation UI exists yet; these are later PRs.
- Search is currently title matching, not full-text search over conversation content.
- The API uses plaintext HTTP with bearer authentication on loopback. It is trusted-local-only, not suitable for exposure to untrusted networks or as protection from local users/processes.
- CA trust is not installed automatically; CA lifecycle and user-guided trust management need further review.
- PR2's mandatory packaged-app updater acceptance on hosted `macos-15` remains pending: real app update 0.1.0→0.1.1, tamper/wrong-key rejection, native UI screenshots, and WKWebView proxy capture/ingest plus undecrypted auth tunnel evidence must be linked before claiming success.
- Real ChatGPT login/account behavior and downloaded-app quarantine/Gatekeeper behavior remain manual acceptance items. Do not characterize all macOS behavior as untested; the hosted macos-15 evidence gate is specific and pending.
