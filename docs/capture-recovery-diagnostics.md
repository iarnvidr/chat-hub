# Local capture recovery, history retry and diagnostics

These changes address issue #41's application bugs/features. Release naming and
obsolete draft handling are separate in PR #42. This branch does not change
release workflows or duplicate PR #37's saved-identity projection.

## What is actually saved

A pending sidebar row is metadata, not a transcript. Its provider/account/source
identity is retained when a navigation event temporarily has no account scope.
An explicit different account cannot claim that row. Any saved row with the exact
provider/account/source identity can reconcile it, including duplicate physical
copies. Storage failures are reported instead of silently treated as persistence;
corrupt stored metadata is preserved instead of overwritten with an empty list.

Completed, normalized live captures now pass through a private recovery journal
before the Store commit. Successful database commits remain the authority for the
capture counters. A failed database write retains the normalized payload. Replay
runs at desktop startup before the service writer starts, and before subsequent
live writes, without requesting remote history. Failed or corrupt records remain
available for diagnosis rather than being erased.

The active database generation has a 64 MiB retained-byte limit, 512 record limit,
and 8 MiB per-record limit. Files are atomically published after fsync and use
0600 permissions inside 0700 directories on Unix. The journal contains private
conversation data, just like the database; it is NEVER included in diagnostics.
A replacement database file gets a different generation, so an old library's
pending captures are not automatically replayed into the replacement. Retired
generations are retained for explicit review, not silently deleted.

This is **not** an end-to-end streaming write-ahead log. A stream interrupted or
dropped before normalization has no complete journal entry. Disk-full failures
before journal publication also cannot promise durability. The new status details
show confirmed database outcomes, active attempts and failed/dropped attempts
separately. Real signed-in capture remains part of native acceptance below.

Replay retries failed writes on startup or the next live capture; this change does
not add a second background SQLite writer. A corrupt entry or persistent database
error can still require intervention. Journal retention/capacity is per database
generation, not an unbounded silent eviction of unsaved records.

## History retry

Provider Retry-After remains authoritative. Exponential backoff is applied once
per attempt rather than repeatedly on status polls. The remaining automatic retry
budget survives restart, and an old completed receipt cannot reset that budget.
Explicit manual resume resets the budget only after the active cooldown expires.
Pin refresh respects the same known cooldown and imports a new native provider
cooldown immediately when a pin request fails. Already fetched library data is
not cleared by these UI retry changes.

## Exporting a diagnostic report

Use **File → Export diagnostics…** in the native application menu. Settings shows
where to find it and displays capture/pending-metadata warnings. The native Save
dialog chooses the output path; no new provider-accessible IPC or arbitrary
webview file-writing capability is introduced.

The atomic JSON report contains build version/source identity when available,
platform, service readiness, capture counters, current recovery-journal counts,
startup recovery outcome and history progress/cooldown flags. It is metadata-only
by construction: a strict allowlist drops unknown fields, free-text errors,
conversation text/titles/IDs, account identifiers, credentials, paths and raw logs.
It does not read the journal bodies, copy SQLite/WAL files, or perform a full
SQLite integrity scan. Browser-only sidebar metadata is explicitly marked as not
collected; do not interpret that field as zero pending chats. This is a support
snapshot, not a library backup or a full diagnostic ZIP.

The source-commit field is null when the build does not embed it. The separate
release-identity PR provides that build-time value without making this PR depend
on it. A native build/release and a real logged-in session are still needed to
validate the final installed-app behavior.

## Validation and native acceptance

Focused JavaScript tests cover account attribution, duplicate saved matches,
restart hydration, unavailable/corrupt storage, persistent retry budgets,
non-sliding cooldowns, pin cooldown sharing and truthful status descriptions.
Rust tests cover journal replay after failed commits/restart, repeat replay,
corrupt/oversized entries, replacement-database isolation, private permissions,
symlink rejection and metadata-only report serialization/atomic publication.

Before merge/release, run the normal Dagger and Apple Silicon checks, then:

1. Create a signed-in chat, submit a prompt, receive a reply, confirm a database
   outcome, restart, and verify the same saved chat appears once.
2. Simulate history HTTP 429 while creating another chat; verify local capture
   still commits independently and the retry deadline does not slide on polls.
3. Inject a failed Store commit after journal publication; restart and verify
   recovery into the original account scope. Interrupted-before-normalization
   streams must report failure, not pretend recovery is possible.
4. Export from the native menu, cancel once and save once, inspect valid JSON and
   verify that no transcript, identifier, credential, sensitive path or raw log is
   present. Check that a write failure does not affect the library.

The current session ran the focused JS and Python wiring tests. Rust compilation,
Rust behavioral tests and the native signed-in smoke were not run locally; hosted
checks and native acceptance remain required. No installed app or release was
changed by opening the PR.
