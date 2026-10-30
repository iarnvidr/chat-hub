"""Focused contract test for fail-closed native endpoint shutdown assertions."""
import ast
import pathlib
import types
import unittest
from unittest.mock import patch

ROOT = pathlib.Path(__file__).resolve().parents[1]
SOURCE = ROOT / 'build/native-launch-driver.py'

class NativeEndpointClosure(unittest.TestCase):
    def test_both_reported_endpoints_must_close_and_proxy_must_be_present(self):
        tree = ast.parse(SOURCE.read_text())
        node = next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == 'ports_closed')
        namespace = {'socket': types.SimpleNamespace(socket=None)}
        exec(compile(ast.Module(body=[node], type_ignores=[]), str(SOURCE), 'exec'), namespace)
        calls = []
        def factory(result):
            class FakeSocket:
                def settimeout(self, value): pass
                def connect_ex(self, address):
                    calls.append(address)
                    return result(address)
                def close(self): pass
            return FakeSocket
        info = {'api': 'http://127.0.0.1:1234', 'proxy': 'http://127.0.0.1:5678'}
        namespace['socket'].socket = factory(lambda address: 1)
        namespace['ports_closed'](info)
        self.assertEqual(calls, [('127.0.0.1', 1234), ('127.0.0.1', 5678)])
        for opened in (('127.0.0.1', 1234), ('127.0.0.1', 5678)):
            calls.clear()
            namespace['socket'].socket = factory(lambda address: 0 if address == opened else 1)
            with self.assertRaisesRegex(RuntimeError, 'listener remained open'):
                namespace['ports_closed'](info)
            self.assertEqual(calls, [*([('127.0.0.1', 1234)] if opened == ('127.0.0.1', 1234) else [('127.0.0.1', 1234), ('127.0.0.1', 5678)])])
        calls.clear()
        namespace['socket'].socket = factory(lambda address: 1)
        with self.assertRaisesRegex(RuntimeError, r'proxy endpoint not reported; keys=\[.api.\]'):
            namespace['ports_closed']({'api': info['api']})
        self.assertEqual(calls, [], 'missing proxy must fail before any socket probe')

if __name__ == '__main__': unittest.main()
