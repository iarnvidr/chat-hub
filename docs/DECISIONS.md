# Architecture and scope decisions

## PR1 foundation and order

PR1 established the local Rust service/CLI, SQLite initialization, authenticated localhost API, and selective CONNECT TLS proxy. Subsequent work added the normalized conversation library, export import, graph/branch navigation, body full-text search, and desktop UI. Those implemented library features do not establish real-provider capture or the full proposed lifecycle/account-scope contract below.

## Deliberate boundaries

The proxy selectively intercepts `chatgpt.com` and its subdomains while excluding recognized authentication/sign-in endpoints. Those excluded endpoints and other hosts tunnel without TLS decryption. A per-install CA is persisted, but trust is not silently installed. Current synthetic Linux verification demonstrates a local CONNECT/TLS round trip and capture event, not compatibility with a real provider.

Real provider ingestion and compatibility remain unverified. Capture is disabled by default; synthetic Linux proxy checks do not establish provider behavior. Public owner-downloaded DMG and native Save acceptance also remain pending; see [current known issues](../KNOWN_ISSUES.md).

The API's bearer token travels over plaintext HTTP on loopback. This is a trusted-local-only boundary, not remote-network security and not isolation from other local processes or users.

## PR2 desktop updater and evidence gate

The Tauri desktop updater is mandatory in PR2. Stable checks use only the `releases/latest` feed; preview prereleases are unsupported and require manual installation. Pin the updater verification public key in the application and validate signatures. Keep updater private signing material out of the repository. Ad-hoc app signing permits distribution but is not notarization; release notes must disclose non-notarized status and possible Gatekeeper blocking. Protected manual releases are prepared with `publish=false` until artifacts and evidence have been reviewed.

The hosted PR23 run [41110436605](https://github.com/iarnvidr/chat-hub/actions/runs/41110436605) verifies a bounded debug-native library fixture flow at source `6b7f1f9a41daf4e9894e4ab946acb1639fa48c9d`; it does not verify the original updater/WKWebView acceptance package. The controlled 0.1.0→0.1.1 updater fixture verifier accepted a valid payload and rejected a tampered artifact, but genuine stable-feed update, wrong-key rejection in the packaged flow, updater UI, and provider capture remain unproven. The genuine unmodified v0.1.1→0.2.0 stable-feed check is a protected post-publication acceptance item because the stable feed cannot expose an unpublished candidate; its ordering and execution feasibility plan remain pending. See [UPDATES](UPDATES.md) and the [release checklist](../RELEASE-CHECKLIST.md). Remaining manual items include real ChatGPT account behavior and downloaded-build quarantine/Gatekeeper behavior.
