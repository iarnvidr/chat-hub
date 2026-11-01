import pathlib
import sys
import unittest
from unittest import mock


ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "ci/scripts"))
import cache_retention


def version(version_id, day, tags=()):
    return cache_retention.validate_version({
        "id": version_id,
        "created_at": f"2026-10-{day:02}T12:00:00Z",
        "metadata": {"container": {"tags": list(tags)}},
    })


class CacheRetentionSelectionTests(unittest.TestCase):
    def test_preflight_selection_keeps_newest_platform_caches_and_three_fallbacks(self):
        versions = [
            version(12, 12, ["cargo-sccache-v1-macos-aabbccddeeff00112233"]),
            version(11, 11, ["cargo-sccache-v1-linux-11223344556677889900"]),
            version(10, 10, ["cargo-sccache-v1-macos-old-contract"]),
            version(9, 9, ["cargo-sccache-v1-linux-old-contract"]),
            version(8, 8),
            version(7, 7),
            version(6, 6),
            version(5, 5),
        ]
        self.assertEqual(cache_retention.select_versions_to_keep(versions), {12, 11, 10, 9, 8})

    def test_platform_tag_is_short_and_explicitly_scoped(self):
        self.assertEqual(cache_retention.package_name("Iarnvidr/Chat-Hub"), "chat-hub-rust-cache-chat-hub")
        self.assertEqual(cache_retention.platform_prefix("linux"), "cargo-sccache-v1-linux-")
        with self.assertRaises(cache_retention.RetentionError):
            cache_retention.platform_prefix("other")


class CacheRetentionPreflightTests(unittest.TestCase):
    def setUp(self):
        self.base = "https://api.github.com/users/example/packages/container/chat-hub-rust-cache-chat-hub/versions"
        self.versions = [
            {"id": 8, "created_at": "2026-10-08T12:00:00Z", "metadata": {"container": {"tags": ["cargo-sccache-v1-macos-aaaa"]}}},
            {"id": 7, "created_at": "2026-10-07T12:00:00Z", "metadata": {"container": {"tags": ["cargo-sccache-v1-linux-bbbb"]}}},
            {"id": 6, "created_at": "2026-10-06T12:00:00Z", "metadata": {"container": {"tags": []}}},
            {"id": 5, "created_at": "2026-10-05T12:00:00Z", "metadata": {"container": {"tags": []}}},
            {"id": 4, "created_at": "2026-10-04T12:00:00Z", "metadata": {"container": {"tags": []}}},
            {"id": 3, "created_at": "2026-10-03T12:00:00Z", "metadata": {"container": {"tags": []}}},
            {"id": 2, "created_at": "2026-10-02T12:00:00Z", "metadata": {"container": {"tags": []}}},
            {"id": 1, "created_at": "2026-10-01T12:00:00Z", "metadata": {"container": {"tags": []}}},
        ]

    def test_preflight_prunes_before_push_and_keeps_both_platform_tags(self):
        deleted = []

        def request(method, url, token):
            if method == "GET":
                return 200, self.versions
            deleted.append(int(url.rsplit("/", 1)[1]))
            return 204, None

        with mock.patch.object(cache_retention, "package_api_base", return_value=(self.base, "example", "chat-hub-rust-cache-chat-hub")), \
             mock.patch.object(cache_retention, "request_json", side_effect=request):
            result = cache_retention.preflight_publish("example/chat-hub", token="test-token")
        self.assertEqual(result, {"before": 8, "deleted": 3, "remaining": 5})
        self.assertEqual(set(deleted), {1, 2, 3})
        self.assertFalse({8, 7} & set(deleted))

    def test_delete_denial_fails_preflight_so_caller_must_skip_push(self):
        def request(method, _url, _token):
            return (200, self.versions) if method == "GET" else (403, {"message": "forbidden"})

        with mock.patch.object(cache_retention, "package_api_base", return_value=(self.base, "example", "package")), \
             mock.patch.object(cache_retention, "request_json", side_effect=request):
            with self.assertRaises(cache_retention.RetentionError):
                cache_retention.preflight_publish("example/chat-hub", token="test-token")

    def test_missing_package_is_the_only_empty_initial_state(self):
        with mock.patch.object(cache_retention, "package_api_base", return_value=(self.base, "example", "package")), \
             mock.patch.object(cache_retention, "request_json", return_value=(404, {"message": "Not Found"})):
            self.assertEqual(cache_retention.preflight_publish("example/chat-hub", token="test-token"),
                             {"before": 0, "deleted": 0, "remaining": 0})

    def test_incomplete_pagination_fails_closed(self):
        with mock.patch.object(cache_retention, "PAGE_SIZE", 2), \
             mock.patch.object(cache_retention, "MAX_PAGES", 1), \
             mock.patch.object(cache_retention, "request_json", return_value=(200, self.versions[:2])):
            with self.assertRaises(cache_retention.RetentionError):
                cache_retention.list_versions(self.base, "test-token")


if __name__ == "__main__":
    unittest.main()
