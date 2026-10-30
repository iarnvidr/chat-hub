# API v1 synthetic response fixtures

These are normative target responses, not live endpoint recordings or implementation proof. See [the contract](../../../docs/api-v1-contract.md). No production data, credentials, real account identifiers, or file paths occur in the fixtures. All HTTP requests require the configured Bearer token. Native adapter examples are not HTTP routes.

| Fixture (`.json`) | Request/scenario | HTTP status |
| --- | --- | --- |
| status-ready | `GET /api/v1/status`, initialized service and healthy DB | 200 |
| status-failed | `GET /api/v1/status`, failed worker, healthy DB | 503 |
| status-database-failed | `GET /api/v1/status`, database unavailable | 503 |
| version | `GET /api/v1/version`, illustrative version values | 200 |
| conversations-page-1 | `GET /api/v1/conversations?limit=1` | 200 |
| conversations-page-2 | `GET /api/v1/conversations?limit=1&cursor=fixture-list-r7-after-cv-a` | 200 |
| conversations-empty | `GET /api/v1/conversations`, separate empty library at revision 0 | 200 |
| search-page-1 | `GET /api/v1/search?q=amber%20telescope&limit=1` | 200 |
| search-page-2 | `GET /api/v1/search?q=amber%20telescope&limit=1&cursor=fixture-search-r7-after-n-a` | 200 |
| search-empty | `GET /api/v1/search?q=nonexistentterm` | 200 |
| search-title | `GET /api/v1/search?q=branches` | 200 |
| conversation-selected | `GET /api/v1/conversations/cv-a` | 200 |
| conversation-alternate | `GET /api/v1/conversations/cv-a?leaf_node_id=n-b` | 200 |
| conversation-fallback | `GET /api/v1/conversations/cv-b` | 200 |
| import-result | `POST /api/v1/imports/chatgpt`, four entries: new, changed, duplicate, invalid | 200 |
| import-cancelled | Native file picker cancelled | n/a |
| import-picker-result | Native file picker completed with same import-result receipt | n/a |
| error-invalid-cursor | `GET /api/v1/conversations?cursor=invalid` | 400 |
| error-stale-cursor | Replay page-1 cursor after durable mutation to revision 8 | 409 |
| error-not-found | `GET /api/v1/conversations/no-such-conversation` | 404 |

All nonempty library examples share revision 7 and contain exactly `cv-a` and `cv-b`. List updated timestamps tie, testing canonical ID ascending. Search ranks are illustrative finite values in ascending order, not expected SQLite BM25 constants. The phrase `amber telescope` exists only in message text, never in a title. Search snippets are literal substrings of their message text and contain no inserted highlights. A title match uses three null IDs; every message match provides a canonical message ID, scoped node ID, and navigable descendant leaf ID.

Cursor strings are synthetic opaque stand-ins, not production token encodings or usable authentication material. A fixture consumer maps each page's next_cursor to its next request verbatim. A stale cursor scenario starts at revision 7 and mutates to revision 8; it is not a third page of the unchanged revision-7 library.

`cv-a` has a null-message root, a user node, and two assistant leaves. The default selected leaf is `n-a`; `n-b` is a view-only override. Both responses retain identical nodes and messages. Markdown fenced code survives as source text. The second assistant includes an unsupported structured widget preserved both in raw content and the canonical unknown part; clients show an unsupported-content placeholder. Node IDs and canonical message IDs intentionally differ. `cv-b` preserves an invalid source pointer, falls back to its smallest source-ID leaf, and tests null provider message ID/timestamps.

The import HTTP receipt has exactly six keys: four counts, errors, and errors_truncated. Its four counts sum to four entries; the invalid entry is zero-based index 3. The native completion wraps that exact receipt. Errors contain no paths or source bodies. With more than 100 failed entries, return only the first 100 errors, retain the full failed count, and set errors_truncated true.

Validation for this documentation change parses every JSON file and checks paging/sort order, revisions, node/message references, selected ancestry/leaves, unchanged graphs between views, timestamp conversion/null preservation, snippets, and receipt/wrapper equality. Runtime acceptance still needs real auth, revision/transaction, FTS, cursor tampering/staleness, malformed graph, size/admission, and lifecycle tests. No runtime test or macOS readiness is implied.
