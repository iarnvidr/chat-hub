import importlib.util, json, pathlib, tempfile, unittest
P=pathlib.Path(__file__).with_name('prepare-native-evidence.py')
spec=importlib.util.spec_from_file_location('safe_evidence',P); m=importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
class Evidence(unittest.TestCase):
 def test_only_allowlisted_names_and_json_fields_are_retained(self):
  with tempfile.TemporaryDirectory() as td:
   root=pathlib.Path(td); source=root/'source'; source.mkdir(); dest=root/'dest'
   (source/'native-launch-result.json').write_text(json.dumps({'passed':True,'process_id':123,'service_token':'do-not-copy','cookies':['bad'],'other':'discard','service_info':{'api':'http://127.0.0.1:12','api_token':'no'}}))
   (source/'service.token').write_text('private')
   (source/'dump.env').write_text('secret')
   (source/'app.log').write_text('Authorization: Bearer private-secret\nservice_token=secret')
   (source/'private.log').write_text('secret')
   import subprocess,sys
   subprocess.run([sys.executable,str(P),str(dest),str(source)],check=True,capture_output=True)
   record=json.loads((dest/'run-0/native-launch-result.json').read_text())
   self.assertEqual(record,{'passed':True,'process_id':123,'service_info':{'api':'http://127.0.0.1:12'}})
   log=(dest/'run-0/app.log').read_text()
   self.assertNotIn('private-secret',log); self.assertNotIn('service_token=secret',log)
   self.assertFalse((dest/'run-0/service.token').exists()); self.assertFalse((dest/'run-0/dump.env').exists()); self.assertFalse((dest/'run-0/private.log').exists())
if __name__=='__main__': unittest.main()
