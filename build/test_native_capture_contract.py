import importlib.util
import pathlib
import unittest

spec = importlib.util.spec_from_file_location("native_capture_contract", pathlib.Path(__file__).with_name("native_capture_contract.py"))
contract = importlib.util.module_from_spec(spec)
spec.loader.exec_module(contract)


def snapshot(**changes):
    value = {"status": "available", "enabled": False,
             "counters": {**{key: 0 for key in contract.COUNTERS}, "last_error_code": None}}
    value.update(changes)
    return value


ARCHIVE = {"library_revision": 0, "items": [], "next_cursor": None}


class ContractTests(unittest.TestCase):
    def validate(self, before=None, after=None, **kwargs):
        return contract.validate(before or snapshot(), after or snapshot(),
                                 forwarding_ok=kwargs.pop("forwarding_ok", True),
                                 archive_before=kwargs.pop("archive_before", ARCHIVE),
                                 archive_after=kwargs.pop("archive_after", ARCHIVE), **kwargs)

    def test_disabled_zero_and_unchanged_pass(self):
        value = snapshot()
        self.validate(value, value.copy())

    def test_enabled_rejected(self):
        with self.assertRaises(AssertionError): self.validate(snapshot(enabled=True), snapshot(enabled=True))

    def test_unknown_or_missing_status_rejected(self):
        for bad in ({"status": "unavailable"}, {"status": "available", "enabled": False, "counters": {}}):
            with self.assertRaises(AssertionError): self.validate(bad, bad)

    def test_any_nonzero_authoritative_counter_rejected(self):
        value = snapshot(); value["counters"]["started"] = 1
        with self.assertRaises(AssertionError): self.validate(value, value)

    def test_archive_change_rejected(self):
        with self.assertRaises(AssertionError): self.validate(archive_before=ARCHIVE, archive_after={**ARCHIVE, "library_revision": 1})

    def test_nonempty_archive_rejected_including_full_first_page(self):
        for items, cursor in [([{"id": f"cv-{i}"} for i in range(100)], "more"),
                              ([{"id": "cv-a"}], None)]:
            archive = {"library_revision": 0, "items": items, "next_cursor": cursor}
            with self.assertRaises(AssertionError):
                self.validate(archive_before=archive, archive_after=archive)

    def test_missing_next_cursor_rejected(self):
        archive = {"library_revision": 0, "items": []}
        with self.assertRaises(AssertionError):
            self.validate(archive_before=archive, archive_after=archive)

    def test_archive_revision_change_rejected(self):
        changed = {"library_revision": 1, "items": [], "next_cursor": None}
        with self.assertRaises(AssertionError): self.validate(archive_after=changed)

    def test_failed_forwarding_rejected(self):
        value = snapshot()
        with self.assertRaises(AssertionError): self.validate(value, value, forwarding_ok=False)

    def test_unknown_counter_schema_rejected(self):
        value = snapshot(); del value["counters"]["lost_observations"]
        with self.assertRaises(AssertionError): self.validate(value, value)

    def test_missing_last_error_code_rejected(self):
        value = snapshot(); del value["counters"]["last_error_code"]
        with self.assertRaises(AssertionError): self.validate(value, value)

    def test_non_null_last_error_code_rejected(self):
        value = snapshot(); value["counters"]["last_error_code"] = "capture_failed"
        with self.assertRaises(AssertionError): self.validate(value, value)

    def test_boolean_numeric_counter_rejected(self):
        value = snapshot(); value["counters"]["started"] = False
        with self.assertRaises(AssertionError): self.validate(value, value)

if __name__ == "__main__": unittest.main()
