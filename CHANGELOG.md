# Changelog

## 0.1.1

- Added an early macOS Objective-C uncaught-exception handler that writes exception name, reason, and call-stack symbols to stderr and (when configured) a diagnostic evidence file.
- Required the packaged app startup and authenticated localhost health smoke in macOS CI and release preparation.
- Manual DMG upgrade acceptance from v0.1.0 remains unverified. The v0.1.0 app reportedly crashes on startup and therefore cannot self-update; the root cause is pending diagnosis. This version bump is not a release publication.
