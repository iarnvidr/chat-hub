# Changelog

## 0.2.4 — release candidate

- History sync commits active conversations before archived-list and project enrichment requests. It acknowledges each successfully fetched detail batch after local import, so later rate limits preserve saved progress.

## 0.2.3 — release candidate

- New chats appear immediately in the sidebar, with a pending state until a saved conversation is available. Starting a chat resets incompatible sidebar filters; assigned provider IDs persist across restart and reconcile only with the same verified account.
- Captures the current ChatGPT `/backend-api/f/conversation` send route and bounded `delta_encoding: v1` streams. Completed captures preserve the prompt, response and conversation identity while removing resume-token credentials from the retained raw stream.
- History imports keep valid chats and continue after item-level failures. Bulk project metadata skips missing same-account records without creating dangling links; diagnostics retain safe status codes for import failures.
- A busy local library no longer drops a history batch: the same chunk retries within a bounded deadline. An unchanged ChatGPT pin refresh no longer invalidates the library index.

## 0.2.2 — release candidate

- Adds a No project filter across chats, search and the indexed overview.
- Replaces the full-row hover with a compact action rail, an inline project picker, pin/unpin, archive/restore, tags and copy-link actions. Keyboard and reduced-motion interactions are supported.
- Pin and archive changes use ChatGPT's signed-in session and require verified readback before updating the local library. Archived chats have a separate filter; pins sort before other chats across database pages.
- Keeps repositories, pull requests, issues, domains and files in the interactive graph, with a separate List mode. Shared indexed references now contribute to chat connections, with common domains suppressed.
- Adds scrollable graph inspectors, clickable labels, temporary side-by-side saved-chat previews and compact hover actions. Graph motion is finite and respects reduced motion.
- Replaces the tag dialog with a hover editor offering content-derived suggestions and a distinct custom-tag option.
- Selected chats and related previews read from the beginning in independently scrollable panes; long saved content loads progressively. Suggestion failures offer a retry instead of unrelated library tags.
- Project assignment and rename use anchored hover controls with no local-only option. Fixes assignment for cached chats with legacy source-ID prefixes while preserving stored identities and account checks.
- Consolidates row actions into one hover menu. Reads ChatGPT's current pins independently of history sync, reconciles complete account-bound snapshots, and displays passive pin markers on saved chats.
- No live personal-chat mutations were performed during implementation checks. The existing live history/capture acceptance limitations remain.

## 0.2.1 — release candidate

- Compact tabbed chat workspace, Chat/Codex separation, inline expandable projects, five saved color schemes, distinct speaker styling and native tab shortcuts. Cmd/Ctrl+W leaves the app open when there are no tabs.
- Local catalog for chat connections, message branches, repositories, pull requests, issues, domains, links and file references. Full-index search and source/project/tag filters keep original source references.
- Persistent local tags and explicit file attachment, verified local storage, Open/Reveal and Save copy. File references distinguish produced files, attachments and mentions; automatic ChatGPT file downloading is not included. SQLite backups include saved bytes; JSON exports include metadata only.
- Chat Hub owns its ChatGPT session and fetching/capture implementation without depending on Paseo infrastructure. Authenticated sidebar metadata restores project names. Upstream project assignment/rename commits locally only after verified readback.
- Rust CLI installation, shared desktop-library status/search/read, and a Node-free development and CI pipeline using Cargo, Bun, Python and Go.
- Known limitations: fresh history listing currently encounters HTTP 429 in the connected test account. Fresh-history success, generated/interrupted live capture, account switching, upstream project mutations and automatic generated-file retrieval remain unverified. Existing imported transcripts do not contain original branches. This release does not claim full live-chat MVP acceptance.
- macOS builds are ad-hoc signed, not notarized; downloaded-app Gatekeeper behavior and the genuine stable-feed upgrade remain manual acceptance items. Release assets must pass the exact-source protected packaging, launch, updater-signature and DMG gates before publication.

## 0.2.0 candidate — unreleased draft

- Adds the local conversation library: normalized storage, export import, conversation graph and selected-branch browsing, body full-text search, and desktop library UI.
- Includes the disabled-by-default snapshot-capture path and native library backup/JSON export commands. Backup uses SQLite's consistent backup API; JSON export is streamed and is not a restore format. Existing destinations are refused, cancellation creates no file, and only `Error::ArchivePublished` means publication succeeded without confirmed durability. Native OS Save dialog/file acceptance remains unverified.
- Accepted PR23 debug-native fixture evidence covers import, library UI body, body search, selection, and alternate-branch detail/persistence in the same default OS-account app-data directory across same-process use, restart, and closure ([run 41110436605](https://github.com/iarnvidr/chat-hub/actions/runs/41110436605), source `6b7f1f9a41daf4e9894e4ab946acb1639fa48c9d`). This does not establish public DMG installation, provider capture, or native Save acceptance.
- This candidate is not a published release. Required exact-candidate macOS package, launch, signature, and DMG gates remain pending. The genuine unmodified v0.1.1 stable-feed update to v0.2.0 is not automated and remains a protected post-publication acceptance item. Ordering and an execution feasibility plan are pending; runner OS automation/trust and WKWebView automation permissions are unverified. No waiver is assumed.
- No real provider login/capture, public owner-downloaded DMG installation, or downloaded-app Gatekeeper behavior is claimed.

## 0.1.1

- Added an early macOS Objective-C uncaught-exception handler that writes exception name, reason, and call-stack symbols to stderr and (when configured) a diagnostic evidence file.
- Fixed packaged-app startup abort: the desktop `main` was entering a Tokio runtime before Tauri setup synchronously waited on its async runtime, causing Rust's `Cannot start a runtime from within a runtime` panic. Tauri now owns the sole desktop runtime; setup performs its bootstrap waits before the event loop enters runtime callbacks. The observed failure was a Rust panic followed by process abort, not an `NSException`; the earlier Objective-C exception diagnosis was not supported by the captured stderr.
- Required the packaged app startup and authenticated localhost health smoke in macOS CI and release preparation.
- Manual DMG upgrade acceptance from v0.1.0 remains unverified. The v0.1.0 startup failure's runtime-nesting root cause is fixed in source, but packaged macOS startup and the manual DMG upgrade still require their release gates. This version bump is not a release publication.
