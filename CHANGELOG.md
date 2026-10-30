# Changelog

## 0.2.0 candidate — unreleased draft

- Adds the local conversation library: normalized storage, export import, conversation graph and selected-branch browsing, body full-text search, and desktop library UI.
- Includes the disabled-by-default snapshot-capture path and native library backup/JSON export commands. Backup uses SQLite's consistent backup API; JSON export is streamed and is not a restore format. Existing destinations are refused, cancellation creates no file, and only `Error::ArchivePublished` means publication succeeded without confirmed durability. Native OS Save dialog/file acceptance remains unverified.
- Accepted PR23 debug-native fixture evidence covers import, library UI body, body search, selection, and alternate-branch detail/persistence in the same default OS-account app-data directory across same-process use, restart, and closure ([run 41110436605](https://github.com/iarnvidr/chat-hub/actions/runs/41110436605), source `6b7f1f9a41daf4e9894e4ab946acb1639fa48c9d`). This does not establish public DMG installation, provider capture, or native Save acceptance.
- This candidate is not a published release. Required exact-candidate macOS package, launch, signature, and DMG gates remain pending. The genuine unmodified v0.1.1 stable-feed update to v0.2.0 is not automated and remains a protected post-publication acceptance item. Ordering and an execution feasibility plan are pending; runner OS automation/trust and WKWebView automation permissions are unverified. No waiver is assumed.
- No real provider login/capture, public owner-downloaded DMG installation, or downloaded-app Gatekeeper behavior is claimed.

## 0.1.1

- Added an early macOS Objective-C uncaught-exception handler that writes exception name, reason, and call-stack symbols to stderr and (when configured) a diagnostic evidence file.
- Fixed packaged-app startup abort: the desktop `main` was entering a Tokio runtime before Tauri setup synchronously waited on its async runtime, causing Rust's `Cannot start a runtime from within a runtime` panic. Tauri now owns the sole desktop runtime; setup performs its bootstrap waits before the event loop enters runtime callbacks. The observed failure was a Rust panic followed by process abort, not an `NSException`; the earlier Objective-C exception diagnosis was not supported by the captured stderr.
- Required the packaged app startup and authenticated localhost health smoke in macOS CI and release preparation.
- Manual DMG upgrade acceptance from v0.1.0 remains unverified. The v0.1.0 startup failure's runtime-nesting root cause is fixed in source, but packaged macOS startup and the manual DMG upgrade still require their release gates. This version bump is not a release publication.
