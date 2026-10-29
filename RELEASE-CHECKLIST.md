# Release acceptance checklist

Record actual evidence; do not infer success from synthetic/Linux tests. PR2 requires hosted macOS packaged-app updater acceptance as specified below.

## PR1 / Linux

- [ ] `cargo fmt --all -- --check` passes.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo test --all-targets` passes, including synthetic CONNECT/TLS forwarding and capture-event test.
- [ ] Confirm API/CLI behavior, bearer auth, loopback-only binds, and SQLite initialization.
- [ ] Confirm proxy streaming framing and allowlist fixes have landed and have focused tests; current work remains in progress.
- [ ] Gitleaks/dependency checks pass where run; record actual CI evidence.

## PR2 desktop updater — hosted macos-15 evidence gate (PENDING)

- [ ] Link hosted `macos-15` workflow run and retained screenshots/artifacts here: **PENDING; run not yet recorded**.
- [ ] Run the real packaged Tauri app: install 0.1.0, discover and install 0.1.1, confirm resulting app version.
- [ ] Capture native UI screenshots of the update flow/outcome.
- [ ] Demonstrate rejection of a tampered update and a wrong-key-signed update; retain observable rejection evidence.
- [ ] Exercise packaged-app WKWebView via capture proxy against synthetic HTTPS provider; show response capture and ingestion.
- [ ] Show authentication/sign-in host tunnel remains undecrypted, distinct from intercepted capture host.
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
