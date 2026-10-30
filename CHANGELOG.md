# Changelog

## 0.1.1

- Added an early macOS Objective-C uncaught-exception handler that writes exception name, reason, and call-stack symbols to stderr and (when configured) a diagnostic evidence file.
- Fixed packaged-app startup abort: the desktop `main` was entering a Tokio runtime before Tauri setup synchronously waited on its async runtime, causing Rust's `Cannot start a runtime from within a runtime` panic. Tauri now owns the sole desktop runtime; setup performs its bootstrap waits before the event loop enters runtime callbacks. The observed failure was a Rust panic followed by process abort, not an `NSException`; the earlier Objective-C exception diagnosis was not supported by the captured stderr.
- Required the packaged app startup and authenticated localhost health smoke in macOS CI and release preparation.
- Manual DMG upgrade acceptance from v0.1.0 remains unverified. The v0.1.0 startup failure's runtime-nesting root cause is fixed in source, but packaged macOS startup and the manual DMG upgrade still require their release gates. This version bump is not a release publication.
