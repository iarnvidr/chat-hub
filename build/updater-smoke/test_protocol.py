"""Host-independent checks for updater evidence contract and fail-closed negatives."""
import ast,copy,json,pathlib,shutil,subprocess,sys,tempfile,unittest
HERE=pathlib.Path(__file__).resolve().parent
BASE_H={'signature_accepted':True,'replacement_installed':True,'restart_requested':True,'restart_observed':True,'restart_process_exited':True,'settings_persisted':True,'tabs_persisted':True,'restart_identity':{'before_pid':101,'after_pid':202,'expected_executable':'/tmp/Muninn.app/Contents/MacOS/muninn','actual_executable':'/tmp/Muninn.app/Contents/MacOS/muninn'}}
BASE_T={'tamper_rejected':True,'tamper_status':{'state':'failed','reason':'signature_verification_failed'},'old_version_retained':True,'app_usable':True}
def run(root): return subprocess.run([sys.executable,str(HERE/'verify_evidence.py'),str(root)],capture_output=True)
class EvidenceProtocol(unittest.TestCase):
 def test_tampered_mode_usable_marker_does_not_require_healthy_fixture_theme(self):
  driver=(HERE/'driver.py').read_text()
  self.assertIn("usable=valid_settings(settings) and proc.poll() is None",driver)
  self.assertNotIn("usable=valid_settings(settings) and settings.get('theme')=='classified_light'",driver)
  self.assertIn("failed=[name for name,passed in",driver)
  self.assertIn("if failed: raise RuntimeError('tamper rejection assertions failed: '+', '.join(failed))",driver)
  self.assertLess(driver.index("(root/'tampered-result.json').write_text"),driver.index("if failed: raise RuntimeError"))
 def test_settings_response_requires_valid_object_and_theme(self):
  tree=ast.parse((HERE/'driver.py').read_text())
  fn=next(n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name=='valid_settings')
  helper=ast.Module(body=[fn],type_ignores=[])
  namespace={}; exec(compile(helper,'driver.py','exec'),namespace)
  valid=namespace['valid_settings']
  self.assertTrue(valid({'theme':'classified_light'}))
  for malformed in (None,{},[],{'theme':None},{'theme':3},{'error':'unavailable'}):
   self.assertFalse(valid(malformed))
 def test_driver_state_identity_keys_match_native_schema(self):
  driver=(HERE/'driver.py').read_text()
  schema=(HERE.parent.parent/'src-tauri/src/e2e.rs').read_text()
  self.assertIn('state["process_id"]',schema)
  self.assertIn("before.get('process_id')",driver)
  self.assertIn("after_state.get('process_id')",driver)
  self.assertIn("before.get('executable_path')",driver)
  self.assertIn("after_state.get('executable_path')",driver)
  self.assertIn("'op':'state'",driver)
  self.assertIn('before=native_state()',driver)
  self.assertIn('after_state=native_state()',driver)
  self.assertNotIn("invoke('state'",driver)
  self.assertIn('def schedule_install()',driver)
  self.assertIn('setTimeout(() => { Muninn.invoke',driver)
  self.assertIn('os.replace(tmp,command)',driver)
  self.assertIn("owned_pid=locals().get('after_pid')",driver)
  self.assertIn("if pathlib.Path(observed).resolve()==expected.resolve()",driver)
  self.assertIn('proc.wait(timeout=120)',driver)
  self.assertNotIn("root/'healthy'/'Muninn.app'",driver)
  self.assertNotIn('has_process_id',driver)
 def test_marker_uses_persistable_youtube_search_and_restart_checked_before_reads(self):
  tree=ast.parse((HERE/'driver.py').read_text())
  source=(HERE/'driver.py').read_text()
  self.assertIn("https://www.youtube.com/results?search_query=",source)
  healthy=source.index("after_pid=after_state.get('process_id')")
  guarded=source.index("after_pid==before_pid",healthy)
  settings=source.index("post_settings=api('GET','/api/settings')",healthy)
  self.assertLess(guarded,settings)
  self.assertLess(source.index("if not after_exe",healthy),settings)
 def test_install_schedule_is_valid_javascript_expression(self):
  if not shutil.which('node'): self.skipTest('node is unavailable')
  tree=ast.parse((HERE/'driver.py').read_text())
  fn=next(n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name=='schedule_install')
  call=next(n for n in ast.walk(fn) if isinstance(n,ast.Call) and isinstance(n.func,ast.Name) and n.func.id=='bridge')
  expression=ast.literal_eval(call.args[0])
  with tempfile.TemporaryDirectory() as d:
   js=pathlib.Path(d)/'schedule.js'
   js.write_text('async function bridge(){ return await ('+expression+'); }\n')
   result=subprocess.run(['node','--check',str(js)],capture_output=True)
   self.assertEqual(result.returncode,0,result.stderr.decode())
 def test_missing_evidence_fails_closed(self):
  with tempfile.TemporaryDirectory() as d:
   result=run(d); self.assertNotEqual(result.returncode,0); self.assertIn(b'FAIL',result.stderr)
 def test_complete_evidence_and_negative_identity_and_reason(self):
  with tempfile.TemporaryDirectory() as d:
   root=pathlib.Path(d); h=copy.deepcopy(BASE_H); t=copy.deepcopy(BASE_T)
   def write():
    (root/'healthy-result.json').write_text(json.dumps(h)); (root/'tampered-result.json').write_text(json.dumps(t))
   write(); subprocess.run([sys.executable,str(HERE/'verify_evidence.py'),d],check=True)
   emitted=json.loads((root/'evidence.json').read_text()); self.assertEqual(emitted,{'healthy':h,'tampered':t})
   for key,value in [('after_pid',101),('actual_executable','/wrong/app')]:
    bad=copy.deepcopy(h); bad['restart_identity'][key]=value
    (root/'healthy-result.json').write_text(json.dumps(bad)); self.assertNotEqual(run(root).returncode,0)
   for bad_status in (
    {'state':'failed','reason':'signature failed'},
    {'state':'failed','reason':'different_reason'},
    {'state':'available','reason':'signature_verification_failed'},
    {'state':'failed','status':{'reason':'signature_verification_failed'}},
   ):
    bad=copy.deepcopy(t); bad['tamper_status']=bad_status
    (root/'healthy-result.json').write_text(json.dumps(h)); (root/'tampered-result.json').write_text(json.dumps(bad))
    self.assertNotEqual(run(root).returncode,0)
if __name__=='__main__': unittest.main()
