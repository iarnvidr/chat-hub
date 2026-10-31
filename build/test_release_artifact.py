#!/usr/bin/env python3
import copy
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock
import zipfile

spec = importlib.util.spec_from_file_location('release_artifact', Path(__file__).with_name('release_artifact.py'))
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def zip_data(files):
    out = io.BytesIO()
    with zipfile.ZipFile(out, 'w') as archive:
        for name, contents in files.items():
            archive.writestr(name, contents)
    return out.getvalue()


class ReleaseArtifactTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / 'src-tauri').mkdir()
        (self.root / 'src-tauri/tauri.conf.json').write_text('{"version":"1.2.3"}')
        self.commit = 'a' * 40
        self.tree = 'b' * 40
        self.pr_head = 'c' * 40
        self.base = 'd' * 40
        self.archive_name = 'Chat_Hub_1.2.3_macos-arm64.zip'
        self.dmg_name = 'Chat_Hub_1.2.3_macos-arm64.dmg'
        self.app_archive = zip_data({'Chat Hub.app/Contents/Info.plist': b'fixture app'})
        self.checksum = f'{digest(self.app_archive)}  {self.archive_name}\n'.encode()
        self.dmg = b'accepted corresponding-source DMG'
        self.files = {self.archive_name: self.app_archive,
                      self.archive_name + '.sha256': self.checksum,
                      self.dmg_name: self.dmg}
        self.pr = {
            'number': 42, 'merged': True, 'merge_commit_sha': self.commit,
            'base': {'ref': 'main', 'repo': {'full_name': release.REPOSITORY}},
            'head': {'sha': self.pr_head, 'repo': {'full_name': release.REPOSITORY}},
        }
        self.native_run = {
            'id': 123, 'run_attempt': 2, 'path': release.NATIVE_WORKFLOW,
            'repository': {'full_name': release.REPOSITORY},
            'head_repository': {'full_name': release.REPOSITORY},
            'event': 'pull_request', 'head_branch': 'feature', 'head_sha': self.pr_head,
            'conclusion': 'success',
            'pull_requests': [{'number': 42, 'base': {'sha': self.base}}],
        }
        self.check_run = {
            'id': 124, 'run_attempt': 1, 'path': release.CHECK_WORKFLOW,
            'repository': {'full_name': release.REPOSITORY},
            'head_repository': {'full_name': release.REPOSITORY},
            'event': 'pull_request', 'head_branch': 'feature', 'head_sha': self.pr_head,
            'conclusion': 'success',
            'pull_requests': [{'number': 42, 'base': {'sha': self.base}}],
        }
        self.mapping_run = {
            'id': 789, 'run_attempt': 1, 'path': release.MAPPING_WORKFLOW,
            'repository': {'full_name': release.REPOSITORY}, 'event': 'push',
            'head_branch': 'main', 'head_sha': self.commit, 'conclusion': 'success',
        }
        self.manual_run = {
            'id': 321, 'run_attempt': 1, 'path': release.NATIVE_WORKFLOW,
            'repository': {'full_name': release.REPOSITORY}, 'event': 'workflow_dispatch',
            'head_branch': 'main', 'head_sha': self.commit, 'conclusion': 'success',
        }
        self.native_artifact = {'id': 456, 'name': f'{release.NATIVE_ARTIFACT_PREFIX}-pr-123-2', 'expired': False}
        self.check_artifact = {'id': 457, 'name': f'{release.CHECK_ARTIFACT_PREFIX}-124-1', 'expired': False}
        self.mapping_artifact = {'id': 987, 'name': f'{release.MAPPING_ARTIFACT_PREFIX}-{self.commit}-789-1', 'expired': False}
        self.manual_artifact = {'id': 654, 'name': f'{release.NATIVE_ARTIFACT_PREFIX}-main-321-1', 'expired': False}
        self.native_blob = None
        self.check_blob = None
        self.mapping_blob = None
        self.manual_blob = None
        self.include_mapping = True
        self.include_check = True
        self.policy_drift = False
        self.native_tree = self.tree
        self.build_native()
        self.build_check()

    def git(self, root, *args):
        return self.tree if 'HEAD^{tree}' in args else self.commit

    def build_native(self, event='pull_request', run=None, artifact=None, tree=None, workflow_sha=None):
        run = run or self.native_run
        artifact = artifact or self.native_artifact
        record = {
            'schema': 2, 'repository': release.REPOSITORY,
            'workflow_path': release.NATIVE_WORKFLOW,
            'run_id': run['id'], 'run_attempt': run['run_attempt'], 'event': event,
            'head': self.pr_head if event == 'pull_request' else self.commit,
            'tree': tree or self.tree, 'workflow_sha': workflow_sha or self.commit,
            'helper_sha': 'e' * 40,
            'toolchain': {'rustc': 'rustc fixture', 'node': 'v22 fixture', 'macos': '15', 'sdk': 'fixture'},
            'platform': release.PLATFORM, 'version': '1.2.3',
            'files': {name: digest(data) for name, data in self.files.items()},
        }
        if event == 'pull_request':
            record.update({'pr_number': 42, 'pr_head_sha': self.pr_head,
                           'tested_base': self.base, 'head_repository': release.REPOSITORY})
        self.native_blob = zip_data({**self.files, 'native-receipt.json': json.dumps(record, sort_keys=True).encode()})
        artifact['digest'] = 'sha256:' + digest(self.native_blob)

    def build_check(self, tree=None):
        record = {
            'schema': 1, 'repository': release.REPOSITORY,
            'workflow_path': release.CHECK_WORKFLOW,
            'run_id': self.check_run['id'], 'run_attempt': self.check_run['run_attempt'],
            'event': 'pull_request', 'head_sha': self.pr_head, 'base_sha': self.base,
            'pr_number': 42, 'tested_tree': tree or self.tree,
            'workflow_sha': self.commit, 'helper_sha': 'e' * 40,
        }
        self.check_blob = zip_data({'check-receipt.json': json.dumps(record, sort_keys=True).encode()})
        self.check_artifact['digest'] = 'sha256:' + digest(self.check_blob)

    def api(self, path, binary=False):
        if '/commits/' in path and path.endswith('/pulls'):
            return [{'number': 42, 'base': {'ref': 'main'}}]
        if path.endswith('/pulls/42'):
            return self.pr
        if '/workflows/macos.yml/runs?' in path:
            return {'workflow_runs': [self.native_run]}
        if '/workflows/check.yml/runs?' in path:
            return {'workflow_runs': [self.check_run] if self.include_check else []}
        if '/actions/runs/123/artifacts?per_page=100' in path:
            return {'artifacts': [self.native_artifact]}
        if '/actions/runs/124/artifacts?per_page=100' in path:
            return {'artifacts': [self.check_artifact]}
        if '/actions/runs/321/artifacts?per_page=100' in path:
            return {'artifacts': [self.manual_artifact]}
        if '/actions/runs/789/artifacts?per_page=100' in path:
            return {'artifacts': [self.mapping_artifact]}
        if path.endswith('/actions/runs/123'):
            return self.native_run
        if path.endswith('/actions/runs/124'):
            return self.check_run
        if path.endswith('/actions/runs/321'):
            return self.manual_run
        if path.endswith('/actions/runs/789'):
            return self.mapping_run
        if path.endswith('/actions/artifacts/456/zip'):
            return self.native_blob
        if path.endswith('/actions/artifacts/457/zip'):
            return self.check_blob
        if path.endswith('/actions/artifacts/654/zip'):
            return self.manual_blob
        if path.endswith('/actions/artifacts/987/zip'):
            return self.mapping_blob
        if '/contents/' in path:
            if self.policy_drift and 'ref=' + ('9' * 40) in path:
                return {'sha': 'f' * 40}
            return {'sha': 'e' * 40}
        raise AssertionError(path)

    def promote(self):
        with mock.patch.object(release, 'git', side_effect=self.git):
            return release.promote(self.root, self.commit, self.api)

    def retrieve(self, **kwargs):
        with mock.patch.object(release, 'git', side_effect=self.git):
            return release.retrieve(self.root, 'v1.2.3', api_fn=self.api, **kwargs)

    def install_mapping(self, mapping):
        name = f'chat-hub-accepted-native-{self.commit}-{self.mapping_run["id"]}-{self.mapping_run["run_attempt"]}'
        self.mapping_artifact['name'] = name
        self.mapping_blob = zip_data({'accepted-native.json': json.dumps(mapping, sort_keys=True).encode()})
        self.mapping_artifact['digest'] = 'sha256:' + digest(self.mapping_blob)

    def test_main_promoter_maps_same_repo_merge_and_exact_native_and_check_receipts(self):
        mapping = self.promote()
        self.assertEqual(mapping['accepted_commit'], self.commit)
        self.assertEqual(mapping['accepted_tree'], self.tree)
        self.assertEqual(mapping['source_run_id'], 123)
        self.assertEqual(mapping['source_artifact_id'], 456)
        self.assertEqual(mapping['source_artifact_digest'], self.native_artifact['digest'])
        self.assertEqual(mapping['check_receipt']['run_id'], 124)
        self.assertEqual(mapping['check_receipt']['artifact_id'], 457)
        self.assertEqual(json.loads((self.root / 'accepted-native.json').read_text()), mapping)

    def test_promoter_rejects_fork_and_missing_check_receipt(self):
        self.pr['head']['repo']['full_name'] = 'fork/chat-hub'
        with self.assertRaisesRegex(ValueError, 'fork-produced'):
            self.promote()
        self.pr['head']['repo']['full_name'] = release.REPOSITORY
        self.include_check = False
        with self.assertRaisesRegex(ValueError, 'Linux check receipt unavailable'):
            self.promote()

    def test_promoter_rejects_wrong_tree_policy_or_base(self):
        self.build_native(tree='f' * 40)
        with self.assertRaisesRegex(ValueError, 'accepted PR tree'):
            self.promote()
        self.build_native()
        self.policy_drift = True
        self.build_native(workflow_sha='9' * 40)
        with self.assertRaisesRegex(ValueError, 'workflow differs'):
            self.promote()
        self.policy_drift = False
        self.native_run['pull_requests'][0]['base']['sha'] = 'f' * 40
        with self.assertRaisesRegex(ValueError, 'accepted native artifact unavailable'):
            self.promote()

    def test_retrieval_uses_explicit_mapping_and_preserves_native_archive_and_dmg(self):
        mapping = self.promote()
        self.install_mapping(mapping)
        result = self.retrieve(mapping_run_id=789)
        self.assertEqual(result['acceptance'], 'merged-pr')
        self.assertEqual(result['source_artifact_id'], 456)
        self.assertEqual((self.root / 'artifacts' / self.archive_name).read_bytes(), self.app_archive)
        self.assertEqual((self.root / 'artifacts' / self.dmg_name).read_bytes(), self.dmg)
        self.assertEqual(json.loads((self.root / 'artifacts/release-input.json').read_text()), result)

    def test_retrieval_rejects_changed_artifact_identity_and_mapping_digest(self):
        mapping = self.promote()
        self.install_mapping(mapping)
        self.native_artifact['digest'] = 'sha256:' + '0' * 64
        with self.assertRaisesRegex(ValueError, 'artifact ID or immutable digest changed'):
            self.retrieve(mapping_run_id=789)
        self.build_native()
        self.native_artifact['digest'] = 'sha256:' + digest(self.native_blob)
        self.mapping_artifact['digest'] = 'sha256:' + '0' * 64
        with self.assertRaisesRegex(ValueError, 'immutable artifact digest'):
            self.retrieve(mapping_run_id=789)

    def test_check_receipt_tree_change_blocks_release_retrieval(self):
        mapping = self.promote()
        self.install_mapping(mapping)
        accepted_digest = self.check_artifact['digest']
        self.build_check(tree='f' * 40)
        # Simulate a changed receipt payload under a replayed GitHub artifact digest.
        self.check_artifact['digest'] = accepted_digest
        with self.assertRaisesRegex(ValueError, 'immutable artifact digest mismatch'):
            self.retrieve(mapping_run_id=789)

    def test_manual_trusted_main_fallback_requires_explicit_run_id(self):
        self.include_mapping = False
        self.native_run = self.manual_run
        self.native_artifact = self.manual_artifact
        self.build_native(event='workflow_dispatch', run=self.manual_run, artifact=self.manual_artifact)
        self.manual_blob = self.native_blob
        with self.assertRaisesRegex(ValueError, 'exactly one'):
            self.retrieve()
        result = self.retrieve(trusted_main_run_id=321)
        self.assertEqual(result['acceptance'], 'trusted-main-manual')
        self.assertEqual(result['source_artifact_id'], 654)
        self.assertEqual((self.root / 'artifacts' / self.dmg_name).read_bytes(), self.dmg)

    def test_manual_fallback_rejects_push_pr_and_non_main_runs(self):
        self.manual_run['event'] = 'push'
        self.native_run = self.manual_run
        self.native_artifact = self.manual_artifact
        self.build_native(event='workflow_dispatch', run=self.manual_run, artifact=self.manual_artifact)
        self.manual_blob = self.native_blob
        with self.assertRaisesRegex(ValueError, 'trusted-main manual native run'):
            self.retrieve(trusted_main_run_id=321)
        self.manual_run['event'] = 'workflow_dispatch'
        self.manual_run['head_branch'] = 'feature'
        with self.assertRaisesRegex(ValueError, 'trusted-main manual native run'):
            self.retrieve(trusted_main_run_id=321)

    def test_tag_version_and_unsafe_native_archive_paths_are_rejected(self):
        with mock.patch.object(release, 'git', side_effect=self.git):
            with self.assertRaisesRegex(ValueError, 'tag does not match'):
                release.retrieve(self.root, 'v9.9.9', api_fn=self.api,
                                 trusted_main_run_id=321)
        self.app_archive = zip_data({'Chat Hub.app/../../outside': b'escape'})
        self.checksum = f'{digest(self.app_archive)}  {self.archive_name}\n'.encode()
        self.files[self.archive_name] = self.app_archive
        self.files[self.archive_name + '.sha256'] = self.checksum
        self.build_native()
        with self.assertRaisesRegex(ValueError, 'unsafe member'):
            self.promote()

    def test_retrieval_requires_exactly_one_mapping_or_fallback(self):
        with self.assertRaisesRegex(ValueError, 'exactly one'):
            self.retrieve(mapping_run_id=789, trusted_main_run_id=321)


if __name__ == '__main__':
    unittest.main()
