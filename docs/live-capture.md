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
or total process memory. The compatibility `Sender<CaptureEvent>` has its
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

Implementation checkpoint: default-off and opt-in synthetic TLS forwarding,
normalized v1 read and reopen persistence are tested. Detailed deterministic
failure, budget, panic and shutdown conservation proofs remain required before
this work is considered complete or merge-ready. No native/macOS validation is
claimed.
