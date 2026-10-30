import os, pathlib, subprocess, sys, tempfile, unittest
ROOT=pathlib.Path(__file__).resolve().parents[1]
class SmokeReportTests(unittest.TestCase):
 def test_app_log_uses_200_lines_and_generic_streams_80_with_caps(self):
  with tempfile.TemporaryDirectory() as d:
   p=pathlib.Path(d); app=p/'app.log'; other=p/'other.log'; evidence=p/'evidence'; evidence.mkdir()
   app.write_text(''.join(f'APP-{i:03d} '+('λ'*250)+'\n' for i in range(1,258)))
   other.write_text(''.join(f'GEN-{i:03d}\n' for i in range(1,101)))
   args=[sys.executable,str(ROOT/'build/native-smoke-report.py'),'16','1','test',str(evidence),f'{app}:{other}','']
   run=subprocess.run(args,capture_output=True,text=True,check=True)
   self.assertIn('APP-058',run.stdout); self.assertIn('APP-257',run.stdout); self.assertNotIn('APP-057',run.stdout)
   self.assertIn('GEN-021',run.stdout); self.assertIn('GEN-100',run.stdout); self.assertNotIn('GEN-020',run.stdout)
   for line in run.stdout.splitlines(): self.assertLessEqual(len(line),240)
   for line in run.stdout.splitlines():
    if 'APP-' in line or 'GEN-' in line: self.assertLessEqual(len(line.split('```')[0]),240)
   self.assertLessEqual(len(run.stdout.encode()),400*1024)
if __name__=='__main__': unittest.main()
