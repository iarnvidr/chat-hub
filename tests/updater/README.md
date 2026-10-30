# Updater smoke tests

This directory contains no native updater smoke implementation. The repository's separate debug-only native updater harness is `build/updater-smoke/run-macos.sh`; it prepares disposable 0.1.0/0.1.1 fixtures and is not genuine stable-feed acceptance. Do not substitute protocol fixtures or copied Muninn tests for packaged genuine-baseline evidence. The production plugin remains configured to the stable endpoint and pinned key. See [updater and release status](../../docs/UPDATES.md) for verified scope and pending acceptance.
