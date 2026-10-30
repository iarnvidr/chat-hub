"""Behavioral tests extract the production retry functions without launching macOS."""
import ast, pathlib, unittest
P=pathlib.Path(__file__).with_name('native-launch-driver.py')
tree=ast.parse(P.read_text())
names={'_run_eval'}
nodes=[n for n in tree.body if isinstance(n,(ast.FunctionDef,ast.AsyncFunctionDef)) and n.name in names]
code='import time\nEVAL_DEADLINE_SECONDS=90\nEVAL_MAX_ATTEMPTS=4\nEVAL_ATTEMPT_SECONDS=20\nrecord={"eval_attempts":[]}\ndef _persist_attempts(): pass\ndef _wake_app(deadline): return "wake",0\n'+ast.unparse(nodes)
ns={}; exec(code,ns)
class RetryBehavior(unittest.TestCase):
 def run_call(self, outcomes):
  ns['record']={'eval_attempts':[]}
  now=[0.0]; ids=[]; wakes=[]
  def clock(): return now[0]
  def wake(deadline): wakes.append(deadline); now[0]+=.1; return 'wake',100
  def call(op,**kw):
   ids.append(kw['_ident']); now[0]+=.2
   outcome=outcomes.pop(0)
   if outcome=='timeout': raise RuntimeError('IPC timed out')
   if outcome=='error': raise RuntimeError('JavaScript evaluation failed')
   return 'success'
  try: result=ns['_run_eval']('eval',{},clock=clock,wake=wake,call=call)
  except RuntimeError as e: result=e
  return result,ns['record']['eval_attempts'],ids,wakes
 def test_timeout_then_success_retries_with_unique_ids_and_wake(self):
  result,entries,ids,wakes=self.run_call(['timeout','success'])
  self.assertEqual(result,'success'); self.assertEqual([e['outcome'] for e in entries],['timeout','ok'])
  self.assertEqual(len(set(ids)),2); self.assertEqual(len(wakes),2); self.assertLess(max(e['elapsed_ms'] for e in entries),90000)
 def test_permanent_timeout_is_limited_to_four(self):
  result,entries,ids,wakes=self.run_call(['timeout']*4)
  self.assertIsInstance(result,RuntimeError); self.assertEqual(len(entries),4); self.assertEqual(len(ids),4); self.assertEqual(len(wakes),4)
 def test_non_timeout_error_fails_immediately(self):
  result,entries,ids,wakes=self.run_call(['error','success'])
  self.assertIsInstance(result,RuntimeError); self.assertEqual(len(entries),1); self.assertEqual(entries[0]['outcome'],'error')
if __name__=='__main__': unittest.main()
