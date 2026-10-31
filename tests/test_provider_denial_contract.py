import pathlib
import os
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]


class ProviderDenialContract(unittest.TestCase):
    def test_native_driver_runs_provider_invoke_and_requires_denial_evidence(self):
        driver = (ROOT / "build/native-ui-driver.py").read_text()
        source = (ROOT / "src-tauri/src/main.rs").read_text()
        self.assertIn("provider_denial", driver)
        self.assertIn("denial.get('denied') is not True", driver)
        self.assertIn('invoke(\\"service_info\\")', source)
        # Execute the extracted Rust format-string JS body with a Bun stub to
        # prove resolve/deny/missing-bridge behavior, including nonce matching.
        import json, re, subprocess
        match = re.search(r'let start = format!\("(.*?)"\);', source, re.S)
        self.assertIsNotNone(match)
        template = match.group(1)
        # Rust format! escapes literal JavaScript braces by doubling them.
        # Unescape those first, then substitute only the actual format arg;
        # rewriting every brace expression corrupts JS object literals.
        js = template.replace('\\\\"', '"').replace('\\\\\\\\', '\\\\')
        js = js.replace('\\"', '"')
        js = js.replace('{{', '{').replace('}}', '}')
        js = js.replace('{escaped_key}', '"probe-nonce"')
        probe = r'''const vm=require("vm"); async function run(mode){let calls=0;let c={window:{__TAURI_INTERNALS__:mode==="missing"?{}:{invoke:()=>{calls++;return mode==="reject"?Promise.reject(Error("denied")):Promise.resolve({})}}}};vm.runInNewContext(JS,c);await new Promise(r=>setImmediate(r));return {calls,state:c.window["probe-nonce"]}}; (async()=>{for(const [m,a,d] of [["resolve",true,false],["reject",true,true],["missing",false,null]]){let r=await run(m);if(r.calls!==(m==="missing"?0:1)||r.state.nonce!=="probe-nonce"||r.state.attempted!==a||(d!==null&&r.state.denied!==d))process.exit(1)}})()'''.replace('JS', json.dumps(js))
        result = subprocess.run([os.environ.get("BUN", "bun"), "--eval", probe], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("pub fn e2e_result", source)
        self.assertIn('app.get_webview("provider")', source)
        self.assertIn('window_policy::is_provider_origin(u.scheme()', source)
        self.assertIn('provider webview readiness timed out', source)
        self.assertIn('"readiness":readiness', source)
        self.assertIn("authorize_local_main(&window)?;", source[source.index("fn service_info("):])
        handlers = source.split(".invoke_handler", 1)[-1]
        self.assertIn("e2e_result", handlers)


if __name__ == "__main__":
    unittest.main()
