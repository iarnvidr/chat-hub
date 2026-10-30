# Optional ChatGPT snapshot capture

Capture is disabled by default. `CHAT_HUB_CAPTURE_ENABLED=true` (or `1`)
opts in at service startup; `false` and `0` disable it. Other values are rejected.
This is the effective startup setting, not a live toggle. Desktop defaults to
false until its owner supplies a real user choice. JSON imports are independent.

The shared service captures successful ChatGPT GET detail snapshots at
`/backend-api/conversation/<id>`. It does not reconstruct streamed turns. JSON
responses with no content-type header remain accepted for existing provider
compatibility; explicit non-JSON types are ineligible. Compressed snapshots are
forwarded but capture reports unsupported encoding. Provider forwarding stays
independent from capture acceptance and Store failures.

The typed live path retains immutable `provisional:default` request-entry scope,
time and correlation through the sole service writer. Each snapshot has a
2 MiB limit, the typed path has a 16 MiB retained-body reservation limit and a
32-item writer queue. Permits cover accumulation, queue and writer processing.
These bounds concern retained source bodies, not all parser/graph allocations
or total process memory.
Derived JSON/normalized graph allocations are bounded by the 2 MiB input and
single writer, but have additional parsing overhead outside the raw-copy budget.
The compatibility `Sender<CaptureEvent>` has its
separate caller-selected item-count capacity; its externally supplied body
allocations are outside the live byte budget. Oversized legacy events are
ignored at writer entry. Disabled mode never archives legacy submissions.

Staging is in memory and best-effort. Only an atomic committed Store outcome is
durable. A crash can lose uncommitted work. The authenticated v1 status adds a
`capture` field for shared-service routers; legacy routers keep their bodies.
Capture diagnostics are distinct from service readiness and DB health.
Contended capture state returns `status: unavailable`, never invented zeros.

`started = active + mutually exclusive terminal outcomes`. Imported, updated
and unchanged outcomes count only after commit. Admitted/completed responses
are stage counters; queued/writer-in-flight are gauges. Lost observer messages
are independent diagnostics, not an additional dropped attempt. Pending writer
transactions remain active until resolved, including after shutdown timeout.
Safe last-error codes never contain bodies, paths, credentials or SQL errors.

All usable `conversation_id` and export-style `id` values must match the source
request path. Present non-string identity shapes are rejected; null and empty
strings are missing, consistent with F1. When both identities are missing, the
path is supplied only to the shared normalizer; stored raw provenance remains
the original provider JSON without invented fields.

Tests cover default-off and opt-in synthetic TLS forwarding, normalized v1 read,
raw payload fidelity and reopen persistence; malformed/oversized/unsupported and
SQLite-abort responses preserve traffic. Deterministic budget/queue/drop/panic
tests assert conservation. The actual service supervisor drains live work after
writer panic while the handle remains held. A deadline test holds the actual
writer before commit and verifies it remains active until resolution. Fresh full
regressions, Linux CI and lead review remain required before merge. No native or
macOS validation is claimed.
