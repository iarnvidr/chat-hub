# Library JSON archive format (version 1)

`Store::write_library_json` streams a UTF-8 JSON document with top-level fields `format` (`chat-hub-library`), `format_version` (1), `schema_version`, `library_revision`, and arrays `conversations`, `nodes`, `messages`, and `observations`. Rows are emitted in stable primary-key order from a single SQLite read snapshot. Metadata describes that snapshot; each count is available from the corresponding array length.

Each array entry is a canonical database row represented by its SQL column names and values. SQL NULL is JSON null; numeric columns are JSON numbers; text columns are JSON strings. In particular, `author_json`, `create_time_json`, `parts_json`, `raw_payload`, and `ordering_payload` remain their original encoded text strings, not parsed/re-serialized JSON. This preserves source text exactly. The export is a data archive, not a normalized UI representation, and this implementation does not provide a JSON restore adapter. Use the SQLite backup API for a complete SQLite backup.

`Store::export_json_to` writes through an adjacent staging file, synchronizes it and publishes without replacing an existing destination. A failed write/publish cleans up staging and leaves the source database unchanged.
