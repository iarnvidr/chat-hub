# Chat Hub web development loop

`chat-hub-web` runs the existing desktop UI from this checkout and serves its HTTP
bridge on port `8080`. The backend is intentionally opened on `0.0.0.0` so the
Paseo dev-container preview can reach it; use the preview's owner-only access
boundary for browser access. The server reports readiness at `GET /healthz`.
Provider capture is disabled in this development mode.

## Start

The restored library is provisioned by infrastructure. Set its path when starting
the server; the repository does not contain or locate the seed backup.

```sh
CHAT_HUB_DATABASE=/path/to/restored/chat-hub.sqlite3 scripts/dev-web.sh
```

`CHAT_HUB_LIBRARY_PATH` is accepted as an alias for `CHAT_HUB_DATABASE`. The
database must already be restored and must be a development copy, not a database
being actively written by another Chat Hub instance. The default address is
`0.0.0.0:8080`; override it with `CHAT_HUB_WEB_ADDR` if needed. The UI is served
from this checkout's `desktop-ui/` directory. Set `CHAT_HUB_DESKTOP_UI` to use a
different directory.

The first run creates a random access-token file at
`$XDG_STATE_HOME/chat-hub-web/access-token`, or
`~/.local/state/chat-hub-web/access-token` when `XDG_STATE_HOME` is unset. Its
parent directory is set to `0700` and file permissions to `0600`; subsequent
runs reuse it. Token creation is atomic and does not replace an existing file;
symlinked path components, symlink tokens, and unsafe token files are refused.
The token is never printed by the script or server. Use a local editor to read
the file when the login screen asks for it. Keep it private and do not paste it
into chat, issue text, URLs, or logs. Set `CHAT_HUB_WEB_TOKEN_FILE` to choose
another private path. Removing the file does not rotate a running server's
token; restart the server after rotating it.

Open `http://localhost:8080/` from the worker container, or use the configured
dev-container preview URL from the owner browser. To check readiness:

```sh
curl --fail --silent http://127.0.0.1:8080/healthz
```

The health response contains status and capture state only. The start script
also sets `CHAT_HUB_CAPTURE_ENABLED=false`; capture remains disabled in backend
development. Do not add chat content or credentials to server logs, Dagger
output, CI artifacts, or public repository text.

## Dagger and image publication

The existing `dagger check` remains the required validation gate. The web image
is a separate Dagger packaging operation so the desktop ZIP/DMG release path is
unchanged. Packaging reuses the locked Cargo cache volume; a compiled cache
artifact is not validation evidence. The image publisher runs only after a
successful trusted `main` check and uses an immutable GHCR digest. Its
machine-readable receipt binds the repository, source commit, check run and
attempt, publisher run, image digest, and `/healthz` contract. Infrastructure
should resolve and deploy only the digest after validating that receipt; tags
are discovery labels, never deployment identities. See the [Dagger check
guide](../ci/dagger/check/README.md) for the packaging command and receipt
schema.

The seed library is not available in public CI. CI uses synthetic fixtures and
must not publish or log private library content.

## Future server collection work

This comparison is based on repository documentation/source only; it does not
verify that either service is deployed, reachable, or healthy. The [infra
Browser Hub README](https://github.com/efthomsen/ginnungagap-infra/blob/main/services/homeserver/browser-hub/README.md)
describes anonymous verify/capture/PDF/named-fetch operations and anonymous
ephemeral or warm sessions. That documented contract does not provide the
persistent signed-in browser profile needed for ChatGPT collection, so Browser
Hub is not a drop-in collector. The [Muninn server/PWA guide](https://github.com/iarnvidr/muninn/blob/main/docs/server-pwa.md)
documents a dedicated Patchright/Chromium source browser, remote sign-in view,
private capture bridge, and provider-scoped browser state. For phase 3, review
and adapt those source-level packaging and lifecycle components rather than
extending Browser Hub's anonymous session model. Compatibility and live
deployment still require separate verification.
