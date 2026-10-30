# Library archives

## SQLite backup

The CLI provides local backup and JSON export commands. Both use the configured
library database (`CHAT_HUB_DATABASE`, or the normal `CHAT_HUB_DATA_DIR` / platform
default) and initialize/open the shared Store directly; they do not start the
service, proxy, or capture runtime.

```sh
chat-hub backup /path/to/library.sqlite3
chat-hub export-json /path/to/library.json
```

Each command requires exactly one destination. Existing destinations are never
overwritten, and errors exit nonzero. Successful stdout is a small JSON receipt
containing operation and record counts/versions; archive contents and secrets
are not printed. `backup` is a SQLite online backup. It includes the ordinary
Store schema and rows (including schema migrations, library state, conversations,
observations, graph nodes, messages, and their associated relational data), not
an alternate portable interchange format. FTS/search indexes are not part of the
logical archive contract and can be rebuilt by opening the database. SQLite
internal bookkeeping is likewise not a portable API. Credentials/secrets are
not specially stripped: archives contain personal conversation/library content
and must be protected accordingly.

Publication uses an adjacent staging file and a no-replace hard link. The
completed file becomes atomically visible at the destination, so observers do
not see a partially-written destination. The staging file is then unlinked and,
on Unix, the parent directory is synced. Atomic visibility is distinct from
crash durability: a failed post-publication unlink or directory sync returns an
`ArchivePublished` error identifying the destination and failed operation. The
target is already complete and published; durability is unconfirmed, so callers
must not treat the result as an ordinary unpublished failure or blindly retry
to the same existing destination. Staging cleanup is best-effort on errors and
drop; there is no absolute cleanup or crash-durability guarantee.

## JSON export

`export-json` writes the versioned JSON library representation described by the
library JSON format (`format: "chat-hub-library"`, `format_version: 1`). The
archive is a set of explicit relational row arrays: each table uses its exact
SQLite column names and columns, and each row preserves that table's column
order. SQL `NULL` is JSON `null`; text (including values whose content looks
like JSON) is emitted as a JSON string; raw JSON payload/content columns are
therefore string-encoded rather than embedded JSON values. This is an export
representation, not a restore adapter: no JSON import/restore operation is
provided. FTS/search indexes are excluded and rebuildable. As with SQLite
backups, exports may contain sensitive personal conversation content; no
credential or content redaction is implied.
