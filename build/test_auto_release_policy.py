#!/usr/bin/env python3
"""Ensure merge publication never bypasses native acceptance or stable release policy."""
from pathlib import Path
import unittest
ROOT = Path(__file__).resolve().parents[1]

class AutoReleasePolicyTests(unittest.TestCase):
    def test_successful_main_acceptance_is_required(self):
        text = (ROOT / '.github/workflows/auto-release.yml').read_text()
        for required in (
            'workflows: ["Map accepted Chat Hub macOS artifact"]',
            "github.event.workflow_run.conclusion == 'success'",
            "github.event.workflow_run.event == 'push'",
            'merge-base --is-ancestor', 'build/release_artifact.py retrieve',
            'python3 build/merge_preview.py', '--verify-existing',
            '--prerelease --latest=false',
        ):
            self.assertIn(required, text)
        self.assertNotIn('--clobber', text)
        self.assertNotIn('TAURI_SIGNING_PRIVATE_KEY', text)
        helper = (ROOT / 'build/merge_preview.py').read_text()
        for required in ('source.get("accepted_commit")', 'source.get("accepted_tree")',
                         'native_artifact_digest', '"channel": "merge-preview"'):
            self.assertIn(required, helper)

    def test_acceptance_runs_for_every_main_merge(self):
        text = (ROOT / '.github/workflows/accept-native.yml').read_text().split('permissions:', 1)[0]
        self.assertIn('branches: [main]', text)
        self.assertNotIn('    paths:', text)

    def test_draft_cleanup_is_explicit_and_main_only(self):
        text = (ROOT / '.github/workflows/cleanup-release-draft.yml').read_text()
        for required in ('workflow_dispatch:', 'default: false',
                         "github.ref == 'refs/heads/main'", 'build/release_drafts.py',
                         'if [ "$DELETE_DRAFT" = true ]'):
            self.assertIn(required, text)
        self.assertNotIn('git push', text)
        self.assertNotIn('refs/tags/', text)

if __name__ == '__main__':
    unittest.main()
