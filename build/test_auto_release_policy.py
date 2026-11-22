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
            "github.event.workflow_run.event == 'workflow_dispatch'",
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

    def test_native_acceptance_only_runs_on_request(self):
        text = (ROOT / '.github/workflows/accept-native.yml').read_text().split('permissions:', 1)[0]
        self.assertIn('workflow_dispatch:', text)
        self.assertNotIn('  push:', text)
        self.assertNotIn('    paths:', text)

    def test_apple_silicon_builds_are_only_requested_on_demand(self):
        workflow = (ROOT / '.github/workflows/macos.yml').read_text()
        pr = workflow.split('  package-pr:', 1)[1].split('  package-main:', 1)[0]
        main = workflow.split('  package-main:', 1)[1].split('  artifact-transfer:', 1)[0]
        gate = workflow.split('  native-release-gate:', 1)[1]
        self.assertIn('workflow_dispatch:', workflow.split('permissions:', 1)[0])
        self.assertIn("github.event_name == 'workflow_dispatch'", pr)
        self.assertNotIn("github.event_name == 'pull_request'", pr)
        self.assertIn("github.event_name == 'workflow_dispatch'", main)
        self.assertIn('runs-on: ubuntu-24.04', gate)
        self.assertIn("Apple Silicon was NOT built for this PR", gate)
        self.assertIn('test "$PR_PACKAGE_RESULT" = skipped', gate)
        self.assertNotIn('runs-on: macos', gate)
        self.assertNotIn('  scope:', workflow)

    def test_release_starts_after_manual_native_acceptance_only(self):
        mapping = (ROOT / '.github/workflows/accept-native.yml').read_text()
        dispatch = mapping.split('permissions:', 1)[0]
        self.assertIn('workflow_dispatch:', dispatch)
        self.assertNotIn('  push:', dispatch)
        self.assertIn("CHAT_HUB_NATIVE_RECEIPT_WORKFLOW: .github/workflows/accept-native.yml", mapping)
        auto_release = (ROOT / '.github/workflows/auto-release.yml').read_text()
        self.assertIn("github.event.workflow_run.event == 'workflow_dispatch'", auto_release)
        self.assertNotIn("github.event.workflow_run.event == 'push'", auto_release)

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
