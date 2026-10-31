# Chat Hub

Chat Hub is a local-first conversation library with a Rust service/CLI and a Tauri desktop app. The application stores imported conversations in a normalized SQLite library, preserves conversation branches, and supports full-text search over message content. The application metadata in this candidate is version 0.2.1; a version bump is not a release or release acceptance.

Provider capture is disabled by default. No real provider account, provider traffic capture, or production ingestion path has been accepted. The selective CONNECT proxy intercepts only the exact host `chatgpt.com`; subdomains, authentication/sign-in hosts and other hosts tunnel without TLS decryption. The per-install CA is not automatically added to system trust. Linux proxy checks use synthetic traffic and do not establish real-provider behavior.

## Library and current evidence

The desktop library supports export import, conversation/message browsing, selected-branch and alternate-branch navigation, and body full-text search. A hosted debug-native smoke on Apple Silicon verified the official fixture import, library UI body, full-text search, selection, and alternate-branch detail and persistence in the same default OS-account app-data directory across same-process use, restart, and closure. The accepted run is [41110436605](https://github.com/iarnvidr/chat-hub/actions/runs/41110436605) for source `6b7f1f9a41daf4e9894e4ab946acb1639fa48c9d`. It did not test an alternate home/data-directory override, production DMG installation, real provider traffic, or native Save dialog/file acceptance. Raw native IPC health status was not retained as a separate artifact.

The native library actions save a consistent SQLite backup, export a one-way JSON interchange file, and restore a SQLite backup. Restore asks before replacing the current library, validates and stages the backup before stopping the service, then restarts Chat Hub with the restored library. Native file-dialog and restart acceptance still needs a manual desktop run.

The desktop opens saved conversations in individual tabs and defaults to ChatGPT history. Existing Paseo cached transcripts retain their exact text with a provenance notice; the cache does not contain alternate branches. For a recognized ChatGPT conversation ID, **Continue in ChatGPT** opens that specific live chat within the reader; **Read saved copy** returns to its local tab. Sign-in, sending and live reconciliation still require acceptance. Large-transcript reading now budgets JSON escaping and normalized parts accurately; search uses graph topology without loading complete message bodies. Conversations whose full detail exceeds the 8 MiB response limit still need paged reading.

**Connect ChatGPT** uses the desktop app's own provider session. **Sync history** fetches conversation lists and full conversation graphs directly from ChatGPT through that session, then passes bounded chunks to the shared local importer. Credentials remain inside the provider webview; that webview cannot invoke privileged native commands. Each chunk is committed before fetching continues. This replaces the native dependency on the Paseo plugin and its deployed cache. A future plugin can connect to Chat Hub's service; no Paseo infrastructure is required for this flow. Signed-in history fetching and new-chat capture remain acceptance items until tested with a real session.

The CLI restore command remains available for restoring into an empty library: set `CHAT_HUB_DATABASE` to the destination path and run `cargo run -- restore /path/to/library.sqlite3`. It validates the backup, applies compatible migrations, rebuilds all search indexes, and prints a receipt. JSON exports cannot be restored.

The local index also extracts repositories, pull requests, issues, domains, links and file references with links back to their source chats and messages. Overview categories search the full index before pagination and respect source, project and local tag filters. File references distinguish produced files, attachments and mentions; content is saved only after an explicit local file attachment, with Open and Save copy actions for saved bytes. This is not automatic downloading from ChatGPT. SQLite backups include saved file bytes; JSON exports contain their metadata only.

Local tags are stored independently of the derived index and survive rebuilds. The desktop uses the classified terminal palette, distinct speaker styles and stable graph positions. Cmd/Ctrl+W closes the selected tab and does nothing when no tabs are open. Native menu shortcuts also work while the embedded ChatGPT view has focus. Cmd/Ctrl+T opens a new chat; Ctrl+Tab cycles tabs and Cmd/Ctrl+1–9 selects tabs (9 selects the last).

Projects expand inline in the sidebar, with the selected project retained when collapsed. Settings → Appearance offers five saved color schemes: Classified amber, Phosphor, Arctic, Violet and Field notes. Color changes also update the graph without losing its position.

## Build and run

Requires stable Rust, rustfmt, and Clippy.

Install or update the Rust CLI locally:

```sh
sh build/install-cli.sh
chat-hub --help
chat-hub status
chat-hub search diablo
chat-hub read <chat-id>
```

The installer builds an optimized binary into `~/.local/bin`, which must be on your shell's `PATH`. Set `CHAT_HUB_CLI_INSTALL_ROOT` to choose another installation root. The CLI shares the desktop library by default, works offline for library commands, and needs no manually supplied token for those commands. `status` discovers the desktop app or a standalone service using their private local runtime files. Live ChatGPT sign-in, fetching and sending currently use the desktop app; the CLI does not implement those browser operations.

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
```

Capture defaults off. The desktop offers **Save live chats on this Mac**, with an explicit save and restart action. It stores a private preference in the app data directory and waits for the local writer to stop before restarting. For an explicit development override, use `CHAT_HUB_CAPTURE_ENABLED=true CHAT_HUB_CAPTURE_PROFILE_SCOPE=provisional:default cargo tauri dev`. An environment value overrides the saved preference. Request-local account claims are attributed to opaque scopes; real account-switch and restart acceptance remains pending. Captured snapshots include conversation content in the local library.

Set the following environment variables for the Rust CLI/service; the native desktop app shares only the capture opt-in and profile-scope settings above, and uses Tauri's platform app-data directory and explicit database/CA paths instead of the CLI/service path overrides:

- `CHAT_HUB_TOKEN` — required non-empty bearer token for `serve`; unnecessary for local library commands and `status`.
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
