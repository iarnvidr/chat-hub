# Architecture and scope decisions

## PR1 foundation and order

PR1 establishes the local Rust service/CLI, SQLite initialization, authenticated localhost API, and selective CONNECT TLS proxy before higher-level library functionality. The API and CLI provide an early stable local integration surface. Subsequent work should add capture-event ingestion and the normalized conversation/message model before provider-specific extraction/import; UI and richer organization features follow once those data flows are reliable.

## Deliberate boundaries

The proxy selectively intercepts `chatgpt.com` and its subdomains while excluding recognized authentication/sign-in endpoints. Those excluded endpoints and other hosts tunnel without TLS decryption. A per-install CA is persisted, but trust is not silently installed. Current synthetic Linux verification demonstrates a local CONNECT/TLS round trip and capture event, not compatibility with a real provider.

PR1 does not claim provider ingestion, export import, complete conversation/message modeling, full-text conversation search, or a web UI. Those are later-PR work rather than implied capabilities. Proxy HTTP streaming/framing and allowlist behavior are still being corrected and need focused verification.

The API's bearer token travels over plaintext HTTP on loopback. This is a trusted-local-only boundary, not remote-network security and not isolation from other local processes or users.

## PR2 desktop updater and evidence gate

The Tauri desktop updater is mandatory in PR2. Stable checks use only the `releases/latest` feed; preview prereleases are unsupported and require manual installation. Pin the updater verification public key in the application and validate signatures. Keep updater private signing material out of the repository. Ad-hoc app signing permits distribution but is not notarization; release notes must disclose non-notarized status and possible Gatekeeper blocking. Protected manual releases are prepared with `publish=false` until artifacts and evidence have been reviewed.

Acceptance is evidence-gated on hosted `macos-15`: exercise packaged app update 0.1.0→0.1.1, tampered and wrong-key rejection, native UI screenshots, and WKWebView proxy capture/ingest with auth hosts tunneled undecrypted. This run is currently pending, not passed; link actual run evidence in [UPDATES](UPDATES.md) and the [release checklist](../RELEASE-CHECKLIST.md). Remaining manual external checks are real ChatGPT login/account behavior and downloaded-build quarantine/Gatekeeper behavior. This narrows the remaining macOS claims; it is not a blanket statement that macOS is untested.
