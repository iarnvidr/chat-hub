# Release acceptance checklist

Record actual evidence; do not infer success from synthetic/Linux tests. The release-blocking minimum is an actual ad-hoc-signed app build, verified/mounted DMG, cryptographically signed updater payload verified independently with the configured pinned public key, and valid latest.json. Native launch/UI/WKWebView/updater integration smokes are nonblocking diagnostics.

## PR1 / Linux

- [ ] `cargo fmt --all -- --check` passes.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo test --all-targets` passes, including synthetic CONNECT/TLS forwarding and capture-event test.
- [ ] Confirm API/CLI behavior, bearer auth, loopback-only binds, and SQLite initialization.
- [ ] Confirm proxy streaming framing and allowlist fixes have landed and have focused tests; current work remains in progress.
- [ ] Gitleaks/dependency checks pass where run; record actual CI evidence.

## Release-blocking hosted macos-15 build gates

- [ ] Record run/artifact evidence for real Tauri app build with ad-hoc signature (not notarization).
- [ ] Record successful DMG create, verification, mount, and app-bundle checks.
- [ ] Record signed `Chat-Hub.app.tar.gz` and independent minisign verification against the pinned updater public key.
- [ ] Validate generated `latest.json` schema and payload signature; publish remains manual and defaults false.

## Diagnostic native smokes (nonblocking)

- [ ] Optionally retain launch/UI/WKWebView/updater integration smoke results and screenshots. Failures must be visible as failed diagnostics; they do not block the above required gates and do not count as passing evidence.
- [ ] Stable feed is `releases/latest` only; preview prereleases are explicitly opt-in. Verify pinned public key applies to both channels.
- [ ] Release notes identify ad-hoc builds as not notarized and disclose possible Gatekeeper block. Do not treat ad-hoc code signing as updater signing.
- [ ] Prepare protected manual publication with `publish=false`; publish only after review and authorization.

## Manual residual acceptance

- [ ] Validate actual ChatGPT login/account behavior with an authorized account.
- [ ] Assess downloaded-build quarantine/Gatekeeper behavior manually on macOS.

## Not yet verified / later acceptance

- [ ] Capture event ingestion into the database and real conversation extraction.
- [ ] ChatGPT data export import, complete conversation/message model, and full-text search.
- [ ] Service-hosted UI, browser tests, and UI evidence.
- [ ] Live ChatGPT capture with an authorized account. No real-provider behavior is implied by synthetic Linux tests.
- [ ] Any additional physical-Mac behavior beyond the hosted macos-15 updater/WKWebView evidence gate; record OS/version, build and result when performed.
- [ ] CA trust installation/removal and security review for local data and access controls.

## Release hygiene

- [ ] Keep documentation aligned with the implemented PR scope and explicit limitations.
- [ ] Confirm no credentials, private conversations, or private infrastructure details are present.
- [ ] Link passing CI and test evidence; retain untested items above until verified.
