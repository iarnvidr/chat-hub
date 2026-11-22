# Headless server capture

Server mode can capture ChatGPT conversations without any remote view. A
headless Chromium inside the server container uses a persistent profile signed
in to ChatGPT, and all of its traffic goes through the existing selective capture
proxy (`crates/chat-hub-capture`). Conversations the browser loads reach the
library through the normal live-capture pipeline, and a background sweep keeps
history current.

## Not provided

No screencast, canvas, frame or input gateway, KasmVNC, iframe, CDP port or
script-evaluation surface. No sign-in happens inside a viewable browser, and no
PAC file or CA is installed on the owner's devices. A sign-in helper is a separate
later change; until then the profile must be seeded by the operator.

## Prompt flow and live view

The PWA is the only UI. With `capabilities.live` it shows New chat, Continue chat and a composer.

1. The composer posts `{conversation_id: uuid|null, text}` to `POST /api/live/prompt`
   (1 to 32768 bytes; same authentication, trusted-origin and CSRF checks as `/api/bridge`).
2. `chat-hub-web` forwards it over loopback HTTP to the sidecar `POST /prompt` with
   `Authorization: Bearer <secret>`. The sidecar answers `202 {accepted, job_id}` (or `409` when a
   prompt is already running) and runs the job in the background through `HeadlessBrowser.run_job`
   (the history sweep yields to a waiting prompt): it opens `https://chatgpt.com/` or
   `/c/<uuid>`, waits for the composer, fills the text, presses Enter and waits for the generation
   indicator to disappear (180 s limit).
3. ChatGPT's own page sends the request, so the capture proxy sees the `POST /backend-api/(f/)conversation`
   SSE response. While anything subscribes, the tee also feeds the existing `delta_encoding: v1`
   decoder incrementally (`crates/chat-hub-capture/src/live_capture/stream.rs`) and publishes text
   deltas of assistant text messages to a bounded in-process broadcast hub (256 events). With no
   subscriber nothing is decoded. Snapshot persistence and byte-identical forwarding are unchanged.
   Only identity-encoded responses are streamed live; compressed ones still arrive as the saved copy.
4. `GET /api/live/stream` relays those events as SSE. When the stream ends the PWA reloads the saved
   conversation from the library (the writer saves it a moment later, so it reloads after about 1.5 s).

Endpoints of `chat-hub-web` (all behind the auth modes `token`, `none` and `authelia`; absent with 404 unless live is configured):

| Route | Notes |
| --- | --- |
| `GET /api/live/stream` | `text/event-stream`, `no-store`. Needs auth, no CSRF. `: keepalive` comments every 15 s |
| `GET /api/live/status` | `{"state": "idle"\|"busy"\|"needs_sign_in"\|"error"\|"offline"}` only |
| `POST /api/live/prompt` | `202 {"accepted":true,"job_id":"..."}`, `400` invalid, `409 {"code":"busy"}`, `502` sidecar unavailable |

SSE events: `event: live` with `data: {"conversation_id": string|null, "message_id": string,
"kind": "start"|"delta"|"done"|"error", "text_delta": string}` (`text_delta` at most 4096 bytes), and
`event: resync` with `data: {}`. A client that falls more than 256 events behind gets `resync` instead
of buffered events; `error` means the stream was interrupted or malformed. Events never carry headers,
tokens, user text or resume tokens.

`capabilities` in server mode: `capture` true, `live` true only when capture is on and
`CHAT_HUB_BROWSER_SECRET_FILE` is set, `provider_view` always false.

Selectors for the ChatGPT composer, send and generation indicator are listed in one place
(`server-browser/browser.py`: `COMPOSER_SELECTORS`, `GENERATING_INDICATOR`) and need acceptance
against the real page.

## Processes

Run both in the `chat-hub-preview` container under a supervisor (or as two compose
services sharing `127.0.0.1` through one network namespace, and the same volumes):

1. `chat-hub-web` with `CHAT_HUB_WEB_CAPTURE=1`. It serves the read-only UI and also starts
   the capture proxy on loopback, writing into the same library database.
2. `server-browser/main.py` (image from `server-browser/Dockerfile`, non-root UID 10001,
   Chromium sandbox on, `server-browser/chromium-seccomp.json`). Mount the CA directory
   and the profile volume.

The operator deploys these; this repository does not.

## Environment

| Variable | Process | Meaning |
| --- | --- | --- |
| `CHAT_HUB_WEB_CAPTURE` | web | `1`/`true` enables, `0`/`false`/unset disables; anything else fails startup |
| `CHAT_HUB_PROXY_ADDR` | both | loopback proxy address, default `127.0.0.1:8788` |
| `CHAT_HUB_CA_DIR` | both | persistent CA directory (required); web creates it, browser reads `ca.pem` |
| `CHAT_HUB_CAPTURE_PROFILE_SCOPE` | web | capture profile scope, default `provisional:default` |
| `CHAT_HUB_BROWSER_ROOT` | browser | persistent volume root, default `/var/lib/chat-hub-browser` |
| `CHAT_HUB_BROWSER_STATUS_ADDR` | browser | loopback `GET /status` and `POST /prompt`, default `127.0.0.1:8790` |
| `CHAT_HUB_BROWSER_SECRET_FILE` | both | file with the shared bearer secret (16+ printable characters). Web: setting it enables live. Browser: unset disables `POST /prompt` |
| `CHAT_HUB_BROWSER_URL` | web | sidecar URL, `http://<loopback IP>:<port>`, default `http://127.0.0.1:8790` |
| `CHAT_HUB_SWEEP_INTERVAL_SECONDS` | browser | base sweep interval (jittered +/-20%), default 1800 |

With capture on, `/api/capabilities` reports `capture: true` (writes, provider view and
capture controls stay false) and `/healthz` reports `"capture":"enabled"`. Capture works
with `CHAT_HUB_WEB_AUTH=none` (loopback bind) and with token auth. The proxy refuses
non-loopback addresses.

## Trust model

- The capture CA is generated by the proxy at `CHAT_HUB_CA_DIR/ca.pem` (PEM, public) with its
  key beside it as `ca-key.pem`. The proxy intercepts TLS only for `chatgpt.com`; other hosts tunnel.
- `server-browser/browser.py::trust_ca` imports `ca.pem` with `certutil` (Debian package
  `libnss3-tools`) into `<root>/.pki/nssdb`. Chromium's `HOME` is the browser root, so the CA is
  trusted only in that profile, never system-wide.
- ChatGPT credentials exist only in the profile volume (`0700`). The status endpoint returns
  `state`, `last_sweep`, counters and the live state only: no URLs, titles, prompts or content.
- The sidecar listens on loopback only and `POST /prompt` requires the shared secret (constant-time
  comparison, checked before the body is read; body capped at 48 KiB, text at 32 KiB). The PWA never
  reaches the sidecar: only `chat-hub-web` does, after its own authentication, origin and CSRF
  checks. Keep the secret file readable by those two processes only. Prompt text is never logged,
  stored by these components or included in any status or error response.
- Keep `CHAT_HUB_CA_DIR` private: it holds the CA key. Anything able to read the key can forge
  `chatgpt.com` certificates for clients that trust the CA (here, only the browser profile).
- Request policy (`policy.py`): HTTPS only; pages limited to `chatgpt.com` (and `auth.openai.com`
  to recognise a signed-out redirect); sub-resources limited to `chatgpt.com`, `openai.com`,
  `oaistatic.com`, `oaiusercontent.com` and `challenges.cloudflare.com`; media blocked;
  downloads cancelled; service workers blocked.

## What the capture lane consumes

The lane stores successful `GET /backend-api/conversation/<id>` JSON snapshots and
`POST /backend-api/(f/)conversation` event streams. Conversation list responses are not stored.
The sweep therefore reads the sidebar's `/c/<id>` links from the page and opens the most recent
ones (10 per sweep) plus a backfill of up to 10 not yet visited in this process, by ordinary
navigation; ChatGPT's own page then issues the detail GET that the proxy captures. It never calls
the API itself. Visited IDs are held in memory only; a restart re-backfills, which the store
deduplicates. Compressed responses are forwarded but reported as unsupported by capture.

## States

`starting`, `sweeping`, `idle`, `needs_sign_in` (login page or login button seen; the collector
never signs in and retries on the next interval) and `error` (a sweep failed; the next interval
retries). Sign-in is a later change; until then the profile must be seeded by the operator. The live
state reported to the PWA is `busy` while a prompt runs, `needs_sign_in` (also after a prompt found the
login page), `error` (the last prompt failed) or `idle`. The PWA tells the owner to sign in with the local
helper, which this repository does not provide yet.

## Tests

`python3 -m unittest discover -s tests -p 'test_server_browser_*.py'` runs without a browser
(policy, `trust_ca` with a fake `certutil`, collector with a fake page, profile permissions, status
endpoint, prompt typing with a fake page, job serialization, secret, length cap and 409 busy). `server-browser/smoke.py` launches the real headless context and skips when
patchright or Chromium is absent.
