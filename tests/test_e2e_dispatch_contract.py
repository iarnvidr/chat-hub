import pathlib
import re
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[1]


class E2EDispatchContract(unittest.TestCase):
    def test_native_dispatch_and_async_result_use_central_publisher(self):
        source = (ROOT / "src-tauri/src/main.rs").read_text()
        self.assertRegex(source, r"e2e_protocol::publish_dispatch\s*\(")
        async_result = re.search(r"pub\s+fn\s+e2e_result\b[^{]*\{(.*?)\n    }", source, re.S)
        self.assertIsNotNone(async_result)
        self.assertRegex(async_result.group(1), r"e2e_protocol::publish_terminal\s*\(")
        dispatcher = re.search(r"fn\s+.*?dispatch.*?\{(.*?)(?=\n\s*}\s*\n)", source, re.S | re.I)
        self.assertIsNotNone(dispatcher)
        self.assertNotRegex(dispatcher.group(1), r"\.res\.json|write_all\s*\(")

    def test_successful_eval_is_deferred_ack_not_terminal(self):
        source = (ROOT / "src-tauri/src/main.rs").read_text()
        self.assertRegex(source, r"op\s*==\s*\"eval\"\s*&&\s*result\[\"ok\"\]\.as_bool\(\)\s*==\s*Some\(true\)[\s\S]{0,180}?DispatchReply::Deferred")
        self.assertRegex(source, r"DispatchReply::Terminal\s*\(\s*result\s*\)")


if __name__ == "__main__":
    unittest.main()
