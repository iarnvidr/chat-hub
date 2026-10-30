import json
import pathlib
import subprocess
import sys
import tempfile
import unittest
from contextlib import contextmanager
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT = ROOT / 'build/prepare-native-evidence.py'

class NativeEvidenceTests(unittest.TestCase):
    @contextmanager
    def run_case(self, value=None, raw=None, extras=(), symlink=False):
        # Keep the temporary tree alive while callers inspect output and source neighbors.
        with tempfile.TemporaryDirectory() as temp:
            base = pathlib.Path(temp); source=base/'source'; output=base/'out'; source.mkdir()
            result=source/'native-library-result.json'
            if symlink:
                target=base/'target'; target.write_text('{}'); result.symlink_to(target)
            else:
                result.write_text(raw if raw is not None else json.dumps(value))
            for name, data in extras:
                p=source/name; p.parent.mkdir(parents=True,exist_ok=True); p.write_text(data)
            proc=subprocess.run([sys.executable,str(SCRIPT),str(output),str(source)],capture_output=True,text=True)
            retained=output/'run-0/native-library-result.json'
            retained_value=json.loads(retained.read_text()) if retained.exists() else None
            yield proc,retained_value,output,source,retained

    def record(self, passed=True):
        return {'passed':passed,'app':'/Users/runner/work/Chat Hub.app','receipt_counts':{'imported':1,'updated':0,'unchanged':0,'failed':0},
                'canonical_id':'conversation:["chatgpt","provisional:default","synthetic-export-one"]','library_revision':7,'fts_anchor':True,
                'ui':{'ready':True,'row':True,'selected':True,'alternate':True,'anchor':True,'revision':7,'node_id':'node_1','leaf_id':'leaf_2'},
                'same_pid_reopen':{'passed':True,'process_id':12},'old_process_closed':True,'old_listeners_closed':True,
                'final_native_pid_closed':13,'final_listeners_closed':True,
                'restart':{'old_pid':12,'new_pid':13,'service_pid':13,'version_matches_bundle':True,'executable_matches_bundle':True,'default_data_matches_platform':True,'persisted_reader':True,
                           'ui':{'ready':True,'row':True,'selected':True,'alternate':True,'anchor':True,'revision':7,'node_id':'node_1','leaf_id':'leaf_2','canonical_id':'conversation:["chatgpt","provisional:default","synthetic-export-one"]','selected_leaf_id':'leaf_2','alternate_node_id':'node_2'}}, 
                'primary_error':'native smoke did not complete' if not passed else ''}

    def test_success_and_failure_records_retained(self):
        for passed in (True,False):
            with self.run_case(self.record(passed)) as (proc,out,output,source,retained):
                self.assertEqual(proc.returncode,0,proc.stderr); self.assertIs(out['passed'],passed)
                self.assertTrue(output.is_dir()); self.assertTrue(source.is_dir()); self.assertTrue(retained.is_file())
                self.assertEqual(retained.read_bytes(),(json.dumps(out,indent=2)+'\n').encode())
                if passed:
                    self.assertEqual(out['canonical_id'],'conversation:["chatgpt","provisional:default","synthetic-export-one"]')
                    self.assertEqual(out['app'],'Chat Hub.app')
                    self.assertEqual(out['restart']['ui'],self.record()['restart']['ui'])
                    self.assertEqual(out['same_pid_reopen'],{'passed':True,'process_id':12})

    def test_excludes_command_raw_and_sensitive_neighbors(self):
        secret='Bearer top-secret export raw-body database private-key'
        names=('x.cmd.json','x.res.json','export.json','db.sqlite','ca.pem','app.log')
        with self.run_case(self.record(), extras=[(name,secret) for name in names]) as (proc,out,output,source,retained):
            self.assertEqual(proc.returncode,0,proc.stderr); self.assertIsNotNone(out)
            self.assertTrue(output.is_dir()); self.assertTrue(retained.is_file())
            self.assertEqual(retained.read_bytes(),(json.dumps(out,indent=2)+'\n').encode())
            self.assertTrue(all((source/name).is_file() for name in names))
            for name in names:
                self.assertFalse((output/'run-0'/name).exists(),name)
            self.assertNotIn('top-secret',json.dumps(out))

    def test_unknown_sensitive_field_rejected_without_leak(self):
        value=self.record(); value['restart']['access_token']='top-secret'
        with self.run_case(value) as (proc,out,*_):
            self.assertNotEqual(proc.returncode,0); self.assertIsNone(out)
            self.assertNotIn('top-secret',proc.stdout+proc.stderr)
            self.assertIn('invalid native-library result',proc.stderr)

    def test_missing_result_and_neighbor_logs_fail_closed_when_native_source_declared(self):
        with tempfile.TemporaryDirectory() as temp:
            base=pathlib.Path(temp); source=base/'chat-hub-native-library-evidence'; source.mkdir(); (source/'app.log').write_text('secret')
            output=base/'out'
            proc=subprocess.run([sys.executable,str(SCRIPT),str(output),str(source),'--native-library-source',str(source)],capture_output=True,text=True)
            self.assertNotEqual(proc.returncode,0); self.assertFalse((output/'run-0/app.log').exists())

    def test_valid_record_neighbor_raw_logs_never_copied(self):
        with self.run_case(self.record(),extras=[('app.log','Bearer secret')]) as (proc,out,output,source,retained):
            self.assertEqual(proc.returncode,0,proc.stderr); self.assertTrue(output.is_dir())
            self.assertTrue((source/'app.log').is_file()); self.assertTrue(retained.is_file())
            self.assertEqual(retained.read_bytes(),(json.dumps(out,indent=2)+'\n').encode())
            self.assertFalse((output/'run-0/app.log').exists())

    def test_serialized_expansion_over_limit_fails_before_write(self):
        value={'passed':False,'primary_error':'x'*180}
        raw=json.dumps(value)
        import runpy
        module=runpy.run_path(str(SCRIPT))
        with tempfile.TemporaryDirectory() as temp:
            base=pathlib.Path(temp); src=base/'native-library-result.json'; dest=base/'out.json'
            src.write_text(raw)
            # Compact input fits, but indentation makes the sanitized output exceed this cap.
            with mock.patch.dict(module['_retain_native'].__globals__, {'NATIVE_MAX':len(raw.encode('utf-8'))+1}):
                with self.assertRaisesRegex(RuntimeError, 'invalid native-library result'):
                    module['_retain_native'](src,dest)
            self.assertFalse(dest.exists())

    def test_invalid_types_malformed_oversized_and_symlink_fail_closed(self):
        cases=[dict(self.record(),passed=1),dict(self.record(),ui=[])]
        for value in cases:
            with self.run_case(value) as (proc,out,*_):
                self.assertNotEqual(proc.returncode,0); self.assertIsNone(out)
        for args in ({'raw':'{'}, {'raw':' '* (64*1024+1)}, {'symlink':True}):
            with self.run_case(**args) as (proc,out,*_):
                self.assertNotEqual(proc.returncode,0); self.assertIsNone(out)

if __name__=='__main__': unittest.main()
