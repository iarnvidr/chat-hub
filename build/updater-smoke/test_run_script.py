import pathlib, subprocess, unittest
ROOT=pathlib.Path(__file__).resolve().parents[2]
SCRIPT=ROOT/'build/updater-smoke/run-macos.sh'
class ScriptRegression(unittest.TestCase):
 def test_fixture_preparation_precedes_build_for_each_version(self):
  s=SCRIPT.read_text(); block=s.split('build_fixture() {',1)[1].split('\n}',1)[0]
  self.assertLess(block.index('prepare_fixture.py'),block.index('cargo build --locked'))
  self.assertIn('build_fixture "$OLD"',s); self.assertIn('build_fixture "$NEW"',s)
  self.assertIn('actual temporary build tree',s)
  self.assertIn('OLD_HASH" != "$NEW_HASH',s)
  self.assertNotIn('installed_hash" == "$OLD_HASH',s)
  self.assertIn('service_info version', s)
 def test_starting_copies_are_copied_from_compiled_old_bundle(self):
  s=SCRIPT.read_text(); block=s.split('APP="$TMP/build-$OLD/Chat Hub-$OLD.app"',1)[1].split('# Isolated ephemeral',1)[0]
  self.assertIn('ditto "$APP" "$bundle"',block)
  self.assertIn('"$TMP/healthy/Chat Hub.app"',block); self.assertIn('"$TMP/tampered/Chat Hub.app"',block)
  self.assertNotIn('ditto "$MUNINN_UPDATER_SMOKE_APP" "$TMP/healthy',s)
 def test_tamper_checks_actual_old_running_identity(self):
  driver=(ROOT/'build/updater-smoke/driver.py').read_text()
  verifier=(ROOT/'build/updater-smoke/verify_evidence.py').read_text()
  for field in ('old_running_identity_retained','running_service_info','before_pid','proc.pid','expected_exe'):
   self.assertIn(field,driver)
  self.assertIn("retained.get('version')==old",driver)
  self.assertIn("tampered.get('old_running_identity_retained') is True",verifier)
  self.assertIn("'tampered.running_service_info.process_id.matches_before'",verifier)
  self.assertIn("tamper_retained.get('process_id') == tamper_before.get('process_id')",verifier)
  self.assertNotIn("tamper_retained.get('process_id')==healthy_identity.get('before_pid')",verifier)
 def test_tar_archive_uses_chat_hub_bundle_name(self):
  s=SCRIPT.read_text()
  self.assertIn('tar -czf "$out/Chat-Hub.app.tar.gz" -C "$TMP" "Chat Hub.app"',s)
  self.assertIn('/Chat-Hub.app.tar.gz',s)
if __name__=='__main__': unittest.main()
