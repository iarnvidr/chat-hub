# Implementation plan

Ordered contract for follow-on work; do not broaden a bounded repair into later stages.

1. **Launch/TLS/UI bridge repair (current Step 1):** preserve the verified PR #6 startup fix; explicitly select Rustls crypto providers in production; exercise a fresh process where intercepted CONNECT TLS is the first Rustls use; repair the local app UI invoke bridge and test its privilege boundary. No release/tag/publication.
2. **Identity and environment contract:** establish canonical product, binary, bundle, provider, and filesystem identities; resolve migration and compatibility implications before parallel work.
3. **Service lifecycle and local API hardening:** startup/readiness, authenticated loopback API, shutdown, and ownership semantics.
4. **Capture/proxy correctness:** protocol boundaries, interception scope, bounded-memory behavior, and failure modes; keep streaming changes separately scoped.
5. **Desktop UX and platform integration:** local UI, permissions/capabilities, macOS trust and lifecycle behavior.
6. **Release and update safety:** reproducible artifacts, signed update validation, rollback, and release evidence.
7. **End-to-end acceptance:** platform smoke tests, security review, documentation and operator sign-off before any explicitly authorized release.

Each stage requires its own bounded task and validation evidence. Stage 2 identity must precede parallel tracks.
