"""Pure assertions for the opt-in native smoke's default-off capture contract."""

COUNTERS = (
    "started", "active", "committed_imported", "committed_updated",
    "committed_unchanged", "invalid_snapshot", "unsupported_encoding", "decode_failed",
    "oversized", "dropped", "interrupted", "store_failed",
    "shutdown_unprocessed", "admitted", "completed_responses", "queued",
    "writer_in_flight", "retained_bytes", "lost_observations",
)
METADATA = ("last_error_code",)


def validate(before, after, *, forwarding_ok, archive_before, archive_after):
    """Require a fresh empty archive and reject mutation, nonzero capture, or failed forwarding."""
    if forwarding_ok is not True:
        raise AssertionError("TLS fixture did not traverse the proxy")
    if not isinstance(before, dict) or not isinstance(after, dict):
        raise AssertionError("capture status must be an object")
    for archive in (archive_before, archive_after):
        if (not isinstance(archive, dict) or type(archive.get("library_revision")) is not int
                or "items" not in archive or "next_cursor" not in archive):
            raise AssertionError("canonical archive snapshot has unknown schema")
        if archive.get("items") != [] or archive.get("next_cursor") is not None:
            raise AssertionError("default-off smoke requires a fresh empty canonical archive")
    if archive_before != archive_after:
        raise AssertionError("canonical archive changed during default-off TLS fixture")
    for snapshot in (before, after):
        if snapshot.get("status") != "available" or snapshot.get("enabled") is not False:
            raise AssertionError("capture status is unavailable, missing, or enabled")
        counters = snapshot.get("counters")
        if not isinstance(counters, dict) or set(counters) != set(COUNTERS) | set(METADATA):
            raise AssertionError("authoritative capture counter schema is unknown")
        if any(type(counters[k]) is not int or counters[k] != 0 for k in COUNTERS):
            raise AssertionError("default-off capture counters are not all zero")
        if counters["last_error_code"] is not None:
            raise AssertionError("default-off capture last_error_code must be null")
