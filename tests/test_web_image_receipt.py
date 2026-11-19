import argparse
import importlib.util
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("web_image_receipt", ROOT / "ci/scripts/web_image_receipt.py")
receipt_module = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(receipt_module)


def args(**changes):
    values = {
        "source_sha": "a" * 40,
        "run_id": 42,
        "run_attempt": 2,
        "publisher_run_id": 51,
        "publisher_run_attempt": 1,
        "image_ref": "ghcr.io/iarnvidr/chat-hub-web:run-42-2@sha256:" + "b" * 64,
    }
    values.update(changes)
    return argparse.Namespace(**values)


class WebImageReceiptTests(unittest.TestCase):
    def test_receipt_binds_source_and_publication_runs_to_digest(self):
        result = receipt_module.create(args())
        self.assertEqual(result["schema"], "chat-hub-web-image-receipt/v1")
        self.assertEqual(result["source"]["run_id"], 42)
        self.assertEqual(result["source"]["run_attempt"], 2)
        self.assertEqual(result["source"]["commit"], "a" * 40)
        self.assertEqual(result["publication"]["run_id"], 51)
        self.assertEqual(result["publication"]["run_attempt"], 1)
        self.assertEqual(result["publication"]["health_endpoint"], "/healthz")

    def test_rejects_mutable_or_wrong_image_reference(self):
        for reference in ("ghcr.io/iarnvidr/chat-hub-web:latest", "ghcr.io/other/chat-hub-web@sha256:" + "b" * 64):
            with self.subTest(reference=reference), self.assertRaises(ValueError):
                receipt_module.create(args(image_ref=reference))


if __name__ == "__main__":
    unittest.main()
