# Desktop updater and release channels

## Channel policy

The desktop app's stable updater feed is GitHub's `releases/latest` endpoint. Only published, non-prerelease releases participate in that feed. Preview updates are opt-in and come from published prereleases; preview builds are not stable releases and release notes must identify them as previews. A pinned repository-owned updater public key is mandatory for signature verification in either channel. An ad-hoc code signature is not a substitute for the updater signature.

Ad-hoc builds are not notarized. Release notes must say so and warn that macOS may block first launch through Gatekeeper/quarantine. Do not imply that publishing a prerelease activates the stable feed.

## Required updater acceptance evidence (PR2)

Acceptance requires a hosted `macos-15` run against the real packaged Tauri app, not just helper/unit tests or an artifact-only simulation. The run must:

1. Install/run packaged version 0.1.0, discover version 0.1.1 through the intended feed, download and install it, and show the resulting version in the app.
2. Show native app UI screenshots of the update flow and outcome.
3. Reject a tampered update payload and reject a payload signed with a key other than the pinned public key. Record observable rejection and retain the associated logs/screenshots.
4. Exercise the packaged app's WKWebView through the capture proxy against a synthetic HTTPS provider, and show successful response capture and ingestion.
5. Verify the configured authentication/sign-in host is tunneled without TLS decryption while the capture host is intercepted; retain proxy evidence and screenshots.

**Status: PENDING.** No hosted `macos-15` run has yet been recorded. Add the actual workflow run URL and screenshot/artifact links here only after execution. Do not mark the gate passed based on local/Linux tests. Manual residual acceptance is limited to real ChatGPT login/account behavior and downloaded-app quarantine/Gatekeeper behavior.

## Protected release preparation

Release publication is a protected, manual action. Prepare with `publish=false`; review the exact source/version, updater manifest and signature verification, app package, release notes and evidence before authorizing publication. Do not publish or present a release as accepted until the required review and acceptance gate are complete. Keep signing private material out of the repository; only the public verification key belongs in source control.
