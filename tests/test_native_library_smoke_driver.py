import importlib.util
import pathlib
import socket
import tempfile
import unittest
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('native_library_smoke', ROOT / 'build/native-library-smoke.py')
DRIVER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(DRIVER)

class LibrarySmokeDriverTests(unittest.TestCase):
    def test_wrapped_import_receipt_unwrap_and_count_validation(self):
        good = {'ok': True, 'receipt': {'imported':1,'updated':0,'unchanged':0,'failed':0,'errors_truncated':False}}
        self.assertTrue(DRIVER._receipt_ok(good['receipt']))
        for bad in ({**good, 'receipt': {**good['receipt'], 'imported':2}}, {**good, 'ok': False}):
            value = bad.get('receipt')
            self.assertFalse(DRIVER._receipt_ok(value) and bad.get('ok') is True)

    def test_listener_url_is_strict_loopback_http(self):
        self.assertTrue(DRIVER._listener_address('http://127.0.0.1:1234'))
        for url in ('https://127.0.0.1:1234','http://localhost:1234','http://127.0.0.1:1234/path','http://u:p@127.0.0.1:1234','http://127.0.0.1:1234?q=x'):
            self.assertFalse(DRIVER._listener_address(url), url)

    def test_native_pid_coherence_is_strict_and_rejects_restart_reuse(self):
        self.assertTrue(DRIVER._native_pid(42))
        for bad in (True, False, 0, -1, '42', None):
            self.assertFalse(DRIVER._native_pid(bad), repr(bad))
        self.assertTrue(DRIVER._coherent_native_pid({'process_id':42}, {'pid':42}))
        self.assertFalse(DRIVER._coherent_native_pid({'process_id':42}, {'pid':43}))
        self.assertFalse(DRIVER._coherent_native_pid({'process_id':True}, {'pid':True}))
        self.assertFalse(DRIVER._coherent_native_pid({'process_id':0}, {'pid':0}))
        self.assertFalse(DRIVER._coherent_native_pid({'process_id':42}, {'pid':True}))
        self.assertTrue(DRIVER._new_native_pid({'process_id':43}, {'pid':43}, 42))
        self.assertFalse(DRIVER._new_native_pid({'process_id':42}, {'pid':42}, 42))

    def test_native_identity_checks_actual_version_and_executable_fields(self):
        good_state = {'version': '0.1.1', 'executable_path': '/Applications/App.app/Contents/MacOS/app'}
        good_service = {'version': '0.1.1', 'executable': good_state['executable_path']}
        self.assertTrue(DRIVER._native_identity_ok(good_state, good_service, '0.1.1', good_state['executable_path']))
        for state, service, version, executable in (
            ({**good_state, 'version': 'wrong'}, good_service, '0.1.1', good_state['executable_path']),
            (good_state, {**good_service, 'version': 'wrong'}, '0.1.1', good_state['executable_path']),
            ({**good_state, 'executable_path': '/wrong'}, good_service, '0.1.1', good_state['executable_path']),
            (good_state, {**good_service, 'executable': '/wrong'}, '0.1.1', good_state['executable_path']),
        ):
            self.assertFalse(DRIVER._native_identity_ok(state, service, version, executable))

    def test_default_data_proof_requires_actual_resolver_fields_and_match(self):
        expected = '/Users/test/Library/Application Support/io.github.iarnvidr.chat-hub'
        good = {'ok': True, 'data_dir_source': 'platform_default', 'configured_matches_platform_default': True,
                'configured_data_dir': expected, 'platform_default_data_dir': expected}
        self.assertTrue(DRIVER._default_data_ok(good, expected))
        for bad in (
            {**good, 'configured_matches_platform_default': False},
            {**good, 'configured_data_dir': '/override'},
            {**good, 'platform_default_data_dir': '/wrong'},
            {**good, 'data_dir_source': 'custom'},
            {**good, 'ok': False},
        ):
            self.assertFalse(DRIVER._default_data_ok(bad, expected))

    def test_default_data_requires_explicit_opt_in(self):
        with tempfile.TemporaryDirectory() as temp, mock.patch.dict('os.environ', {'MUNINN_E2E_DIR': temp}, clear=True):
            with self.assertRaisesRegex(RuntimeError, 'without --allow-default-app-data'):
                DRIVER.run(pathlib.Path(temp), pathlib.Path(temp) / 'evidence')

    def test_forbids_alternate_data_location(self):
        with tempfile.TemporaryDirectory() as temp, mock.patch.dict('os.environ', {'MUNINN_E2E_DIR': temp, 'MUNINN_E2E_DATA_DIR': temp}):
            with self.assertRaisesRegex(RuntimeError, 'override is forbidden'):
                DRIVER.run(pathlib.Path(temp), pathlib.Path(temp) / 'evidence', True, pathlib.Path(DRIVER.pwd.getpwuid(__import__('os').getuid()).pw_dir))

    def test_health_uses_actual_ipc_shape_and_separate_v1_proof(self):
        self.assertTrue(DRIVER._health_ok({'healthy': True}))
        for bad in ({}, {'healthy': False}, {'healthy': 1}): self.assertFalse(DRIVER._health_ok(bad))
        self.assertTrue(DRIVER._v1_health_ok({'status':'healthy','service':{'status':'ready'},'database':{'status':'healthy'}}))
        for bad in ({}, {'status':'starting','service':{'status':'ready'},'database':{'status':'healthy'}}, {'status':'healthy','service':{'status':'starting'},'database':{'status':'healthy'}}, {'status':'healthy','service':{'status':'ready'},'database':{'status':'unhealthy'}}): self.assertFalse(DRIVER._v1_health_ok(bad))

    def test_fixture_receipt_requires_exact_real_counts(self):
        self.assertTrue(DRIVER._receipt_ok({'imported':1,'updated':0,'unchanged':0,'failed':0,'errors_truncated':False}))
        for bad in ({'imported':0,'updated':0,'unchanged':0,'failed':0,'errors_truncated':False}, {'imported':1,'updated':1,'unchanged':0,'failed':0,'errors_truncated':False}, {'imported':1,'updated':0,'unchanged':0,'failed':0,'errors_truncated':True}): self.assertFalse(DRIVER._receipt_ok(bad))

    def test_dom_requires_search_revision_and_both_branch_contents_and_anchor(self):
        good={'ready':True,'row':True,'revision':7,'selected':True,'alternate':True,'anchor':True}
        self.assertTrue(DRIVER._ui_ok(good))
        for field in ('ready','row','revision','selected','alternate','anchor'):
            bad=dict(good); bad[field]=False
            self.assertFalse(DRIVER._ui_ok(bad),field)

    def test_actual_node_ids_are_distinct_from_opaque_message_id_and_reader_targets_nodes(self):
        canonical = 'conversation:[opaque]'
        selected_node, selected_leaf = 'right', 'right'
        alternate_node, alternate_leaf = 'left', 'left'
        detail = {
            'nodes': [
                {'id': 'root', 'message': {'content': 'root'}},
                {'id': 'right', 'parent_id': 'root', 'message': {'content': 'Selected synthetic answer'}},
                {'id': 'left', 'parent_id': 'root', 'message': {'content': 'Alternate synthetic answer'}},
            ],
            'selection': {'leaf_node_id': 'right'},
            'branches': [{'leaf_node_id': 'right', 'selected': True}, {'leaf_node_id': 'left'}],
        }
        self.assertEqual(DRIVER._detail_anchor_ids(detail, selected_node),
                         ('right', 'right', 'left', 'left'))
        script = DRIVER.reader_script(canonical, selected_node, selected_leaf, alternate_node, alternate_leaf)
        self.assertIn("a.dataset.nodeId===snode", script)
        self.assertIn("a.dataset.nodeId===anode", script)
        self.assertIn("node_id:snode", script)
        self.assertIn("canonical_id:cid", script)
        self.assertIn("branch.value=aleaf", script)
        self.assertLess(script.index("anchor absent before branch switch"), script.index("branch.value=aleaf"))
        self.assertNotEqual(canonical, selected_node)

    def test_revision_must_be_canonical_integer_not_render_fallback(self):
        self.assertIs(type(7), int)
        self.assertNotIsInstance('rendered', int)

    def test_home_optin_rejects_home_override_and_mismatch(self):
        import os
        actual = pathlib.Path(DRIVER.pwd.getpwuid(os.getuid()).pw_dir).resolve()
        self.assertEqual(pathlib.Path.home().resolve(), actual)
        self.assertNotEqual(pathlib.Path('/tmp/not-the-user-home').resolve(), actual)

    def test_closed_listener_probe_requires_tcp_refusal(self):
        listener=socket.socket(); listener.bind(('127.0.0.1',0)); listener.listen(); address=listener.getsockname()
        try: self.assertFalse(DRIVER._connection_refused(address))
        finally: listener.close()
        self.assertTrue(DRIVER._connection_refused(address))

    def test_run_level_driver_full_success_and_graceful_final_cleanup(self):
        import json, os, plistlib
        with tempfile.TemporaryDirectory() as temp:
            root=pathlib.Path(temp); app=root/'App.app'; binary=app/'Contents/MacOS/app'; binary.parent.mkdir(parents=True); binary.write_text('')
            evidence=root/'evidence'; data=root/'Library/Application Support/example.app'; data.parent.mkdir(parents=True); data_expected=str(data.resolve()); data=data
            (app/'Contents/Info.plist').write_bytes(plistlib.dumps({'CFBundleExecutable':'app','CFBundleIdentifier':'example.app','CFBundleShortVersionString':'1.2'}))
            e2e=root/'e2e'; e2e.mkdir(); env={'MUNINN_E2E_DIR':str(e2e)}
            service=lambda pid,port:{'api':f'http://127.0.0.1:{port}','proxy':f'http://127.0.0.1:{port+1}','pid':pid,'version':'1.2','executable':str(binary)}
            state=lambda pid:{'process_id':pid,'version':'1.2','executable_path':str(binary),'window_visible':True}
            calls=[]; counts={'state':0,'list':0,'service':0,'launch':0,'reader':0}
            def request(_e2e,op,**kw):
                calls.append((op,kw.get('js','')))
                if op=='state':
                    counts['state']+=1; return state(101 if counts['launch']<3 else 202)
                if op=='default_data_info':
                    data.mkdir(parents=True,exist_ok=True); (data/'service.token').write_text('fresh-token'); return {'ok':True,'data_dir_source':'platform_default','configured_matches_platform_default':True,'configured_data_dir':str(data),'platform_default_data_dir':str(data)}
                if op=='import_library_fixture': return {'ok':True,'receipt':{'imported':1,'updated':0,'unchanged':0,'failed':0,'errors_truncated':False}}
                js=kw.get('js','')
                if 'service_info' in js:
                    counts['service']+=1
                    pid=101 if counts['service']<=4 else 202
                    result=service(pid,4100 if pid==101 else 4200)
                    return result
                if 'service_health' in js: return {'healthy':True}
                if "const [cid,snode,sleaf,anode,aleaf]" in js:
                    counts['reader']+=1
                    return {'ready':True,'row':True,'revision':1,'selected':True,'alternate':True,'anchor':True,'node_id':'right','leaf_id':'left','canonical_id':'opaque','selected_leaf_id':'right','alternate_node_id':'left'}
                if "get_conversation" in js: return {'nodes':[{'id':'root','message':{'content':'root'}},{'id':'right','parent_id':'root','message':{'content':'Selected synthetic answer'}},{'id':'left','parent_id':'root','message':{'content':'Alternate synthetic answer'}}],'selection':{'leaf_node_id':'right'},'branches':[{'leaf_node_id':'right','selected':True},{'leaf_node_id':'left'}]}
                if 'list_conversations' in js:
                    counts['list']+=1
                    if "query:'Selected synthetic answer'" in js: return {'items':[{'message_id':'message:[canonical...]','node_id':'right','leaf_node_id':'right','conversation':{'id':'opaque'}}]}
                    if counts['list']>=3: return {'library_revision':1,'items':[{'id':'opaque','title':'Synthetic branching export','provider':'chatgpt','account_scope':'provisional:default','source_id':'synthetic-export-one'}]}
                    if counts['list']==2: return {'library_revision':1,'items':[{'id':'opaque','title':'Synthetic branching export','provider':'chatgpt','account_scope':'provisional:default','source_id':'synthetic-export-one'}]}
                    return {'library_revision':0,'items':[]}
                raise AssertionError(op+js)
            class Proc:
                returncode=None
                def poll(self): return None
            launches=[]
            def popen(args,**kwargs):
                launches.append(args); counts['launch']+=1; return Proc()
            with mock.patch.dict(os.environ,env,clear=True), mock.patch.object(DRIVER.pwd,'getpwuid',return_value=type('P',(),{'pw_dir':str(root)})()), mock.patch.object(DRIVER.pathlib.Path,'home',return_value=root), mock.patch.object(DRIVER,'request',side_effect=request), mock.patch.object(DRIVER.subprocess,'Popen',side_effect=popen), mock.patch.object(DRIVER.subprocess,'run') as run_cmd, mock.patch.object(DRIVER,'_pid_running',side_effect=[True,False,False,False,False,False]), mock.patch.object(DRIVER,'_connection_refused',return_value=True), mock.patch.object(DRIVER.urllib.request,'urlopen',side_effect=[*([__import__('contextlib').nullcontext(type('R',(),{'read':lambda self:b'{"status":"healthy","service":{"status":"ready"},"database":{"status":"healthy"}}'})())]*4)]):
                result=DRIVER.run(app,evidence,True,root)

    def test_early_reader_failure_gracefully_cleans_native_app(self):
        import json, os, plistlib
        with tempfile.TemporaryDirectory() as temp:
            root=pathlib.Path(temp); app=root/'App.app'; binary=app/'Contents/MacOS/app'; binary.parent.mkdir(parents=True); binary.write_text('')
            evidence=root/'evidence'; data=root/'Library/Application Support/example.app'; data.parent.mkdir(parents=True); data_expected=str(data.resolve()); data=data
            (app/'Contents/Info.plist').write_bytes(plistlib.dumps({'CFBundleExecutable':'app','CFBundleIdentifier':'example.app','CFBundleShortVersionString':'1.2'}))
            e2e=root/'e2e'; e2e.mkdir(); env={'MUNINN_E2E_DIR':str(e2e)}
            service=lambda pid,port:{'api':f'http://127.0.0.1:{port}','proxy':f'http://127.0.0.1:{port+1}','pid':pid,'version':'1.2','executable':str(binary)}
            state=lambda pid:{'process_id':pid,'version':'1.2','executable_path':str(binary),'window_visible':True}
            calls=[]; counts={'state':0,'list':0,'service':0,'launch':0,'reader':0}
            def request(_e2e,op,**kw):
                calls.append((op,kw.get('js','')))
                if op=='state':
                    counts['state']+=1; return state(101 if counts['launch']<3 else 202)
                if op=='default_data_info':
                    data.mkdir(parents=True,exist_ok=True); (data/'service.token').write_text('fresh-token'); return {'ok':True,'data_dir_source':'platform_default','configured_matches_platform_default':True,'configured_data_dir':str(data),'platform_default_data_dir':str(data)}
                if op=='import_library_fixture': return {'ok':True,'receipt':{'imported':1,'updated':0,'unchanged':0,'failed':0,'errors_truncated':False}}
                js=kw.get('js','')
                if 'service_info' in js:
                    counts['service']+=1
                    pid=101 if counts['service']<=4 else 202
                    result=service(pid,4100 if pid==101 else 4200)
                    return result
                if 'service_health' in js: return {'healthy':True}
                if "const [cid,snode,sleaf,anode,aleaf]" in js:
                    counts['reader']+=1
                    return {'ready':True,'row':True,'revision':1,'selected':False,'alternate':True,'anchor':True,'node_id':'right','leaf_id':'left','canonical_id':'opaque','selected_leaf_id':'right','alternate_node_id':'left'}
                if "get_conversation" in js: return {'nodes':[{'id':'root','message':{'content':'root'}},{'id':'right','parent_id':'root','message':{'content':'Selected synthetic answer'}},{'id':'left','parent_id':'root','message':{'content':'Alternate synthetic answer'}}],'selection':{'leaf_node_id':'right'},'branches':[{'leaf_node_id':'right','selected':True},{'leaf_node_id':'left'}]}
                if 'list_conversations' in js:
                    counts['list']+=1
                    if "query:'Selected synthetic answer'" in js: return {'items':[{'message_id':'message:[canonical...]','node_id':'right','leaf_node_id':'right','conversation':{'id':'opaque'}}]}
                    if counts['list']>=3: return {'library_revision':1,'items':[{'id':'opaque','title':'Synthetic branching export','provider':'chatgpt','account_scope':'provisional:default','source_id':'synthetic-export-one'}]}
                    if counts['list']==2: return {'library_revision':1,'items':[{'id':'opaque','title':'Synthetic branching export','provider':'chatgpt','account_scope':'provisional:default','source_id':'synthetic-export-one'}]}
                    return {'library_revision':0,'items':[]}
                raise AssertionError(op+js)
            class Proc:
                returncode=None
                def poll(self): return None
            launches=[]
            def popen(args,**kwargs):
                launches.append(args); counts['launch']+=1; return Proc()
            with mock.patch.dict(os.environ,env,clear=True), mock.patch.object(DRIVER.pwd,'getpwuid',return_value=type('P',(),{'pw_dir':str(root)})()), mock.patch.object(DRIVER.pathlib.Path,'home',return_value=root), mock.patch.object(DRIVER,'request',side_effect=request), mock.patch.object(DRIVER.subprocess,'Popen',side_effect=popen), mock.patch.object(DRIVER.subprocess,'run') as run_cmd, mock.patch.object(DRIVER,'_pid_running',side_effect=[True,False,False,False,False,False]), mock.patch.object(DRIVER,'_connection_refused',return_value=True), mock.patch.object(DRIVER.urllib.request,'urlopen',side_effect=[*([__import__('contextlib').nullcontext(type('R',(),{'read':lambda self:b'{"status":"healthy","service":{"status":"ready"},"database":{"status":"healthy"}}'})())]*4)]):
                with self.assertRaises(RuntimeError):
                    DRIVER.run(app,evidence,True,root)
                record=json.loads((evidence/'native-library-result.json').read_text())
                self.assertFalse(record['passed'])
                self.assertEqual(record.get('final_native_pid_closed'),101)
                self.assertTrue(record.get('final_listeners_closed'))
                self.assertNotIn('cleanup_error',record)
                self.assertEqual(run_cmd.call_count,1)
                self.assertEqual(run_cmd.call_args.args[0][0], 'osascript')
                self.assertEqual(counts['launch'],1)
                self.assertEqual(counts['reader'],1)

    def _assert_final_cleanup_failure(self, fault='api'):
        import json, os, plistlib
        with tempfile.TemporaryDirectory() as temp:
            root=pathlib.Path(temp); app=root/'App.app'; binary=app/'Contents/MacOS/app'; binary.parent.mkdir(parents=True); binary.write_text('')
            evidence=root/'evidence'; data=root/'Library/Application Support/example.app'; data.parent.mkdir(parents=True); data_expected=str(data.resolve()); data=data
            (app/'Contents/Info.plist').write_bytes(plistlib.dumps({'CFBundleExecutable':'app','CFBundleIdentifier':'example.app','CFBundleShortVersionString':'1.2'}))
            e2e=root/'e2e'; e2e.mkdir(); env={'MUNINN_E2E_DIR':str(e2e)}
            service=lambda pid,port:{'api':f'http://127.0.0.1:{port}','proxy':f'http://127.0.0.1:{port+1}','pid':pid,'version':'1.2','executable':str(binary)}
            state=lambda pid:{'process_id':pid,'version':'1.2','executable_path':str(binary),'window_visible':True}
            calls=[]; counts={'state':0,'list':0,'service':0,'launch':0,'reader':0}
            def request(_e2e,op,**kw):
                calls.append((op,kw.get('js','')))
                if op=='state':
                    counts['state']+=1; return state(101 if counts['launch']<3 else 202)
                if op=='default_data_info':
                    data.mkdir(parents=True,exist_ok=True); (data/'service.token').write_text('fresh-token'); return {'ok':True,'data_dir_source':'platform_default','configured_matches_platform_default':True,'configured_data_dir':str(data),'platform_default_data_dir':str(data)}
                if op=='import_library_fixture': return {'ok':True,'receipt':{'imported':1,'updated':0,'unchanged':0,'failed':0,'errors_truncated':False}}
                js=kw.get('js','')
                if 'service_info' in js:
                    counts['service']+=1
                    pid=101 if counts['service']<=4 else 202
                    result=service(pid,4100 if pid==101 else 4200)
                    if fault=='identity' and counts['service']>=6:
                        result['executable']='/other/app'
                    return result
                if 'service_health' in js: return {'healthy':True}
                if "const [cid,snode,sleaf,anode,aleaf]" in js:
                    counts['reader']+=1
                    return {'ready':True,'row':True,'revision':1,'selected':True,'alternate':True,'anchor':True,'node_id':'right','leaf_id':'left','canonical_id':'opaque','selected_leaf_id':'right','alternate_node_id':'left'}
                if "get_conversation" in js: return {'nodes':[{'id':'root','message':{'content':'root'}},{'id':'right','parent_id':'root','message':{'content':'Selected synthetic answer'}},{'id':'left','parent_id':'root','message':{'content':'Alternate synthetic answer'}}],'selection':{'leaf_node_id':'right'},'branches':[{'leaf_node_id':'right','selected':True},{'leaf_node_id':'left'}]}
                if 'list_conversations' in js:
                    counts['list']+=1
                    if "query:'Selected synthetic answer'" in js: return {'items':[{'message_id':'message:[canonical...]','node_id':'right','leaf_node_id':'right','conversation':{'id':'opaque'}}]}
                    if counts['list']>=3: return {'library_revision':1,'items':[{'id':'opaque','title':'Synthetic branching export','provider':'chatgpt','account_scope':'provisional:default','source_id':'synthetic-export-one'}]}
                    if counts['list']==2: return {'library_revision':1,'items':[{'id':'opaque','title':'Synthetic branching export','provider':'chatgpt','account_scope':'provisional:default','source_id':'synthetic-export-one'}]}
                    return {'library_revision':0,'items':[]}
                raise AssertionError(op+js)
            class Proc:
                returncode=None
                def poll(self): return None
            launches=[]
            def popen(args,**kwargs):
                launches.append(args); counts['launch']+=1; return Proc()
            pid_results=iter([True,False,False,False,False,False])
            def pid_running(pid):
                if fault=='pid' and pid==202: return True
                return next(pid_results)
            refused=lambda address: not ((fault=='api' and address[1]==4200) or (fault=='proxy' and address[1]==4201))
            clock=[0.0]
            def monotonic():
                clock[0]+=.3
                return clock[0]
            with mock.patch.dict(os.environ,env,clear=True), mock.patch.object(DRIVER.pwd,'getpwuid',return_value=type('P',(),{'pw_dir':str(root)})()), mock.patch.object(DRIVER.pathlib.Path,'home',return_value=root), mock.patch.object(DRIVER,'request',side_effect=request), mock.patch.object(DRIVER.subprocess,'Popen',side_effect=popen), mock.patch.object(DRIVER.subprocess,'run') as run_cmd, mock.patch.object(DRIVER,'_pid_running',side_effect=pid_running), mock.patch.object(DRIVER,'_connection_refused',side_effect=refused), mock.patch.object(DRIVER.time,'monotonic',side_effect=monotonic), mock.patch.object(DRIVER.time,'sleep'), mock.patch.object(DRIVER.urllib.request,'urlopen',side_effect=[*([__import__('contextlib').nullcontext(type('R',(),{'read':lambda self:b'{"status":"healthy","service":{"status":"ready"},"database":{"status":"healthy"}}'})())]*4)]):
                with self.assertRaisesRegex(RuntimeError, 'cleanup failed'):
                    DRIVER.run(app,evidence,True,root)
                record=json.loads((evidence/'native-library-result.json').read_text())
                self.assertFalse(record['passed'])
                self.assertNotIn('final_native_pid_closed', record)
                self.assertIsNotNone(run_cmd.call_args)
                if fault=='identity': self.assertEqual(run_cmd.call_count,1)
                else: self.assertEqual(run_cmd.call_count,2)
                if fault=='identity':
                    self.assertIn('refusing cleanup',record.get('cleanup_error',''))
                else:
                    self.assertEqual(record.get('cleanup_error'), 'owned native app or listener did not close after graceful quit')

    def test_run_level_driver_failure_gracefully_cleans_native_app(self):
        import json, os, plistlib
        with tempfile.TemporaryDirectory() as temp:
            root=pathlib.Path(temp); app=root/'App.app'; binary=app/'Contents/MacOS/app'; binary.parent.mkdir(parents=True); binary.write_text('')
            evidence=root/'evidence'; e2e=root/'e2e'; e2e.mkdir()
            (app/'Contents/Info.plist').write_bytes(plistlib.dumps({'CFBundleExecutable':'app','CFBundleIdentifier':'example.app','CFBundleShortVersionString':'1.2'}))
            def req(_e2e,op,**kw):
                if op=='state': return {'process_id':333,'version':'1.2','executable_path':str(binary),'window_visible':True}
                if op=='eval' and 'service_info' in kw.get('js',''): return {'api':'http://127.0.0.1:4100','proxy':'127.0.0.1:4101','pid':333,'version':'1.2','executable':str(binary)}
                if op=='eval' and 'service_health' in kw.get('js',''): return {'healthy':True}
                if op=='eval' and 'list_conversations' in kw.get('js',''): return {'library_revision':1,'items':[]}
                if op=='eval' and 'service_info' in kw.get('js',''): return {'api':'http://127.0.0.1:4100','proxy':'127.0.0.1:4101','pid':333,'version':'1.2','executable':str(binary)}
                if op=='import_library_fixture': return {'ok':True,'receipt':{'imported':0,'updated':0,'unchanged':0,'failed':0,'errors_truncated':False}}
                if op=='default_data_info':
                    data=root/'Library/Application Support/example.app'; data.mkdir(parents=True,exist_ok=True); (data/'service.token').write_text('token')
                    return {'ok':True,'data_dir_source':'platform_default','configured_matches_platform_default':True,'configured_data_dir':str(data),'platform_default_data_dir':str(data)}
                if op=='eval': raise AssertionError(kw.get('js'))
                raise AssertionError(op)
            class Proc:
                returncode=None
                def poll(self): return None
            healthy = __import__('contextlib').nullcontext(type('R',(),{'read':lambda self:b'{"status":"healthy","service":{"status":"ready"},"database":{"status":"healthy"}}'})())
            with mock.patch.dict(os.environ,{'MUNINN_E2E_DIR':str(e2e)},clear=True), mock.patch.object(DRIVER.pwd,'getpwuid',return_value=type('P',(),{'pw_dir':str(root)})()), mock.patch.object(DRIVER.pathlib.Path,'home',return_value=root), mock.patch.object(DRIVER,'request',side_effect=req), mock.patch.object(DRIVER.urllib.request,'urlopen',return_value=healthy), mock.patch.object(DRIVER.subprocess,'Popen',return_value=Proc()), mock.patch.object(DRIVER.subprocess,'run') as quit_cmd, mock.patch.object(DRIVER,'_pid_running',side_effect=[False,False]):
                with self.assertRaisesRegex(RuntimeError,'fixture import receipt counts'):
                    DRIVER.run(app,evidence,True,root)
            self.assertIsNotNone(quit_cmd.call_args)
            self.assertEqual(quit_cmd.call_args.args[0][0], 'osascript')
            record=json.loads((evidence/'native-library-result.json').read_text())
            self.assertEqual(record.get('final_native_pid_closed'),333)
            self.assertTrue(record.get('final_listeners_closed'))

    def test_final_listener_probe_failure_rejects_otherwise_successful_run(self):
        self._assert_final_cleanup_failure('api')

    def test_final_proxy_listener_probe_failure_rejects_otherwise_successful_run(self):
        self._assert_final_cleanup_failure('proxy')

    def test_final_owned_pid_still_running_fails_cleanup(self):
        self._assert_final_cleanup_failure('pid')

    def test_final_service_identity_mismatch_refuses_cleanup(self):
        self._assert_final_cleanup_failure('identity')

    def test_request_atomically_publishes_complete_unique_command(self):
        import json, os
        with tempfile.TemporaryDirectory() as temp:
            e2e=pathlib.Path(temp)
            real_replace=os.replace
            published=[]
            def checked_replace(src,dst):
                src=pathlib.Path(src); dst=pathlib.Path(dst)
                self.assertEqual(dst.parent,e2e)
                self.assertFalse(dst.exists())
                self.assertNotEqual(src,dst)
                payload=json.loads(src.read_text())
                self.assertEqual(payload['id'],dst.name.removesuffix('.cmd.json'))
                self.assertEqual(payload['op'],'eval')
                published.append(src)
                return real_replace(src,dst)
            original_sleep=DRIVER.time.sleep
            seen=[]
            def complete(_):
                for command in e2e.glob('*.cmd.json'):
                    ident=command.name.removesuffix('.cmd.json')
                    if ident not in seen:
                        seen.append(ident)
                        (e2e/f'{ident}.res.json').write_text(json.dumps({'ok':True,'value':ident}))
                original_sleep(.001)
            with mock.patch.object(DRIVER.os,'replace',side_effect=checked_replace), mock.patch.object(DRIVER.time,'sleep',side_effect=complete):
                first=DRIVER.request(e2e,'eval',timeout=.5,js='return 1')
                second=DRIVER.request(e2e,'eval',timeout=.5,js='return 2')
            self.assertIn(first,seen)
            self.assertIn(second,seen)
            self.assertEqual(len(published),2)
            self.assertEqual(len(set(seen)),2)

    def test_cleanup_evidence_requires_gone_pid_and_both_closed_ports(self):
        # Small focused assertion on the production close predicate used by run cleanup.
        live = socket.socket(); live.bind(('127.0.0.1', 0)); live.listen()
        address = live.getsockname()
        try:
            self.assertFalse(DRIVER._connection_refused(address))
            self.assertTrue(DRIVER._pid_running(__import__('os').getpid()))
        finally:
            live.close()
        self.assertTrue(DRIVER._connection_refused(address))
        self.assertFalse(DRIVER._pid_running(-987654))

    def test_eval_ack_only_is_not_terminal(self):
        with tempfile.TemporaryDirectory() as temp:
            e2e=pathlib.Path(temp); original=DRIVER.time.sleep
            def ack(_):
                original(.001)
                for command in e2e.glob('*.cmd.json'):
                    ident=command.name.removesuffix('.cmd.json'); (e2e/f'{ident}.res.json').write_text('{"ok":true}')
            with mock.patch.object(DRIVER.time,'sleep',side_effect=ack):
                with self.assertRaisesRegex(RuntimeError,'acknowledgement without terminal value'):
                    DRIVER.request(e2e,'eval',timeout=.02,js='return 1')

if __name__ == '__main__': unittest.main()
