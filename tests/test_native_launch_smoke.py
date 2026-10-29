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
  self.assertIn("info2.get('api')!=info.get('api')",driver)
  self.assertIn("denial_result=bridge('provider_denial')",driver)
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
