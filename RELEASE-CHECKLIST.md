# Release acceptance checklist

This repository contains a v0.2.0 release candidate draft. Version metadata and documentation do not constitute a release, prove acceptance, create a tag, or authorize publication. Record evidence for the exact source and version; do not infer success from synthetic/Linux tests.

## Implemented library scope and existing evidence

- [x] Normalized local conversation store, export import, conversation graph/selected-branch support, body full-text search, and desktop library UI exist in source.
- [x] Hosted PR23 debug-native run [41110436605](https://github.com/iarnvidr/chat-hub/actions/runs/41110436605), source `6b7f1f9a41daf4e9894e4ab946acb1639fa48c9d`, verified official fixture import, library UI body, body search, selection, alternate-branch detail, and persistence in the same default OS-account app-data directory across same-process use, restart, and closure.
- [ ] Repeat required checks against the exact v0.2.0 candidate head; the earlier PR23 result is feature evidence, not candidate-version release evidence.
- [ ] Native OS Save dialog/file acceptance; PR24 source and Linux checks do not prove the native dialog/file path.
- [ ] Alternate home/data-directory override, public owner-downloaded DMG installation, or native Save acceptance; none is established by run 41110436605.

Snapshot capture remains disabled by default. Real provider login, provider traffic capture/ingestion, and provider compatibility are not established by synthetic fixtures or the PR23 library smoke.

## Required protected hosted macos-15 release workflow gates

The current protected workflow requires these checks before its prepare job can produce release artifacts. They must pass for the exact versioned candidate:

- [ ] Production and native-smoke Cargo compilation succeeds.
- [ ] Build and verify the ad-hoc-signed app and corresponding-source manifest.
- [ ] Required positive persisted-library smoke succeeds in the verified debug bundle.
- [ ] Required packaged-app startup and authenticated health smoke succeeds.
- [ ] Required native launch/UI, health, update-status, reopen, and shutdown smoke succeeds.
- [ ] Create, mount, and verify the DMG.
- [ ] Sign the updater archive and independently verify it against the pinned public key.
- [ ] Validate the generated `latest.json` and retained release evidence.

These checks are required by the workflow, not nonblocking diagnostics. The provider proxy/capture smoke is advisory and remains unverified. The optional updater UI smoke is diagnostic and its acceptance remains unproven.

## Genuine stable-feed update — protected post-publication ordering

- [ ] Preserve the genuine unmodified v0.1.1 stable-feed-to-v0.2.0 update as a pending post-publication acceptance item. The baseline app can only query GitHub's `releases/latest`; it cannot discover an unpublished candidate.
- [ ] Before publication, the existing protected approval package must explicitly record the unavoidable ordering decision and a reviewed feasibility plan for the post-publication run. The production baseline has no debug IPC; runner OS automation permissions/trust and WKWebView automation allowlisting are unverified. Do not assume approval clicks alone make this test runnable. No waiver is assumed.
- [ ] The current release workflow has no literal genuine-baseline integration step. A standalone no-prompt AX preflight is available under `build/genuine-updater-preflight`, but has not run on macOS and is not workflow-integrated; it checks only baseline identity, health, AX trust, and the actionable “Check for updates” control. After feasibility review and authorized publication makes v0.2.0 available, run the genuine baseline update and retain observable install/version evidence; also retain tampered and wrong-key rejection evidence for the packaged flow. Keep acceptance pending until feasibility and evidence are reviewed.

The controlled updater fixture verifier accepted a valid 0.1.0→0.1.1 fixture and rejected a tampered payload with `signature_verification_failed`. This is not genuine stable-feed update evidence and must not check the post-publication item above.

## Remaining manual acceptance

- [ ] Verify native OS Save dialog/file acceptance on an authorized Mac, including no-clobber, cancellation, and durability-warning behavior.
- [ ] Verify public owner-downloaded DMG installation and downloaded-app quarantine/Gatekeeper behavior.
- [ ] Verify real provider login/account behavior and provider capture/ingestion only with an authorized account. Do not treat advisory capture smoke or synthetic Linux tests as provider acceptance.
- [ ] Retain raw native IPC health separately only if a future evidence run safely produces it; PR23 did not retain a separate raw-health artifact.
- [ ] Verify CA trust installation/removal and review local data/access controls.

## Release hygiene

- [ ] Review the draft v0.2.0 release notes in [CHANGELOG.md](CHANGELOG.md) and record any additional limitations.
- [ ] Confirm the exact source/version, corresponding-source manifest, updater signature, app package, DMG, and test evidence before authorizing publication.
- [ ] Preserve the two-stage release workflow: prepare only from exact protected main and an existing owner-created matching tag in protected `chat-hub-release` with read-only contents; separate publish requires successful prepare, explicit `publish=true`, and the same protected environment with contents-write. `publish` defaults to false; the workflow never creates a tag.
- [ ] Keep credentials, private conversations, tokens, and signing private material out of artifacts and source control. Only the updater public key belongs in source.
