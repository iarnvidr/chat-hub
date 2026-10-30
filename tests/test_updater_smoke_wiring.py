"""Wiring checks for actual ChatHub updater native smoke driver."""
import pathlib, subprocess, sys, tempfile, unittest
ROOT=pathlib.Path(__file__).resolve().parents[1]
SMOKE=ROOT/'build/updater-smoke'
class UpdaterSmokeWiring(unittest.TestCase):
 def test_driver_uses_real_ipc_and_checks_relaunch_version(self):
  driver=(SMOKE/'driver.py').read_text()
  for expected in ('window.ChatHub.invoke(',"invoke('update_status')","invoke('check_for_updates'","invoke('install_update')","post.get('version')!=new","after_pid!=before_pid","old_running_identity_retained","retained.get('version')==old","retained.get('process_id')==before_pid==proc.pid"):
   self.assertIn(expected,driver)
  for obsolete in ('Muninn.invoke','Muninn.api','updater_download','updater_install','tabs_state'):
   self.assertNotIn(obsolete,driver)
 def test_smoke_inputs_are_explicit_old_and_new_versions(self):
  runner=(SMOKE/'run-macos.sh').read_text()
  self.assertIn('OLD="0.1.0"',runner); self.assertIn('NEW="0.1.1"',runner)
  self.assertIn("!= '0.1.0'",(SMOKE/'verify_artifacts.py').read_text())
 def test_missing_native_results_fail_closed(self):
  with tempfile.TemporaryDirectory() as d:
   result=subprocess.run([sys.executable,str(SMOKE/'verify_evidence.py'),d],capture_output=True)
   self.assertNotEqual(result.returncode,0); self.assertIn(b'FAIL',result.stderr)
if __name__=='__main__': unittest.main()
