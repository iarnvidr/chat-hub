import pathlib, subprocess, unittest
ROOT=pathlib.Path(__file__).resolve().parents[2]
SCRIPT=ROOT/'build/updater-smoke/run-macos.sh'
class ScriptRegression(unittest.TestCase):
 def test_fixture_preparation_precedes_build_for_each_version(self):
  s=SCRIPT.read_text(); block=s.split('build_fixture() {',1)[1].split('\n}',1)[0]
  self.assertLess(block.index('prepare_fixture.py'),block.index('cargo build --locked'))
  self.assertIn('build_fixture "$OLD"',s); self.assertIn('build_fixture "$NEW"',s)
 def test_starting_copies_are_copied_from_compiled_old_bundle(self):
  s=SCRIPT.read_text(); block=s.split('APP="$TMP/build-$OLD/Chat Hub-$OLD.app"',1)[1].split('# Isolated ephemeral',1)[0]
  self.assertIn('ditto "$APP" "$bundle"',block)
  self.assertIn('"$TMP/healthy/Chat Hub.app"',block); self.assertIn('"$TMP/tampered/Chat Hub.app"',block)
  self.assertNotIn('ditto "$MUNINN_UPDATER_SMOKE_APP" "$TMP/healthy',s)
 def test_tar_archive_uses_chat_hub_bundle_name(self):
  s=SCRIPT.read_text()
  self.assertIn('tar -czf "$out/Chat-Hub.app.tar.gz" -C "$TMP" "Chat Hub.app"',s)
  self.assertIn('/Chat-Hub.app.tar.gz',s)
if __name__=='__main__': unittest.main()
