import json
import pathlib
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "build/updater-smoke/prepare_fixture.py"

class PrepareFixture(unittest.TestCase):
    def test_both_versions_keep_metadata_and_lock_consistent(self):
        for version in ("0.1.0", "0.1.1"):
            with self.subTest(version=version), tempfile.TemporaryDirectory() as tmp:
                fixture = pathlib.Path(tmp)
                (fixture / "src-tauri/src").mkdir(parents=True)
                for rel in ("Cargo.toml", "Cargo.lock", "src-tauri/Cargo.toml", "src-tauri/Cargo.lock", "src-tauri/tauri.conf.json", "src-tauri/build.rs", "src-tauri/src/main.rs", "package.json"):
                    target = fixture / rel
                    target.write_bytes((ROOT / rel).read_bytes())
                subprocess.run(["python3", str(SCRIPT), str(fixture), version], check=True)
                self.assertIn(f'version = "{version}"', (fixture / "Cargo.toml").read_text())
                self.assertIn(f'version = "{version}"', (fixture / "src-tauri/Cargo.toml").read_text())
                self.assertEqual(json.loads((fixture / "src-tauri/tauri.conf.json").read_text())["version"], version)
                self.assertEqual(json.loads((fixture / "package.json").read_text())["version"], version)
                lock = (fixture / "src-tauri/Cargo.lock").read_text()
                section = lock.split('name = "chat-hub-desktop"', 1)[1].split("[[package]]", 1)[0]
                self.assertIn(f'version = "{version}"', section)
                subprocess.run(["cargo", "metadata", "--format-version", "1", "--locked", "--offline", "--no-deps", "--manifest-path", str(fixture / "src-tauri/Cargo.toml")], check=True, stdout=subprocess.DEVNULL)

if __name__ == "__main__":
    unittest.main()
