"""Host-independent checks for updater evidence contract and fail-closed cases."""
import ast,copy,json,pathlib,subprocess,sys,tempfile,unittest,os
HERE=pathlib.Path(__file__).resolve().parent
BASE_H={'signature_accepted':True,'replacement_installed':True,'restart_requested':True,'restart_observed':True,'restart_process_exited':True,'before_version':'0.1.0','after_version':'0.1.1','restart_identity':{'before_pid':101,'after_pid':202,'expected_executable':'/tmp/Muninn.app/Contents/MacOS/app','actual_executable':'/tmp/Muninn.app/Contents/MacOS/app'}}
BASE_T={'tamper_rejected':True,'tamper_status':{'state':'failed','reason':'signature_verification_failed'},'old_version_retained':True,'app_usable':True}
def run(root): return subprocess.run([sys.executable,str(HERE/'verify_evidence.py'),str(root)],capture_output=True)
class EvidenceProtocol(unittest.TestCase):
 def test_restart_transport_classifier_is_narrow_and_postconditions_remain_required(self):
  s=(HERE/'driver.py').read_text(); tree=ast.parse(s)
  node=next(n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name=='restart_transport_interrupted')
  ns={}; exec(compile(ast.Module(body=[node],type_ignores=[]),str(HERE/'driver.py'),'exec'),ns)
  classify=ns['restart_transport_interrupted']
  self.assertTrue(classify(RuntimeError('native E2E eval bridge timed out')))
  self.assertTrue(classify(RuntimeError('connection closed')))
  self.assertFalse(classify(RuntimeError('signature_rejected')))
  self.assertFalse(classify(RuntimeError('signature verification failed')))
  self.assertIn("candidate.get('process_id') != before_pid",s)
  self.assertIn("post.get('version')!=new",s)
  self.assertIn("observed_status.get('status')=='signature_rejected'",s)
 def test_summary_is_bounded_across_reports_and_redacts_secret_file_names(self):
  reporter=HERE.parents[0]/'native-smoke-report.py'
  with tempfile.TemporaryDirectory() as d:
   root=pathlib.Path(d); log=root/'huge.log'; log.write_text(('x'*500+'\\n')*2000)
   evidence=root/'evidence'; evidence.mkdir(); (evidence/'service.token').write_text('TOKEN_SECRET_CONTENT'); (evidence/'privatekey.pem').write_text('PRIVATE_KEY_CONTENT'); (evidence/'ordinary.txt').write_text('ok')
   summary=root/'summary.md'
   args=[sys.executable,str(reporter),'--step','15','--status','1','--command','x','--reason','test','--logfiles',str(log),'--evidence',str(evidence),'--summary',str(summary)]
   # Use the script's positional interface as used by the workflow wrapper.
   args=[sys.executable,str(reporter),'15','1','x',str(evidence),str(log), '']
   for _ in range(8):
    env=dict(os.environ,GITHUB_STEP_SUMMARY=str(summary))
    result=subprocess.run(args,capture_output=True,text=True,check=True,env=env)
    self.assertLessEqual(len(result.stdout.encode()),400*1024)
   content=summary.read_bytes(); self.assertLessEqual(len(content),400*1024)
   self.assertNotIn(b'TOKEN_SECRET_CONTENT',content); self.assertNotIn(b'PRIVATE_KEY_CONTENT',content)
   self.assertNotIn(b'service.token',content); self.assertNotIn(b'privatekey.pem',content)
   self.assertIn(b'ordinary.txt',content)
   self.assertLessEqual(max(map(len,content.splitlines()),default=0),400*1024)
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
