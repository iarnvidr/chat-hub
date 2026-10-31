"""Run both publication steps with ORAS's relative-file validation enforced."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import textwrap
import unittest

ROOT = Path(__file__).resolve().parents[1]


class OciTransportPaths(unittest.TestCase):
    def test_native_and_review_uploads_keep_path_validation_enabled(self):
        cases = [
            ("ci/actions/macos-native-build/action.yml", "    - id: publish-transport\n", "", "chat-hub-transport.tar.gz"),
            (".github/workflows/release.yml", "      - id: publish-review-transport\n", "\n  artifact-transfer:", "release-review-transport.tar.gz"),
        ]
        for filename, start, end, payload in cases:
            with self.subTest(filename=filename), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                workspace, runner = root / "workspace", root / "runner with spaces"
                (workspace / "build").mkdir(parents=True)
                runner.mkdir()
                fake = root / "oras"
                fake.write_text("""#!/usr/bin/env python3
import json, os, pathlib, sys
args = sys.argv[1:]
if args[0] == 'login':
    sys.stdin.read()
    sys.exit(0)
assert args[0] == 'push'
assert '--disable-path-validation' not in args
paths = [arg.split(':', 1)[0] for arg in args if ':application/vnd.chat-hub.' in arg]
assert len(paths) == 1
assert not pathlib.Path(paths[0]).is_absolute(), 'absolute file path detected'
assert pathlib.Path(paths[0]).is_file(), 'payload missing in working directory'
pathlib.Path(os.environ['PUSH_RECORD']).write_text(json.dumps({'cwd': str(pathlib.Path.cwd()), 'path': paths[0], 'args': args}))
print(json.dumps({'digest': 'sha256:' + 'a' * 64}))
""")
                fake.chmod(0o755)
                (workspace / "build/install-oras.sh").write_text('printf \'%s\\n\' "$FAKE_ORAS"\n')
                (runner / payload).write_text("transport fixture")
                env = {**os.environ, "RUNNER_TEMP": str(runner), "FAKE_ORAS": str(fake), "PUSH_RECORD": str(root / "push.json"), "GH_TOKEN": "fixture-only", "GITHUB_ACTOR": "fixture", "GITHUB_REPOSITORY": "iarnvidr/chat-hub", "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "2", "GITHUB_SHA": "a" * 40, "INPUT_TAG": "v0.2.1", "GITHUB_OUTPUT": str(root / "output")}
                source = (ROOT / filename).read_text().split(start, 1)[1]
                if end:
                    source = source.split(end, 1)[0]
                script = textwrap.dedent(source.split("run: |\n", 1)[1])
                subprocess.run(["bash", "-e", "-c", script], cwd=workspace, env=env, capture_output=True, check=True)
                record = json.loads((root / "push.json").read_text())
                self.assertEqual(Path(record['cwd']).resolve(), runner.resolve())
                self.assertEqual(record['path'], payload)
                self.assertIn('digest=sha256:' + 'a' * 64, (root / "output").read_text())


if __name__ == '__main__':
    unittest.main()
