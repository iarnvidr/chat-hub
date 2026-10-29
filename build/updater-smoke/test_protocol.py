"""Host-independent checks for updater evidence contract and fail-closed cases."""
import ast,copy,json,pathlib,subprocess,sys,tempfile,unittest
HERE=pathlib.Path(__file__).resolve().parent
BASE_H={'signature_accepted':True,'replacement_installed':True,'restart_requested':True,'restart_observed':True,'restart_process_exited':True,'before_version':'0.1.0','after_version':'0.1.1','restart_identity':{'before_pid':101,'after_pid':202,'expected_executable':'/tmp/Muninn.app/Contents/MacOS/app','actual_executable':'/tmp/Muninn.app/Contents/MacOS/app'}}
BASE_T={'tamper_rejected':True,'tamper_status':{'state':'failed','reason':'signature_verification_failed'},'old_version_retained':True,'app_usable':True}
def run(root): return subprocess.run([sys.executable,str(HERE/'verify_evidence.py'),str(root)],capture_output=True)
class EvidenceProtocol(unittest.TestCase):
 def test_real_app_api_names_and_no_invented_commands(self):
  s=(HERE/'driver.py').read_text()
  for token in ('window.ChatHub.invoke(',"invoke('update_status')","invoke('check_for_updates'","invoke('install_update')","post.get('version')!=new"):
   self.assertIn(token,s)
  for token in ('Muninn.invoke','Muninn.api','updater_download','updater_install','tabs_state'):
   self.assertNotIn(token,s)
 def test_native_identity_schema_and_bridge(self):
  s=(HERE/'driver.py').read_text(); main=(HERE.parents[1]/'src-tauri/src/main.rs').read_text()
  for key in ('process_id','executable_path','window_visible'):
   self.assertIn(key,main); self.assertIn(key,s)
  self.assertIn("'op':'state'",s); self.assertIn('os.replace(tmp,command)',s)
 def test_versions_are_real_target_fixture(self):
  self.assertIn('OLD="0.1.0"',(HERE/'run-macos.sh').read_text())
  self.assertIn('NEW="0.1.1"',(HERE/'run-macos.sh').read_text())
 def test_missing_evidence_fails_closed(self):
  with tempfile.TemporaryDirectory() as d: self.assertNotEqual(run(d).returncode,0)
 def test_complete_evidence_and_negative_cases(self):
  with tempfile.TemporaryDirectory() as d:
   root=pathlib.Path(d)
   def write(h,t):
    (root/'healthy-result.json').write_text(json.dumps(h)); (root/'tampered-result.json').write_text(json.dumps(t))
   h=copy.deepcopy(BASE_H); t=copy.deepcopy(BASE_T); write(h,t)
   subprocess.run([sys.executable,str(HERE/'verify_evidence.py'),d],check=True)
   for key,value in [('after_pid',101),('actual_executable','/wrong')]:
    bad=copy.deepcopy(h); bad['restart_identity'][key]=value; write(bad,t); self.assertNotEqual(run(root).returncode,0)
   for reason in ('not a signature error','unclassified'):
    bad=copy.deepcopy(t); bad['tamper_status']['reason']=reason; write(h,bad); self.assertNotEqual(run(root).returncode,0)
if __name__=='__main__': unittest.main()
