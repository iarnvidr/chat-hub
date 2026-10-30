"""Regression contract for required native launch smoke gate."""
import pathlib, unittest
ROOT=pathlib.Path(__file__).resolve().parents[1]
class NativeLaunchSmoke(unittest.TestCase):
 def test_required_steps_are_blocking_in_both_workflows(self):
  for name in ('macos.yml','release.yml'):
   text=(ROOT/'.github/workflows'/name).read_text()
   self.assertIn('Required native',text)
   self.assertIn('build/native-launch-smoke.sh',text)
  self.assertIn('cargo build --locked', (ROOT/'.github/workflows/release.yml').read_text())
  for name in ('macos.yml','release.yml'):
   text=(ROOT/'.github/workflows'/name).read_text()
   self.assertIn('basic',text)
   self.assertIn('Advisory provider proxy capture',text)
   advisory=text.split('Advisory provider proxy capture',1)[1].split('      - name:',1)[0]
   self.assertIn('continue-on-error: true',advisory)
   self.assertIn('::warning::',advisory)
   self.assertIn('GITHUB_STEP_SUMMARY',advisory)
 def test_eval_retry_is_bounded_and_required_workflow_keeps_both_attempts(self):
  driver=(ROOT/'build/native-launch-driver.py').read_text()
  self.assertIn('EVAL_MAX_ATTEMPTS=4',driver)
  self.assertIn('EVAL_DEADLINE_SECONDS=90',driver)
  self.assertIn('EVAL_ATTEMPT_SECONDS=20',driver)
  self.assertIn("'timeout' if 'IPC timed out' in str(exc) else 'error'",driver)
  self.assertIn("if entry['outcome']!='timeout': raise",driver)
  self.assertIn("(call or _bridge_once)(op, _timeout=min(EVAL_ATTEMPT_SECONDS,remaining), _ident=ident",driver)
  self.assertIn("['open','-a',str(app)]",driver)
  self.assertIn("get unix id of first process whose frontmost is true",driver)
  workflow=(ROOT/'.github/workflows/macos.yml').read_text()
  basic=workflow.split('Required native launch, UI, API, update status, reopen, shutdown smoke',1)[1].split('      - name:',1)[0]
  self.assertIn('for attempt in 1 2',basic)
  self.assertIn('attempt-$attempt',basic)
  self.assertIn('exit "$final_rc"',basic)
  self.assertNotIn('continue-on-error',basic)
  release=(ROOT/'.github/workflows/release.yml').read_text()
  required=release.split('Required native UI, health, update status, relaunch, and shutdown smoke',1)[1].split('      - name:',1)[0]
  self.assertIn('for attempt in 1 2',required)
  self.assertIn('attempt-$attempt',required)
  self.assertIn('exit "$final_rc"',required)
  for text in (workflow,release):
   self.assertIn('prepare-native-evidence.py',text)
   self.assertIn('chat-hub-safe-evidence/**',text)
   self.assertNotIn('chat-hub-native-launch-evidence/**',text)
   self.assertNotIn('chat-hub-updater-evidence/**',text)
 def test_checks_real_dom_api_reopen_and_shutdown(self):
  driver=(ROOT/'build/native-launch-driver.py').read_text()
  for required in ('window.ChatHub.invoke','document.body.innerText','/api/health','Authorization','survival_seconds','osascript','ports_closed','screencapture','native-launch.png','service.token','native-launch-result.json'):
   self.assertIn(required,driver)
  self.assertNotIn("print(token",driver)
 def test_reopen_precedes_single_final_quit_and_checks_same_service(self):
  driver=(ROOT/'build/native-launch-driver.py').read_text()
  reopen=driver.index("subprocess.run(['open','-a',str(app)]")
  quit_call=driver.index("subprocess.run(['osascript','-e',f'tell application id")
  self.assertLess(reopen,quit_call)
  self.assertEqual(driver.count("to quit']"),1)
  self.assertIn("service_after_reopen.get('api')!=service.get('api')",driver)
  self.assertIn("service_after_reopen=bridge('eval',js=\"return await window.ChatHub.invoke('service_info');\")",driver)
  self.assertIn("ports_closed(service_after_reopen)",driver)
  self.assertLess(driver.index('service_after_reopen=bridge'),quit_call)
  self.assertLess(quit_call,driver.index('ports_closed(service_after_reopen)'))
  self.assertIn("denial_result=bridge('provider_denial')",driver)
  self.assertIn("MUNINN_NATIVE_LAUNCH_MODE",driver)
  self.assertIn("status=bridge('eval',js=\"return await window.ChatHub.invoke('update_status');\")",driver)
  self.assertIn("denial_result.get('denied') is not True",driver)
  self.assertIn("return proc.poll() is None",driver)
 def test_synthetic_tls_fixture_is_local_and_narrowly_claimed(self):
  harness=(ROOT/'build/native-launch-smoke.sh').read_text()
  fixture=(ROOT/'build/native-tls-fixture.py').read_text()
  driver=(ROOT/'build/native-launch-driver.py').read_text()
  self.assertIn('127.0.0.1',fixture)
  self.assertIn('subjectAltName=DNS:chatgpt.com',harness)
  self.assertIn('MUNINN_PROVIDER_SMOKE_UPSTREAM',harness)
  self.assertIn('wrap_socket(raw,server_hostname=\'chatgpt.com\')',driver)
  self.assertIn("info['api']+'/api/conversations/native-smoke-synthetic'",driver)
  self.assertIn('not ChatGPT login',driver)
if __name__=='__main__': unittest.main()
