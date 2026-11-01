import json
import pathlib
import shutil
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
                for rel in ("Cargo.toml", "Cargo.lock", "src-tauri/Cargo.toml", "src-tauri/tauri.conf.json", "src-tauri/build.rs", "src-tauri/src/main.rs"):
                    target = fixture / rel
                    target.write_bytes((ROOT / rel).read_bytes())
                shutil.copytree(ROOT / "crates", fixture / "crates")
                (fixture / "src").mkdir()
                for source in ("lib.rs", "main.rs"):
                    (fixture / "src" / source).write_text("")
                subprocess.run(["python3", str(SCRIPT), str(fixture), version], check=True)
                self.assertIn(f'version = "{version}"', (fixture / "Cargo.toml").read_text())
                self.assertIn(f'version = "{version}"', (fixture / "src-tauri/Cargo.toml").read_text())
                self.assertEqual(json.loads((fixture / "src-tauri/tauri.conf.json").read_text())["version"], version)
                self.assertFalse((fixture / "src-tauri/Cargo.lock").exists())
                lock = (fixture / "Cargo.lock").read_text()
                section = lock.split('name = "chat-hub-desktop"', 1)[1].split("[[package]]", 1)[0]
                self.assertIn(f'version = "{version}"', section)
                root_lock = (fixture / "Cargo.lock").read_text()
                self.assertIn('name = "chat-hub"\nversion = "' + version + '"', root_lock)
                self.assertIn('name = "chat-hub-model"\nversion = "0.1.0"', root_lock)
                self.assertIn('name = "chat-hub-formats"\nversion = "0.1.0"', root_lock)
                subprocess.run(["cargo", "metadata", "--format-version", "1", "--locked", "--offline", "--no-deps", "--manifest-path", str(fixture / "src-tauri/Cargo.toml")], check=True, stdout=subprocess.DEVNULL)
                subprocess.run(["cargo", "metadata", "--format-version", "1", "--locked", "--offline", "--no-deps", "--manifest-path", str(fixture / "Cargo.toml")], check=True, stdout=subprocess.DEVNULL)

if __name__ == "__main__":
    unittest.main()
