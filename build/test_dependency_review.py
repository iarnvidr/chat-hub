import json
import os
import unittest
from unittest.mock import patch

import dependency_review


class DependencyReview(unittest.TestCase):
    def run_review(self, response):
        with patch.dict(os.environ, {
            "GH_REPO": "owner/repository",
            "BASE_SHA": "base",
            "HEAD_SHA": "head",
        }), patch.object(dependency_review.subprocess, "run", return_value=type(
            "Result", (), {"stdout": json.dumps(response)}
        )()):
            return dependency_review.main()

    def test_clean_comparison_passes_and_allows_existing_exception(self):
        response = [[
            {"change_type": "added", "name": "safe", "vulnerabilities": []},
            {"change_type": "added", "name": "glib", "vulnerabilities": [
                {"advisory_ghsa_id": "GHSA-wrw7-89jp-8q8g", "severity": "moderate"}
            ]},
            {"change_type": "removed", "name": "old", "vulnerabilities": [
                {"advisory_ghsa_id": "GHSA-new", "severity": "critical"}
            ]},
        ]]
        self.assertEqual(self.run_review(response), 0)

    def test_new_unexcepted_vulnerability_fails(self):
        response = [[{
            "change_type": "changed",
            "name": "unsafe",
            "vulnerabilities": [{"advisory_ghsa_id": "GHSA-new", "severity": "high"}],
        }]]
        self.assertEqual(self.run_review(response), 1)

    def test_malformed_response_fails_closed(self):
        with self.assertRaises(ValueError):
            self.run_review([{"not": "a page list"}])


if __name__ == "__main__":
    unittest.main()
