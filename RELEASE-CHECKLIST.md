# Release acceptance checklist

PR1 is an infrastructure milestone, not acceptance of the complete conversation-hub MVP. Record actual evidence; do not infer provider or macOS support from synthetic Linux tests.

## PR1 / Linux

- [ ] `cargo fmt --all -- --check` passes.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo test --all-targets` passes, including synthetic CONNECT/TLS forwarding and capture-event test.
- [ ] Confirm API/CLI behavior, bearer auth, loopback-only binds, and SQLite initialization.
- [ ] Confirm proxy streaming framing and allowlist fixes have landed and have focused tests; current work remains in progress.
- [ ] Gitleaks/dependency checks pass where run; record actual CI evidence.

## Not yet verified / later acceptance

- [ ] Capture event ingestion into the database and real conversation extraction.
- [ ] ChatGPT data export import, complete conversation/message model, and full-text search.
- [ ] Service-hosted UI, browser tests, and UI evidence.
- [ ] Live ChatGPT capture with an authorized account. No real-provider behavior is implied by synthetic Linux tests.
- [ ] macOS build and physical Mac/WKWebView behavior; record OS/version, build and result when performed.
- [ ] CA trust installation/removal and security review for local data and access controls.

## Release hygiene

- [ ] Keep documentation aligned with the implemented PR scope and explicit limitations.
- [ ] Confirm no credentials, private conversations, or private infrastructure details are present.
- [ ] Link passing CI and test evidence; retain untested items above until verified.
