# Updater smoke tests

No native updater smoke test is currently runnable: the Tauri shell has no smoke bridge for synthetic feed/key overrides, deterministic state, restart identity, or signature-rejection reporting. See `build/UPDATER-NATIVE-SMOKE-CONTRACT.md`. Do not substitute protocol fixtures or copied Muninn tests for Chat Hub native integration evidence. The existing plugin install path remains configured to the production endpoint/key.
