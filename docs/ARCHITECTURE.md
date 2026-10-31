# Architecture

Chat Hub is a local-first conversation library, not a replacement chat frontend. Its Rust service and Tauri desktop app share the local SQLite library. The current implementation includes normalized conversation/message storage, export import, conversation graph and selected-branch navigation, body full-text search, and the desktop library UI.

## Current components

- Rust CLI/service commands: `status`, `search`, `read`, and `serve`.
- SQLite library and migrations, with normalized conversations/messages, preserved branch structure, import support, and body full-text search.
- Local Axum JSON endpoints with bearer-protected health, search, and conversation-read routes.
- Tauri desktop application using the Rust service bridge and platform app-data directory by default.
- Native library backup/export commands that reuse consistent SQLite backup and streamed JSON export APIs. The CLI can restore a SQLite backup into an empty database and rebuild search indexes; JSON export is not a restore format. Restore is currently a developer/operator CLI workflow, and native OS Save dialog/file acceptance is not verified.
- Selective CONNECT TLS proxy for `chatgpt.com` and subdomains, excluding recognized authentication/sign-in hosts; other hosts tunnel without decryption. A per-install CA is created/persisted but is not installed as trusted automatically.
- Snapshot capture exists as a disabled-by-default path. It does not establish provider traffic compatibility, real account capture, or accepted provider ingestion.

## Verification and boundaries

Hosted PR23 debug-native run [41110436605](https://github.com/iarnvidr/chat-hub/actions/runs/41110436605), source `6b7f1f9a41daf4e9894e4ab946acb1639fa48c9d`, verified official fixture import, library UI body, body full-text search, selection, alternate-branch detail, and persistence in the same default OS-account app-data directory across same-process use, restart, and closure. It does not verify an alternate data-directory override, production packaging, public DMG installation, real provider traffic, or native Save dialog/file acceptance. Raw native IPC health status was not retained separately.

Linux proxy checks use synthetic traffic and do not establish real-provider behavior. The API uses plaintext HTTP with bearer authentication on loopback; this is a trusted-local-only boundary, not remote-network security or isolation from other local processes/users. CA trust installation/removal remains an explicit user/security concern.

The [shared service and normalization contract](SERVICE-NORMALIZATION-CONTRACT.md) retains the broader proposed requirements for account scoping, observation reconciliation, queueing, lifecycle, and cross-entrypoint acceptance. Implemented library features do not imply every item in that contract is complete. For exact release and platform evidence, see [updater status](UPDATES.md), [known issues](../KNOWN_ISSUES.md), and the [release checklist](../RELEASE-CHECKLIST.md).

This design is informed by [Muninn](https://github.com/iarnvidr/muninn), an Apache-2.0-licensed project with a local service and capture pipeline. See [NOTICE](../NOTICE) for attribution; this is not a claim of feature parity.
