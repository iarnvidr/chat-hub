# WKWebView native HTTPS smoke

The Tauri service bridge exists, but this optional harness is fail-closed because packaged WKWebView synthetic-provider capture/ingestion and auth-host non-decryption are not accepted. `run-macos.sh` must not be treated as evidence while it fails closed. Provider capture remains advisory and unverified; see [updater and release status](../../docs/UPDATES.md).
