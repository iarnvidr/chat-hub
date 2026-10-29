# Actual macOS updater smoke

Opt-in debug-only integration harness. It drives the native app and real registered Muninn updater commands through the existing chrome-webview E2E `eval` bridge (`js` containing `return await Muninn.invoke(...)`); it does not call an invented Tauri plugin command. It tests signed bundle installation/restart and signature-tamper rejection, not a mock updater. Requires Apple Silicon macOS, Rust/Tauri prerequisites, `npx` network access for pinned `@tauri-apps/cli@2.10.1`, and a debug app bundle.

Run from this worktree:

```sh
MUNINN_UPDATER_SMOKE_APP=/absolute/path/Muninn.app build/updater-smoke/run-macos.sh
```

The runner creates a private temporary key, local feed/archive and isolated HOME/E2E directories. Set `MUNINN_UPDATER_SMOKE_KEEP=1` to retain the workspace. Key generation and signing use the confirmed 2.10.1 CLI interface (`signer generate --ci --write-keys`; `signer sign --private-key-path ...`). No production key/feed is read. Feed is loopback HTTP only and the updater public key/endpoint are test overrides.

The backend test hook is a separate implementation dependency: `MUNINN_UPDATER_SMOKE_ENDPOINT`, `MUNINN_UPDATER_SMOKE_PUBKEY`, and `MUNINN_UPDATER_SMOKE_CURRENT_VERSION` must be honored only for debug builds when both `MUNINN_NATIVE_UI_SMOKE=1` and `--native-ui-smoke --updater-smoke` are supplied. The driver intentionally does not supply a Tauri plugin endpoint command; it uses `updater_check`, `updater_status`, `updater_download`, and `updater_install` app APIs. Without the corresponding native hook, the runner must fail rather than report native success.

The runner copies the supplied debug app into isolated healthy and tamper targets before either test. The healthy target is installed and must itself relaunch; no manual relaunch fallback is allowed. Post-relaunch evidence requires native process identity from `state({})`: a positive PID different from pre-install and executable path equal to the resolved installed bundle executable. Settings and unique tab marker/value are read back and compared after that process is confirmed. The tamper test uses a separate copy with the still-new candidate archive altered while retaining its original signature, and validates the structured failure reason `signature_verification_failed`. Evidence JSON is boolean-only and contains no keys, signatures, endpoints or user data. `verify_artifacts.py` and `verify_evidence.py` validate local fixture/evidence structure only; they are not native integration evidence. The tamper case must offer a version newer than the installed version, alter archive bytes while retaining the genuine original signature, then verify failure, unchanged installed version and continued app usability.
