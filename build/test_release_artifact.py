#!/usr/bin/env python3
import copy
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


class ReleaseArtifactTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / 'src-tauri').mkdir()
        (self.root / 'src-tauri/tauri.conf.json').write_text('{"version":"1.2.3"}')
        self.commit = 'a' * 40
        self.tree = 'b' * 40
        self.run = {'id': 123, 'run_attempt': 2, 'repository': {'full_name': release.REPOSITORY},
                    'head_repository': {'full_name': release.REPOSITORY}, 'event': 'workflow_dispatch',
                    'head_branch': 'main', 'head_sha': self.commit, 'conclusion': 'success',
                    'path': release.WORKFLOW}
        self.name = 'Chat_Hub_1.2.3_macos-arm64.zip'
        self.files = {self.name: b'accepted app zip', self.name + '.sha256': b'checksum file', self.name.removesuffix('.zip') + '.dmg': b'accepted dmg'}
        self.record = {'schema': 1, 'repository': release.REPOSITORY, 'run_id': 123,
                       'run_attempt': 2, 'event': 'workflow_dispatch', 'head': self.commit, 'tree': self.tree,
                       'workflow_sha': self.commit, 'platform': 'darwin-aarch64', 'version': '1.2.3',
                       'files': {n: release.sha(v) for n, v in self.files.items()}}
        self.record['toolchain'] = {'rustc': 'rustc fixture', 'node': 'v22 fixture', 'macos': '15', 'sdk': 'fixture'}
        self.artifact = {'id': 456, 'name': release.ARTIFACT, 'expired': False}

    def api(self, path, binary=False):
        if '/contents/' in path:
            return {'sha': 'approved-workflow-blob'}
        if '/workflows/' in path:
            return {'workflow_runs': [self.run]}
        if path.endswith('/artifacts?per_page=100'):
            return {'artifacts': [self.artifact]}
        if path.endswith('/456/zip'):
            return self.data
        raise AssertionError(path)

    def bundle(self, extra=None):
        output = io.BytesIO()
        with zipfile.ZipFile(output, 'w') as archive:
            for name, data in self.files.items():
                archive.writestr(name, data)
            archive.writestr('native-receipt.json', json.dumps(self.record))
            if extra:
                archive.writestr(extra, b'unsafe')
        self.data = output.getvalue()
        self.artifact['digest'] = 'sha256:' + release.sha(self.data)

    def retrieve(self):
        with mock.patch.object(release, 'git', side_effect=lambda root, *a: self.tree if 'HEAD^{tree}' in a else self.commit):
            return release.retrieve(self.root, 'v1.2.3', self.api)

    def test_exact_run_and_inventory_promote_without_building(self):
        self.bundle()
        result = self.retrieve()
        self.assertEqual(result['artifact_id'], 456)
        self.assertEqual(result['producer_run_attempt'], 2)
        self.assertEqual((self.root / 'artifacts' / self.name).read_bytes(), self.files[self.name])

    def test_fork_or_pr_artifact_never_reaches_signer(self):
        for field, value in [('event', 'pull_request'), ('head_repository', {'full_name': 'fork/hrungnir'})]:
            with self.subTest(field=field):
                run = copy.deepcopy(self.run)
                self.run[field] = value
                self.bundle()
                with self.assertRaisesRegex(ValueError, 'trusted main'):
                    self.retrieve()
                self.run = run
        self.assertFalse((self.root / 'artifacts').exists())

    def test_wrong_attempt_receipt_is_rejected(self):
        self.record['run_attempt'] = 1
        self.bundle()
        with self.assertRaisesRegex(ValueError, 'receipt identity'):
            self.retrieve()

    def test_missing_or_expired_artifact_does_not_rebuild(self):
        self.bundle()
        self.artifact['expired'] = True
        with self.assertRaisesRegex(ValueError, 'eligible artifact'):
            self.retrieve()

    def test_tampered_download_and_file_bytes_are_rejected(self):
        self.bundle()
        self.artifact['digest'] = 'sha256:' + '0' * 64
        with self.assertRaisesRegex(ValueError, 'immutable artifact digest'):
            self.retrieve()
        self.files[self.name] = b'replaced app'
        self.bundle()
        with self.assertRaisesRegex(ValueError, 'file checksum'):
            self.retrieve()

    def test_unexpected_archive_member_is_rejected(self):
        self.bundle('../outside')
        with self.assertRaisesRegex(ValueError, 'inventory'):
            self.retrieve()


if __name__ == '__main__':
    unittest.main()
