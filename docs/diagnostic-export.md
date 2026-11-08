# Diagnostic export (initial implementation)

Settings → **Export diagnostics…** writes a metadata-only JSON file through
the native macOS Save dialog. The export is intentionally different from
**Save library backup** and **Export library as JSON**, which can contain
conversation content. Nothing is automatically uploaded to a server.

Schema version 1 includes the installed application version and OS/architecture,
pending provider-ID counts and session-only draft counts, a flag indicating
whether the app has successfully persisted pending metadata, committed capture
counters observed during the current service session, and the native history
sync's bounded progress counters, state, categorized errors and cooldown.

The app does **not** export the underlying pending list or the ChatGPT
conversation/account IDs, titles, messages, bodies, cookies, authentication
headers, bearer tokens, provider error text or local paths. The native command
builds the schema from allowlisted fields; a caller cannot supply arbitrary
JSON for the file. Export refuses to overwrite an existing destination.

## Interpreting pending entries

A sidebar item marked **Not saved to library yet** is a recovery placeholder,
not a transcript. The pending metadata is stored in WebView localStorage
*after* a provider conversation ID is available; it is not a replacement
for committed SQLite capture. The app now reports a metadata storage failure
instead of silently claiming success. Keep the current session open when the
warning is visible.

Live capture remains best-effort: in-memory queuing and proxy response handling
can drop work before Store commit. A history-sync HTTP 429 does not imply a
capture/SQLite failure. History backfill respects ChatGPT retry deadlines;
forcing additional requests risks a longer cooldown.

## Known limitations / next steps

This is a first diagnostic and visibility slice of issue #41, **not a proof
that all conversations are now saved**. The export does not yet include
durable per-conversation capture receipts, a recovery journal of completed
SSE payloads, or a device-local bounded log bundle. It does not include the
stable/merge-preview build commit; until injected by the native packaging
pipeline the build channel is explicitly `unspecified`.

Acceptance still requires actual logged-in macOS session testing: new chat
→ prompt and reply commit → app restart → exactly one saved library entry,
including with history backfill throttled. The release naming/draft cleanup is
a distinct follow-up so a diagnostic feature PR cannot silently change the
stable updater feed.
