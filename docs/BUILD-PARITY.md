# Build and release parity audit

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

The previously observed `muninn-macos-arm64` artifact was only 253 bytes and contains no real app bundle; it is failed-artifact evidence, not a build. No app, mounted DMG, signature, or launch-health PASS is claimed here. This environment is Linux, so the macOS build, codesign, hdiutil mount, app launch and native WKWebView/updater UI acceptance were not executable. `verify-macos.txt` must only be emitted by successful real bundle verification. Hosted macos-15 acceptance remains PENDING per `docs/UPDATES.md`.

The release path requires the configured updater public key, private signing secrets, and manual protected environment. It builds a real app, signs its update tarball, emits signature/manifest/source hashes and mount-verifies the DMG. Publication is gated by `publish=false` by default and is not performed by this audit.

## Exact remaining native shell integration gaps

The permitted port scope excludes `src-tauri/src`, Cargo manifests, and `desktop-ui`. The updater-smoke README describes required debug-only native hook/API commands, but their implementation must be audited/ported in the next desktop integration task. The existing smoke scripts are fail-closed harnesses, not feature implementation. Remaining acceptance: make registered Tauri updater APIs and debug-only feed overrides real; prove native version-0.1.0 to 0.1.1 install/relaunch and wrong-key plus tamper rejection in app UI; prove packaged WKWebView capture and ingestion via synthetic HTTPS provider with auth-host tunnel evidence. Also supply actual library UI functionality before claiming meaningful native UI interactions. Then run on hosted `macos-15`, retain screenshots/logs, and update `docs/UPDATES.md` with run evidence. Real provider account and downloaded-app Gatekeeper behavior remain manual.
