# Chat Hub

Chat Hub is a local-first conversation library with a Rust service/CLI and a Tauri desktop app. The application stores imported conversations in a normalized SQLite library, preserves conversation branches, and supports full-text search over message content. The application metadata in this candidate is version 0.2.0; a version bump is not a release or release acceptance.

Provider capture is disabled by default. No real provider account, provider traffic capture, or production ingestion path has been accepted. The selective CONNECT proxy targets `chatgpt.com` and its subdomains; recognized authentication/sign-in hosts and other hosts tunnel without TLS decryption. The per-install CA is not automatically added to system trust. Linux proxy checks use synthetic traffic and do not establish real-provider behavior.

## Library and current evidence

The desktop library supports export import, conversation/message browsing, selected-branch and alternate-branch navigation, and body full-text search. A hosted debug-native smoke on Apple Silicon verified the official fixture import, library UI body, full-text search, selection, and alternate-branch detail and persistence in the same default OS-account app-data directory across same-process use, restart, and closure. The accepted run is [41110436605](https://github.com/iarnvidr/chat-hub/actions/runs/41110436605) for source `6b7f1f9a41daf4e9894e4ab946acb1639fa48c9d`. It did not test an alternate home/data-directory override, production DMG installation, real provider traffic, or native Save dialog/file acceptance. Raw native IPC health status was not retained as a separate artifact.

The native library Save implementation reuses the store's consistent SQLite backup and streamed JSON export APIs; JSON export is not a restore format. Existing destinations are refused, cancellation creates no file, and a normal success receipt confirms durability after sync. `Error::ArchivePublished` means the archive was published but durability could not be confirmed; the UI warns that the file exists and must not automatically overwrite or retry it. Native OS Save dialog/file acceptance remains unverified.

## Build and run

Requires stable Rust, rustfmt, and Clippy.

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
```

Set the following environment variables for the Rust CLI/service only; the production native desktop app uses Tauri's platform app-data directory and explicit database/CA paths, not these CLI/service overrides:

- `CHAT_HUB_TOKEN` — required non-empty bearer token.
- `CHAT_HUB_DATA_DIR` — optional data-directory override; by default macOS uses `~/Library/Application Support/io.github.iarnvidr.chat-hub`, Linux uses `$XDG_DATA_HOME/io.github.iarnvidr.chat-hub` or `~/.local/share/io.github.iarnvidr.chat-hub`, and Windows uses `%APPDATA%/io.github.iarnvidr.chat-hub`.
- `CHAT_HUB_DATABASE` — optional SQLite path; by default it is `chat-hub.sqlite3` inside the selected data directory.
- `CHAT_HUB_BIND` — API bind address (default `127.0.0.1:0`; loopback required).
- `CHAT_HUB_PROXY_ADDR` — proxy bind address (default `127.0.0.1:8788`; loopback required).
- `CHAT_HUB_CA_DIR` — optional CA directory; by default it is `ca` inside the selected data directory.
- `CHAT_HUB_CAPTURE_ENABLED` — snapshot capture opt-in; disabled unless set to `true` or `1`.

For example, run `CHAT_HUB_TOKEN=local-secret cargo run -- serve`; use `cargo run -- status`, `cargo run -- search words`, or `cargo run -- read <id>` for CLI commands. The API uses plaintext HTTP with bearer authentication on loopback. It is suitable only for a trusted local environment, not an untrusted network or as protection from local processes/users.

The browser fixture page is development-only and does not prove backend integration. The native desktop app uses the Rust/Tauri bridge. See [architecture](docs/ARCHITECTURE.md), [decisions](docs/DECISIONS.md), [known issues](KNOWN_ISSUES.md), [updater and release status](docs/UPDATES.md), [release checklist](RELEASE-CHECKLIST.md), [desktop UI](docs/DESKTOP_UI.md), and [data paths](docs/data-paths.md).

## Release and platform limits

The updater checks GitHub's stable `releases/latest` endpoint; preview prereleases require manual installation. Its pinned public key verifies updates. Ad-hoc app signing is not notarization, and release notes must warn that Gatekeeper may block first launch. The protected release workflow retains required compile, package, DMG, signing, manifest, startup/health, native library persistence, and native launch checks. Provider capture remains advisory. The hosted PR23 library smoke is not evidence that those production release gates passed.

Public owner-downloaded DMG installation, downloaded-app quarantine/Gatekeeper behavior, native Save dialog/file acceptance, real provider capture, advisory updater UI behavior, and the genuine unmodified v0.1.1 stable-feed update to v0.2.0 remain unverified. The genuine baseline update is not automated and cannot be run until the published candidate is available at `releases/latest`; its ordering and execution feasibility plan remain pending in protected approval. The production baseline has no debug IPC, and runner OS automation/trust and WKWebView automation permissions are unverified. No waiver is assumed. See [updater and release status](docs/UPDATES.md) and [release checklist](RELEASE-CHECKLIST.md).
