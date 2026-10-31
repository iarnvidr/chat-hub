"""Release review must verify its extracted app and retain that verification."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import textwrap
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ReleaseSignatureEvidence(unittest.TestCase):
    def check_verification(self, status):
        workflow = (ROOT / '.github/workflows/release.yml').read_text()
        step = workflow.split('- name: Verify accepted app signature and retain evidence\n', 1)[1].split('      - name: Create updater archive, sign and verify', 1)[0]
        script = textwrap.dedent(step.split('run: |\n', 1)[1])
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            runner = root / 'runner with spaces'
            (runner / 'chat-hub/Chat Hub.app').mkdir(parents=True)
            codesign = root / 'codesign'
            codesign.write_text("""#!/usr/bin/env python3
import json, os, pathlib, sys
pathlib.Path(os.environ['ARGS']).write_text(json.dumps(sys.argv[1:]))
print('signature verification fixture', file=sys.stderr)
sys.exit(int(os.environ['STATUS']))
""")
            codesign.chmod(0o755)
            env = {**os.environ, 'PATH': str(root) + os.pathsep + os.environ['PATH'], 'ARGS': str(root / 'args'), 'STATUS': str(status), 'RUNNER_TEMP': str(runner), 'INPUT_VERSION': '0.2.1', 'GITHUB_SHA': 'a' * 40}
            result = subprocess.run(['bash', '-e', '-c', script], env=env, capture_output=True)
            self.assertEqual(json.loads((root / 'args').read_text()), ['--verify', '--deep', '--strict', '--verbose=2', str(runner / 'chat-hub/Chat Hub.app')])
            return result.returncode, (runner / 'chat-hub/verify-macos.txt').read_text()

    def test_success_retains_the_accepted_app_verification(self):
        code, evidence = self.check_verification(0)
        self.assertEqual(code, 0)
        self.assertIn('PASS: accepted app signature verified for version 0.2.1 at ' + 'a' * 40, evidence)

    def test_invalid_signature_blocks_release_without_a_pass_receipt(self):
        code, evidence = self.check_verification(7)
        self.assertEqual(code, 7)
        self.assertNotIn('PASS:', evidence)


if __name__ == '__main__':
    unittest.main()
