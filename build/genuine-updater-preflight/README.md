# Genuine v0.1.1 updater AX preflight

This helper checks whether a running, unchanged v0.1.1 app exposes an actionable “Check for updates” accessibility control before the candidate is published. It does not query the updater feed, press the control, install an update, or prove the “Update & relaunch” flow. A normal app launch automatically checks the app's real stable feed after 1.2 seconds, so launch itself is not network-free.

The preflight is designed for a macOS runner after the candidate's persisted-library gate and all candidate app smokes have completed, all owned app processes/listeners have closed, and the candidate-owned default data directory has been safely isolated. Seed the old-schema fixture through the pinned v0.1.1 public Store API, then launch the unmodified baseline app in the same real OS account and default app-data path. Do not set `HOME`, `CHAT_HUB_*`, or `MUNINN_E2E_DATA_DIR` to redirect the baseline. The source contract is pinned at `677b31d01ffdd6607c4dfa291dc8bd62e87306a4`; see `/tmp/v011-baseline-interface-contract.md` during implementation review.

The caller must first download the allowlisted v0.1.1 DMG and independently obtain its expected SHA-256 plus the expected baseline executable SHA-256 from reviewed release provenance. Install a writable owned app copy from that exact image without modifying or re-signing it. The script checks the supplied DMG and running executable against those separate pinned digests; the caller remains responsible for establishing that the app copy came from the verified image. `codesign --verify` checks bundle integrity only; an ad-hoc signature does not establish publisher identity or notarization. Then run:

```sh
python3 build/genuine-updater-preflight/preflight.py \
  --dmg "$RUNNER_TEMP/Chat_Hub_0.1.1_macos-arm64.dmg" \
  --expected-dmg-sha256 "$V011_DMG_SHA256" \
  --app "$RUNNER_TEMP/v011-owned/Chat Hub.app" \
  --expected-executable-sha256 "$V011_EXECUTABLE_SHA256"
```

The script compares the downloaded image bytes with the supplied pinned digest, verifies the app bundle's code-signature integrity, reads only the actual account's `~/Library/Application Support/io.github.iarnvidr.chat-hub/service.json` and `service.token`, and requires the old four-field discovery record and v0.1.1 version. It verifies that the recorded PID owns the exact loopback API listener and that authenticated `/api/health` returns the old `{"status":"ok"}` contract. A compiled external Swift helper then independently checks that the same PID has the expected bundle, version, executable digest and app path; it calls `AXIsProcessTrustedWithOptions` with prompting disabled and finds exactly one enabled “Check for updates” button that supports `AXPress`.

Output is a bounded JSON readiness record or a fixed blocked reason. It contains no app paths, raw process command lines, token, database, CA material, or arbitrary error text. Missing trust, ambiguous controls, stale/foreign PID ownership, unhealthy API, digest or signature mismatch all block. The helper does not request TCC permissions, alter TCC, use debug IPC, invoke Apple Events/System Events, or press the AX button. A `ready` result establishes only prepublication accessibility feasibility for that run. It is not evidence of a published update, offer, install, relaunch, version transition, migration, or final process/listener closure.

The script and Swift helper have not been run on macOS in this change. This tool is not wired into `.github/workflows/macos.yml` or `release.yml`; no workflow integration or runner permission feasibility is claimed. The genuine stable-feed update remains pending after publication and requires separate reviewed automation and execution.
