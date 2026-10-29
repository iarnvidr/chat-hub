# WKWebView native HTTPS smoke

The required native app bridge does not yet exist. `run-macos.sh` intentionally fails closed; it must not be treated as evidence. Implement app-scoped CONNECT proxy, per-install CA trust, response capture/ingest and synthetic sign-in non-decryption before enabling this smoke.
