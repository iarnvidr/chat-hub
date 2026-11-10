"""Cache transport must not obscure or replace the required check result."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ExportBoundaryTests(unittest.TestCase):
    def setUp(self):
        self.workflow = (ROOT / '.github/workflows/check.yml').read_text()
        self.export = self.workflow.split('      - id: cargo-export\n', 1)[1].split('      - name:', 1)[0]

    def test_export_is_bounded_below_the_job_deadline(self):
        self.assertIn('timeout-minutes: 6', self.export)
        self.assertIn('timeout --signal=TERM --kill-after=15s 300s', self.export)
        self.assertIn('continue-on-error: true', self.export)
        self.assertIn('!cancelled()', self.export)

    def test_export_never_overwrites_or_publishes_the_restored_seed(self):
        self.assertIn('--output artifacts/linux-cargo', self.export)
        self.assertNotIn('--output ci/cache/linux-cargo', self.export)
        self.assertIn('pack --source artifacts/linux-cargo', self.workflow)
        self.assertIn('crate_artifacts.py --source artifacts/linux-cargo', self.workflow)
        archive = self.workflow.split('      - id: cargo-archive\n', 1)[1].split('      - name:', 1)[0]
        self.assertIn("steps.cargo-export.outcome == 'success'", archive)

    def test_receipts_still_require_real_success(self):
        checks = self.workflow.split('      - id: dagger-checks\n', 1)[1].split('\n      - ', 1)[0]
        self.assertNotIn('continue-on-error:', checks)
        self.assertIn('verb: check', checks)
        self.assertIn('-vv', checks)
        receipt = self.workflow.split('      - name: Record the exact successful PR check receipt\n', 1)[1]
        self.assertIn('success()', receipt)
        self.assertIn("steps.dagger-checks.outcome == 'success'", receipt)


if __name__ == '__main__':
    unittest.main()
