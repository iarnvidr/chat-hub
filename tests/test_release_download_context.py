"""Release jobs can download their review bundle without any checkout."""
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ReleaseDownloadContext(unittest.TestCase):
    def test_downloads_pin_repository_without_a_git_checkout(self):
        workflow = (ROOT / '.github/workflows/release.yml').read_text()
        commands = re.findall(r'run: (gh run download .*release-assets)', workflow)
        self.assertEqual(len(commands), 2)
        for command in commands:
            with self.subTest(command=command), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                gh = root / 'gh'
                gh.write_text("""#!/usr/bin/env python3
import json, os, pathlib, sys
args = sys.argv[1:]
assert args[:2] == ['run', 'download']
assert '--repo' in args, 'repository context is missing without a checkout'
assert args[args.index('--repo') + 1] == os.environ['GITHUB_REPOSITORY']
pathlib.Path(os.environ['RECORD']).write_text(json.dumps(args))
""")
                gh.chmod(0o755)
                env = {**os.environ, 'PATH': str(root) + os.pathsep + os.environ['PATH'], 'GITHUB_REPOSITORY': 'iarnvidr/chat-hub', 'GITHUB_RUN_ID': '123', 'GH_REPO': 'unrelated/repository', 'RECORD': str(root / 'record')}
                script = command.replace('${{ inputs.tag }}', 'v0.2.1')
                subprocess.run(['bash', '-e', '-c', script], cwd=root, env=env, capture_output=True, check=True)
                args = json.loads((root / 'record').read_text())
                self.assertIn('release-review-v0.2.1', args)
                self.assertFalse((root / '.git').exists())


if __name__ == '__main__':
    unittest.main()
