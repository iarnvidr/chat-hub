# Build and release parity audit

This is a historical path-by-path port audit, not the current product-status or release-acceptance record. The current library implementation and bounded PR23 debug-native evidence are summarized in [architecture](ARCHITECTURE.md) and [updater/release status](UPDATES.md); the audit's implementation-gap statements below describe the earlier audit snapshot.

Comparison performed against the already-present public `iarnvidr/muninn` shallow clone at `51bd490c9fbbcaec1abb7b6b153dc40617dea6de` (also observed as remote HEAD). Clone origin is `https://github.com/iarnvidr/muninn.git`; it is shallow and clean. No GitHub API calls or extra clones were used. This is a path-by-path implementation audit, not a claim that Chat Hub has Muninn's product functionality.

## File-by-file comparison

| Muninn path(s) | Chat Hub counterpart/status | Parity/adaptation |
|---|---|---|
| `build/build-native-macos.sh`, `build/package-dmg.sh`, `build/release_sources.py` | same paths | Adapted Chat Hub binary, app name, bundle ID, source-manifest format, and DMG names. Build is a real locked Rust release binary and codesigns/verifies app; packaging mounts and verifies DMG. Must run on arm64 macOS; not run in this Linux audit. |
| `build/verify_macos.py`, `build/native_info.py` | no counterpart | Missing distinct machine-readable bundle verifier/native metadata helper; codesign, lipo and bundle checks exist inline in build/packaging. |
| `build/updater_release.py`, `build/test_updater_release.py` | `build/updater-smoke/{driver.py,run-macos.sh,verify_artifacts.py,verify_evidence.py,test_protocol.py}` | Different approach: workflow creates/signs a real Tauri archive/manifest; local/native updater smoke is debug-only. Do not treat protocol/artifact tests as native updater proof. |
| `build/native-ui-driver.py`, `build/native-player-interactions.js`, `build/native-search-interactions.js` and corresponding tests | `build/native-ui-smoke.sh`, `build/wkwebview-smoke/run-macos.sh` | Native UI/WKWebView smoke harnesses exist; no Muninn player/search interaction drivers (not relevant Chat Hub UI functionality yet). |
| `build/sign-notarize-macos.sh` | none | No notarization/signing identity path; artifacts are explicitly ad-hoc and not notarized. |
| `build/Dockerfile`, `build/Dockerfile.macos` | none | No Dockerized release build. Current CI runs hosted Linux/macOS jobs. |
| `build/check_publication.py`, `license_inventory.py` | `build/release_sources.py` | Corresponding-source bundle and hash manifest exist; no full publication/license inventory audit equivalent. |
| `build/tools.lock.json`, `fetch_tools.py`, `ffmpeg_lgpl.py` | no counterparts | Intentionally not ported: Chat Hub does not ship Muninn media tools/FFmpeg. |
| `build/export_icons.py`, `app-icon.svg`, `src-tauri/Info.plist`, `src-tauri/icons/**` | no counterparts | No equivalent icon export/native plist asset set. Tauri config supplies app metadata; release script writes Info.plist. |
| `.github/workflows/check.yml` | `.github/workflows/ci.yml` | Rust fmt/clippy/tests and macOS build exist; Chat Hub workflow is not a byte-for-byte port and currently uses floating action refs rather than Muninn's pinned revisions. |
| `.github/workflows/macos.yml` | same | Real app build, DMG, launch health and debug UI/updater evidence wiring. Upload expects named DMG and verify log; no evidence until hosted run. |
| `.github/workflows/release.yml` | same | Manual protected-main workflow, existing tag/version checks, `publish=false`, separate write-permission publish job. |
| `.github/CODEOWNERS`, issue templates, PR template | counterparts absent | Not build-critical; no port made. |
| `rust-toolchain.toml` | absent | CI installs stable; no pinned toolchain file. |
| Muninn contract tests (`test_release_production_contract.py`, `test_verify_macos.py`, `test_sign_notarize_macos.py`, `test_updater_smoke_wiring.py`, etc.) | `tests/test_release_contracts.py`, `build/updater-smoke/test_protocol.py` | Chat Hub has focused workflow/source-manifest contracts, not Muninn's complete production verifier/signing contracts. |

## Artifact and acceptance truth

The previously observed `muninn-macos-arm64` artifact was only 253 bytes and contains no real app bundle; it is failed-artifact evidence, not a build. This historical Linux audit did not produce an app, mounted DMG, signature, or launch-health result. Its local environment could not execute macOS build, codesign, hdiutil mount, app launch, or native WKWebView/updater UI acceptance. `verify-macos.txt` must only be emitted by successful real bundle verification. Current run-specific status is maintained in [updater and release status](UPDATES.md).

The release path requires the configured updater public key, private signing secrets, and manual protected environment. It builds a real app, signs its update tarball, emits signature/manifest/source hashes and mount-verifies the DMG. Publication is gated by `publish=false` by default and is not performed by this audit.

## Exact remaining native shell integration gaps

The permitted port scope in this historical audit excluded `src-tauri/src`, Cargo manifests, and `desktop-ui`. Current source includes the native updater interfaces and the desktop library UI; run 41110436605 provides bounded debug-native fixture evidence, not genuine packaged updater or provider-capture acceptance. Remaining acceptance is recorded in [updater/release status](UPDATES.md) and the [release checklist](../RELEASE-CHECKLIST.md). Real provider account behavior and downloaded-app Gatekeeper behavior remain manual.
