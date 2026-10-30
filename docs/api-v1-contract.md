# API v1 contract

**Status: normative target, not implemented or readiness evidence.** F1's additive store DTO and F2/F3's shared import receipt are coordinated targets. This document and the synthetic [fixtures](../tests/fixtures/api-v1/README.md) change no runtime behavior. The [shared normalization contract](SERVICE-NORMALIZATION-CONTRACT.md) remains applicable; this document specifies the initial single-account API projection.

## Transport and compatibility

Bind only to loopback. Every route below requires the existing `Authorization: Bearer <configured-token>` authentication, including status/version/import. Missing or invalid credentials return HTTP 401. JSON responses use `application/json`; import accepts `application/json`. Do not expose tokens, paths, or source bodies in diagnostics. Existing `/api/health`, `/api/search`, and `/api/conversations/:id` retain their existing response bodies and behavior; v1 is additive.

| Method | Route | Success |
| --- | --- | --- |
| GET | `/api/v1/status` | 200 healthy; 503 failed diagnostic body |
| GET | `/api/v1/version` | 200 version object |
| GET | `/api/v1/conversations?limit=50&cursor=…` | 200 list envelope |
| GET | `/api/v1/search?q=…&limit=50&cursor=…` | 200 search envelope |
| GET | `/api/v1/conversations/{id}?leaf_node_id=…` | 200 graph/detail envelope |
| POST | `/api/v1/imports/chatgpt` | 200 import receipt, including item failures |

Percent-encode path IDs and query values. IDs are case-sensitive opaque strings. Unknown conversation IDs return 404 `not_found`. Invalid or foreign/nonleaf `leaf_node_id` returns 400 `invalid_leaf_node_id`. Ordinary errors have `{error:{code,message}}`; safe messages are diagnostic text, not a parsing interface. Invalid parameters return 400 `invalid_request`. Service unavailability returns 503 `service_unavailable` except the specific import admission code `busy`.

## Identity, summaries, and content

Initial `account_scope` is exactly `provisional:default`; this is a single-account scope, not evidence of multi-account attribution. `provider` and string conversation `source_id` remain explicit. Conversation `id` is a stable opaque local canonical ID. Nodes use conversation-scoped source mapping IDs as `id`; their `source_id`, nullable `parent_id`, and nullable `message_id` are explicit. Every graph/selection pointer is a node ID, never a message ID. Local canonical message `id` is distinct from nullable provider message `source_id`. Resolve node IDs only within their conversation.

A summary contains exactly the fields illustrated in list fixtures: `id`, `provider`, `account_scope`, `source_id` (string), `title` (string, empty allowed), `updated_at_ms` (integer or null). Unknown update times sort after known times; ties use `id` ascending. Initial providers in these fixtures are synthetic ChatGPT exports (`provider: "chatgpt"`). No client may infer identity from title or source ID alone.

The planned F1 DTO is `{id,provider,account_scope,source_id,title,updated_at_ms,source_current_node_id,selected_node_id,selection_reason,thread_complete,nodes:[{id,source_id,parent_id,message_id}],messages:[{id,source_id,node_id,role,author,create_time,content,parts}]}`. The HTTP adapter wraps summary and selection separately, retains the full nodes/messages arrays, and adds `created_at_ms` to each message. `create_time` remains finite source seconds or null; `created_at_ms = floor(create_time * 1000)`, with null preserved. Neither value is the local observation timestamp. `updated_at_ms` is already normalized milliseconds and must not be multiplied again.

`role` and `author` retain source values (author may be structured or null); render unknown roles safely. `content` is the derived legacy plain-text string, never a raw provider content object. `parts` preserves ordered canonical lossless parts; the observation retains the full raw provider payload. Text parts use `{kind:"text",text,raw:<original string or value>}`; retaining raw is required for text too. Unsupported structured parts use `{kind:"unknown",raw:<original JSON value>}` without loss of payload. Render an unsupported-part placeholder; do not execute raw content. Markdown, fenced code, and source text remain intact. Derived display/search text is never the canonical content representation. Null-message nodes stay present and have no synthetic message.

## Detail and branch selection

The envelope is `{conversation:summary,selection:{source_current_node_id,selected_node_id,reason,thread_complete},nodes:[],messages:[],thread_node_ids:[],branches:[{leaf_node_id,selected}],library_revision}`. Arrays contain the full retained graph and all messages, including alternate branches. Node `message_id` resolves to a message in `messages`; message `node_id` points back to its node. `thread_node_ids` is the selected root-to-node ancestry, including null-message nodes. UI renders messages through these node references.

Default selection is deterministic:

1. An existing provider current-node mapping wins, even when it has no message or is not a leaf. Reason is `source_current_node`.
2. If the source pointer is missing/invalid, choose the leaf with lexicographically smallest source mapping ID (case-sensitive UTF-8 byte order). Reason is `fallback_leaf`.
3. If there is no leaf and no valid source selection, select null, use an empty thread, and reason `empty`.

Preserve the raw invalid source pointer in `source_current_node_id`; it is diagnostic source metadata and need not resolve. Other returned pointers resolve except dangling source `parent_id`, which is retained to expose partial ancestry. A leaf is a retained node with no retained child, including a null-message node. `branches` lists all leaves in source-ID order; `selected` is true only for the leaf equal to `selected_node_id` (all false when the selected node is internal).

With `leaf_node_id`, select that conversation's existing leaf for this response only and set reason `leaf_override`. Never persist the override, alter provider current-node metadata, or increment the revision. Repeating the default GET still returns source/default selection. Preserve the full graph in both views.

Traverse ancestry with cycle detection. Dangling ancestry returns the known contiguous suffix in rootward-to-selected order with `thread_complete:false`; it must not fabricate a root. Cyclic selected ancestry is safely rejected as a traversable thread: return `thread_node_ids:[]`, retain graph and selected pointer, and set `thread_complete:false`. A no-leaf cyclic graph without a valid source selection uses `empty` and `thread_complete:false`. A truly empty graph uses `empty` and `thread_complete:true`. Completeness describes the selected thread, not a promise that every provider observation was complete.

## Revisions, list pagination, and search

`library_revision` is a nonnegative integer supplied by F1. Increment it atomically with each durable transaction that changes the visible library; duplicate/idempotent ingestion and view-only selection leave it unchanged. Each response reads one consistent revision, including its records, search index, and cursor. A cursor is a revision check, not a per-client MVCC snapshot.

List/search envelopes are `{items:[],next_cursor:null|string,library_revision}`. `limit` is an integer, default 50, range 1–100; reject values outside this range. List order is `updated_at_ms DESC, id ASC` with null update times last. Use keyset pagination, never offset pagination. `next_cursor` is null exactly when no further results remain.

Cursors are opaque server tokens of at most 2048 UTF-8 bytes. Strictly parse within that bound and bind route, normalized search query (where applicable), order/version, revision, and last ordering tuple; limit may change between pages. No client decoding or persistence format is promised, and cryptographic signing is not required. Malformed, oversized, or wrong-route/query/order cursors return 400 `invalid_cursor`. A valid cursor whose revision differs from the durable current revision returns 409 `stale_cursor`; discard accumulated pages and restart without a cursor. Check cursor structure/binding before revision. No per-client snapshot retention is required.

Search uses FTS5 over titles and derived message text from **all branches**. Required `q` becomes Unicode literal terms combined with AND. Whitespace separates terms, repeated whitespace is ignored, and punctuation is passed as literal text to the tokenizer; never interpret user input as FTS operators, column filters, prefix/wildcard syntax, or expressions. Quote/escape terms before constructing the internal FTS query. Apply the index's Unicode tokenizer consistently to query and indexed text. A missing/blank query or one yielding no searchable tokens returns 400 `invalid_request`. Search order is finite rank ascending, then canonical conversation ID ascending, then node ID ascending (null title-hit node first). Scores are comparable only within this query/revision/index configuration; no probability semantics.

Each item is `{conversation:summary,node_id,message_id,leaf_node_id,snippet,score,selected_branch}`. Emit one hit per matching message plus at most one title hit per conversation. A title hit has all three IDs null and `selected_branch:false`. A message hit has its scoped node ID and canonical message ID, both nonnull. `selected_branch` is true exactly when the hit node belongs to the default selected ancestry. `snippet` is plain text only: no HTML, highlight spans, or Markdown decoration introduced by the API. Source punctuation can remain; clients render it as text.

For a message hit, choose `leaf_node_id` as the current selected leaf if it contains the hit in its ancestry; otherwise choose the lexicographically smallest source-ID descendant leaf containing the hit. Open with `?leaf_node_id=<leaf>` and use the hit `node_id` as a client-side anchor. For malformed graph hits with no safely reachable descendant leaf, omit that message from searchable results until repair; never emit a message hit with a null leaf. A default selected internal node is not a selected leaf, so use the descendant-leaf fallback. This makes alternate-branch hits navigable without mutating selection.

## Status and version

Status reports `status:"healthy"|"failed"`, `service:{status:"ready"|"failed",code:null|string}`, and independently `database:{status:"healthy"|"failed",code:null|string}`. HTTP 200/healthy requires actual required service initialization and a healthy DB. A successful DB probe alone cannot imply ready. HTTP 503/failed can report a healthy database alongside a failed service; DB failures must remain distinct. Version returns `binary_version` (string), `api_version:"v1"`, and `schema_version` (integer). Fixture version values are illustrative, not a claim about the current binary/schema.

## ChatGPT JSON import and desktop adapter

`POST /api/v1/imports/chatgpt` accepts the raw UTF-8 `conversations.json` export array or `{ "conversations": [...] }`. Do not accept base64 or file-path wrappers. Maximum request body is 64 MiB (67,108,864 UTF-8 bytes), at most 10,000 entries. Empty arrays succeed with zero counts. Check actual streamed bytes even without Content-Length. Initial scope is JSON only; ZIP support is deferred, and users must extract `conversations.json` first. Authentication matches read routes.

Malformed JSON, invalid UTF-8, invalid top-level shape, or too many entries returns 400 `invalid_request`, with no entries ingested. Excess body bytes return 413 `payload_too_large`; wrong content type returns 415 `unsupported_media_type`. Validate the global envelope and limits before item commits. Per-item validation/store failures instead return 200 with the exact shared Rust F2/F3 receipt:

`{imported,updated,unchanged,failed,errors:[{item_index,code,message}],errors_truncated}`

All four counts are nonnegative integers whose sum equals the input entry count. `item_index` is zero-based. Each conversation has its own durable transaction; count imported/updated only after commit, and unchanged only after the durable existing state is established. Rolled-back items count failed. Duplicate reimport is idempotent and counts unchanged when no stored change occurs. `errors` contains the first 100 failed items in input order with safe code/message only; `failed` always counts all failed entries and `errors_truncated` is true exactly when additional error entries were omitted. No paths, tokens, source bodies, or raw database diagnostics in errors.

Admit one import at a time; another import returns 503 `busy` before item processing. A client timeout/disconnect does not imply rollback: earlier item transactions may already be durable. Reimporting the same input safely resolves uncertainty through idempotence. The receipt deliberately has no library_revision field; clients refresh list/search after import and honor stale cursors.

The native picker adapter is advisory until F2 implements it and wraps the identical receipt: cancellation is `{cancelled:true,result:null}`; completion is `{cancelled:false,result:<receipt>}`. Transport/admission failures are adapter errors, not invented success receipts. `import-result.json` illustrates the exact HTTP receipt; `import-cancelled.json` and `import-picker-result.json` illustrate the native wrappers.

## Acceptance boundary

Fixtures are entirely synthetic API targets. Validate JSON parsing, paging order/revision, node/message cross-references, branch selection, timestamps, snippet provenance, and import counts. They do not prove endpoint implementation, FTS ranking, durable transaction behavior, authentication, or service readiness. Linux iteration is sufficient for this docs change; do not manually trigger Mac CI. macOS validation belongs to release work. Merge requires lead review.
