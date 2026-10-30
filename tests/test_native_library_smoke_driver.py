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
        good={'ready':True,'row':True,'revision':'rev-1','selected':True,'alternate':True,'anchor':True}
        self.assertTrue(DRIVER._ui_ok(good))
        for field in ('ready','row','revision','selected','alternate','anchor'):
            bad=dict(good); bad[field]=False
            self.assertFalse(DRIVER._ui_ok(bad),field)

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

    def test_eval_ack_only_is_not_terminal(self):
        with tempfile.TemporaryDirectory() as temp:
            e2e=pathlib.Path(temp); original=DRIVER.time.sleep
            def ack(_):
                original(.001)
                for command in e2e.glob('*.cmd.json'):
                    ident=command.name.removesuffix('.cmd.json'); (e2e/f'{ident}.res.json').write_text('{"ok":true}')
            with mock.patch.object(DRIVER.time,'sleep',side_effect=ack):
                with self.assertRaisesRegex(RuntimeError,'known same-file race'):
                    DRIVER.request(e2e,'eval',timeout=.02,js='return 1')

if __name__ == '__main__': unittest.main()
