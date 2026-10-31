import importlib.util
from pathlib import Path
import unittest


spec = importlib.util.spec_from_file_location(
    'native_scope', Path(__file__).resolve().parents[1] / 'build/native_scope.py')
native_scope = importlib.util.module_from_spec(spec)
spec.loader.exec_module(native_scope)


class NativeScopeTests(unittest.TestCase):
    def test_native_source_and_policy_changes_require_macos_validation(self):
        for path in ('src-tauri/src/main.rs', 'desktop-ui/app.tsx', 'build/native_scope.py',
                     '.github/workflows/check.yml', '.github/workflows/accept-native.yml',
                     'Cargo.lock', 'tests/native/launch.sh'):
            with self.subTest(path=path):
                self.assertTrue(native_scope.requires_native([path]))

    def test_docs_and_unrelated_workflows_skip_expensive_native_package(self):
        self.assertFalse(native_scope.requires_native([
            'README.md', 'docs/guide.md', '.github/workflows/ci.yml', 'scripts/lint.sh']))

    def test_mixed_change_requires_native_validation(self):
        self.assertTrue(native_scope.requires_native(['docs/guide.md', 'src-tauri/src/main.rs']))


if __name__ == '__main__':
    unittest.main()
