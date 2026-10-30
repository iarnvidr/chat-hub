# F1 conversation store: implemented contract

This describes the implemented SQLite store and capture adapter, not a complete provider-sync or HTTP product contract. The schema currently migrates to version 6. `Store::open(path)` opens a file and applies pending migrations; `Store::in_memory()` creates the same schema in memory. `health()`, `schema_version()`, and `library_revision()` provide basic status. Store operations return the crate `Result` type.

## Schema and migration behavior

Migrations are ordered and each version's schema/data changes and version record are committed in a SQLite transaction. Version 1 establishes legacy `conversations` and (where needed) `messages`; v2 creates the legacy FTS table; v3 adds provider/account/source identity, observations, graph nodes, and normalized FTS, and reconstructs graph rows from legacy messages; v4 adds normalized timestamps/message IDs; v5 adds library revision and snapshot-search FTS indexes; v6 adds `last_source_updated_ms`. Migration code uses additive column/table/index changes rather than replacing the legacy conversation/message tables. Foreign keys are enabled on the store connection.

The checked-in v1 and v2 SQL fixtures are representative historical schema fixtures, not captured production databases. Migration tests exercise these fixtures, reopen/idempotence, foreign-key checks, and an injected v3 failure followed by retry. They do not establish compatibility with every historical or real-world database.

Legacy reconstruction can recover only what the old flattened `messages` table contains: message IDs, conversation/parent IDs, role, and flattened content. It cannot recreate original provider payload, original node identity when distinct from message identity, alternate branches, account attribution, or typed/raw content parts. Newly ingested observations retain their raw JSON payload in `observations.raw_payload`; this does not retroactively recover payload for legacy rows.

## Observation ingestion and capture

`Observation` carries `provider`, `account_scope`, `source_id`, title/optional URL/current node, graph nodes, raw payload, source time, observation time in milliseconds, parser label, and completeness label. A graph node carries provider node/message IDs, parent, role, optional author/create-time JSON, and an ordered vector of JSON parts. Ingestion rejects blank provider/account/source/completeness values and empty or duplicate node IDs within one observation. Identity is unique on `(provider, account_scope, source_id)`; the local conversation ID is deterministic when available, with collision allocation as needed. Graph node IDs are scoped to their conversation.

`Store::ingest_capture` is a narrow adapter: `parse_event` accepts successful (2xx) `chatgpt.com` responses at `/backend-api/conversation/{id}` with a JSON object containing a mapping. It fills a missing conversation ID from the path. The adapter labels provider `chatgpt`, account scope `provisional:default`, parser `chatgpt-detail-v1`, and completeness `complete`; it derives graph nodes and stores the parsed document as raw payload. It does not establish authenticated account identity or general provider support. The literal scope `provisional:default` is a placeholder, not proof that observations belong to one account.

Each accepted call records observation provenance, including raw payload, even when the source snapshot is stale or causes no library-content change. If numeric `raw_payload.update_time` is present, it is converted to milliseconds and an incoming timestamp less than or equal to the stored timestamp is rejected for content/selection updates: the first accepted observation wins equal-timestamp conflicts. If the source timestamp is absent, the observation is accepted without this stale comparison. Accepted graph updates upsert nodes; this is not complete sync reconciliation (notably, omitted nodes are not deleted). A content no-op, including a later observation that only changes provenance or an omitted URL that leaves the existing URL intact, does not increment library revision. Revision increments when accepted title/URL/current-node or canonical graph state changes. Stale observations do not change library content/revision.

These rules are provisional ordering behavior, not a full synchronization protocol: completeness metadata is recorded but not used to reconcile snapshots; there is no deletion reconciliation, multi-account identity acquisition, or provider-wide conflict policy.

## Read APIs and normalized selection

`read(id)` first looks for the exact local conversation ID, then tries `source_id` only if it identifies exactly one conversation; ambiguous source IDs return `None`. It returns the legacy-shaped JSON view, including nodes and messages.

`read_normalized(id)` accepts a local conversation ID and returns `None` when absent. Its JSON contains conversation identity/title/URL and `updated_at_ms`; graph `nodes`; message records with ordered `parts`; source current-node ID; and selection metadata. Each part is represented as `{kind:"text", text, raw}` for string values or `{kind:"unknown", raw}` for non-string JSON values. Raw part values remain represented, rather than being discarded as unrecognized.

Selection uses the provider's `current_node` when it names a stored graph node. Otherwise the selected node is the lexically smallest leaf node ID (`lexical_leaf_fallback`). `thread_node_ids` contains the selected node followed by its parent ancestry, stopping at a root, missing parent, or repeated node. `thread_complete` indicates whether this ancestry reaches a root (or is empty). This is a selected path view; the full graph, including alternate branches and null-message nodes, remains available in `nodes`.

## Library and search snapshots

`LibrarySnapshot` has exactly `library_revision: i64` and `conversations: Vec<serde_json::Value>`. `library_snapshot()` reads revision and normalized conversations in one SQLite read transaction. Conversations are ordered by `updated_at_ms` descending, null timestamps last, then local ID ascending. The revision lets a consumer detect changes between snapshots; it is not a sync cursor.

`SearchSnapshot` has exactly `library_revision: i64` and `hits: Vec<serde_json::Value>`. `search_snapshot(query)` returns message and title hits with a common conversation summary (`id`, provider, account scope, source ID, title, updated time), score, and `selected_branch`. Message hits additionally carry `node_id`, message ID, snippet, and a descendant `leaf_node_id`; title-only hits use null node/message/leaf IDs, title as snippet, and `selected_branch: false`. A message hit is marked selected when its node is in the normalized selected ancestry. The reported leaf is the lexically smallest reachable descendant leaf. Results are ordered by score then conversation and node IDs. Snapshot revision and hits are read together transactionally.

Both `search(query, limit)` and `search_snapshot(query)` treat input as literal whitespace-delimited terms, not as FTS5 query syntax: terms without any alphanumeric character are discarded; embedded quotes are escaped and each remaining term is quoted, then terms are ANDed. Thus callers cannot supply FTS operators/column selectors. Empty effective queries return no results. `search` returns local conversation IDs up to `limit`; `search_snapshot` provides ranked/snippet hit records and searches title and message text. Search indexing is backed by SQLite FTS5.

## Scope and verification limits

F1 is a local store/read/search implementation. It does not provide multi-account support, a complete synchronization engine, HTTP endpoints, or the deferred F2 functionality. Tests establish Linux behavior in the current environment (64 tests verified); they have not been verified on macOS or the desktop application. Migration and transaction code is present, but there is not yet a dedicated concurrent-writer fixture/test. Do not infer cross-platform or concurrent-writer coverage from the transactional implementation alone.
