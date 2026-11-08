#!/usr/bin/env python3
"""Ensure merge publication never bypasses native acceptance or stable release policy."""
from pathlib import Path
import unittest
ROOT=Path(__file__).resolve().parents[1]

class AutoReleasePolicyTests(unittest.TestCase):
    def test_successful_main_acceptance_is_required(self):
        text=(ROOT/'.github/workflows/auto-release.yml').read_text()
        for required in (
            'workflows: ["Map accepted Chat Hub macOS artifact"]',
            "github.event.workflow_run.conclusion == 'success'",
            "github.event.workflow_run.event == 'push'",
            'merge-base --is-ancestor', 'build/release_artifact.py retrieve',
            "source.get('accepted_commit')", 'native_artifact_digest',
            '--prerelease --latest=false',
        ):
            self.assertIn(required,text)
        self.assertNotIn('--clobber',text)
        self.assertNotIn('TAURI_SIGNING_PRIVATE_KEY',text)
    def test_acceptance_runs_for_every_main_merge(self):
        text=(ROOT/'.github/workflows/accept-native.yml').read_text().split('permissions:',1)[0]
        self.assertIn('branches: [main]',text)
        self.assertNotIn('    paths:',text)

if __name__=='__main__':
    unittest.main()
