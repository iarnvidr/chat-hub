# Chat Hub

Chat Hub is an early local-first conversation hub. PR1 provides a localhost Rust service and CLI, SQLite schema migration, and a selective TLS-intercepting CONNECT proxy. It is infrastructure, not yet a usable conversation library: provider ingestion, a browser UI, and the normalized conversation/message model are later-PR work.

## PR1 status

The service exposes bearer-protected `GET /api/health`, `/api/search?q=...`, and `/api/conversations/{id}` endpoints. The CLI supports `status`, `search <query>`, `read <id>`, and `serve`. The SQLite store initializes its schema on open. `serve` starts the API and proxy; capture events currently are not ingested into the database. Search currently matches stored titles, not full conversation text.

The loopback CONNECT proxy intercepts TLS for `chatgpt.com` and its subdomains, excluding recognized authentication/sign-in hosts, which tunnel without decryption. The configured CA is local and is not installed into system trust automatically. The exact capture host is `chatgpt.com`; other hosts, including sign-in hosts, tunnel. For this safe MVP, each intercepted HTTP request and response body is fully buffered and rejected with HTTP 413 if it exceeds 2 MiB; this hard limit bounds memory use, but streaming large provider payloads is deferred.

Linux verification is synthetic: tests exercise CONNECT/TLS forwarding and a synthetic response, alongside unit tests. This is not live ChatGPT/provider validation. There is no provider ingestion, import, web UI, or complete data model yet; those belong to later PRs. The PR2 hosted `macos-15` updater/WKWebView acceptance gate is separately pending; see below and the [updater policy](docs/UPDATES.md).

## Build and run

Requires stable Rust, rustfmt, and Clippy.

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
```

Set the following environment variables for CLI/service configuration:

- `CHAT_HUB_TOKEN` — required non-empty bearer token.
- `CHAT_HUB_DATABASE` — SQLite path (default `chat-hub.sqlite3`).
- `CHAT_HUB_BIND` — API bind address (default `127.0.0.1:0`; loopback required).
- `CHAT_HUB_PROXY_ADDR` — proxy bind address (default `127.0.0.1:8788`; loopback required).
- `CHAT_HUB_CA_DIR` — CA directory (default `chat-hub-ca`).

For example, run `CHAT_HUB_TOKEN=local-secret cargo run -- serve`; use `cargo run -- status`, `cargo run -- search words`, or `cargo run -- read <id>` for CLI commands. The API uses a plaintext HTTP bearer token on loopback. This is suitable only for a trusted local environment: do not expose it on an untrusted network or treat the token as protection against local processes/users.

See [architecture](docs/ARCHITECTURE.md), [decisions](docs/DECISIONS.md), [known issues](KNOWN_ISSUES.md), [updater policy](docs/UPDATES.md), and [release checklist](RELEASE-CHECKLIST.md).
## Desktop app and updater (PR2)

PR2 adds the Tauri desktop application and a signed-update verification path. The stable updater channel is deliberately limited to GitHub's `releases/latest`; prereleases are excluded. Preview prereleases are opt-in and remain explicitly preview software. Both channels must use the repository-pinned updater public key; ad-hoc app signing is not updater signing and does not imply notarization. Release notes for ad-hoc builds must state that they are not notarized and that Gatekeeper may block first launch.

The updater acceptance gate is pending hosted `macos-15` evidence. The required run must package the real app, install version 0.1.0, discover and install 0.1.1, and demonstrate rejection of both a tampered payload and a payload signed by the wrong key, with native UI screenshots. It must also exercise the packaged app's WKWebView capture proxy using a synthetic HTTPS provider: capture the decrypted response, ingest it, and verify sign-in/auth traffic traverses the tunnel undecrypted. Evidence links: **PENDING hosted macos-15 run** (workflow/run URL and screenshot/artifact links to be added after execution). This is not a passed check. The only remaining manual external acceptance is real ChatGPT login/account behavior and macOS quarantine/Gatekeeper behavior on a downloaded build; see [release checklist](RELEASE-CHECKLIST.md).
