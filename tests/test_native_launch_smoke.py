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
if __name__=='__main__': unittest.main()
