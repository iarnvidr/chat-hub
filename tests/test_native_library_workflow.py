import pathlib
import unittest
from unittest import mock
import os
import plistlib
import subprocess
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]

def parse_workflow_steps(text):
    """Parse this workflow's YAML step sequence using only the Python stdlib.

    This deliberately supports the block-scalar/key subset used by GitHub
    Actions steps, producing mappings rather than searching source offsets.
    Reject unexpected indentation/step syntax so a YAML shape change cannot
    silently weaken these dependency assertions.
    """
    lines = text.splitlines()
    start = next((i for i, line in enumerate(lines) if line == '    steps:'), None)
    if start is None:
        raise ValueError('workflow has no jobs.package.steps sequence')
    steps = []
    i = start + 1
    while i < len(lines):
        line = lines[i]
        if line and not line.startswith('      ') and not line.startswith('        '):
            break
        if not line.strip():
            i += 1
            continue
        if line.startswith('      - '):
            step = {}
            key, _, value = line[8:].partition(':')
            if not _:
                raise ValueError(f'invalid step entry: {line}')
            step[key] = value.strip()
            steps.append(step)
            i += 1
            continue
        if line.startswith('        ') and steps:
            body = line[8:]
            key, sep, value = body.partition(':')
            if not sep:
                raise ValueError(f'invalid step mapping: {line}')
            if value.strip() == '|':
                i += 1
                block = []
                while i < len(lines) and (not lines[i].strip() or lines[i].startswith('          ')):
                    block.append(lines[i][10:] if lines[i].startswith('          ') else '')
                    i += 1
                steps[-1][key] = '\\n'.join(block)
                continue
            steps[-1][key] = value.strip()
            i += 1
            continue
        raise ValueError(f'unexpected workflow step indentation: {line}')
    if not steps:
        raise ValueError('workflow step sequence is empty')
    return steps


def _assert_workflow_contract(test, steps, kind):
    def find(fragment):
        return next(i for i, step in enumerate(steps)
                    if fragment in step.get('name', '') or fragment in step.get('id', ''))

    gate = find('native-library-smoke') if kind == 'macos' else find('Required native library persisted-data smoke')
    build_commands = [i for i, step in enumerate(steps)
                      if 'bash build/build-native-macos.sh' in step.get('run', '')]
    test.assertEqual(len(build_commands), 1, 'expected exactly one production app build command')
    producer = build_commands[0]
    resolve = find('Resolve native-smoke Cargo artifact path')
    compile_step = find('Compile native-smoke feature') if kind == 'macos' else find('Required native-smoke feature compilation')
    debug_build = find('Build debug binary') if kind == 'macos' else find('Build native-smoke binary')
    prepare = find('Prepare isolated debug bundle')
    test.assertLess(producer, resolve)
    test.assertLess(producer, prepare)
    test.assertLess(resolve, compile_step)
    test.assertLess(compile_step, debug_build)
    test.assertLess(debug_build, prepare)
    test.assertLess(prepare, gate)
    prepare_run = steps[prepare].get('run', '')
    test.assertIn('cmp -s', prepare_run)
    test.assertIn('codesign --verify --deep --strict', prepare_run)
    app_assignment = next((line.strip() for line in prepare_run.split('\\n')
                           if line.strip().startswith('app=') and 'Chat Hub.app' in line), None)
    test.assertIsNotNone(app_assignment, 'debug prepare must assign the app bundle path')
    debug_path = '$RUNNER_TEMP/chat-hub-debug-smoke/Chat Hub.app'
    test.assertIn(debug_path, app_assignment)
    test.assertIn('ditto "$RUNNER_TEMP/chat-hub/Chat Hub.app" "$app"', prepare_run)
    test.assertLess(producer, prepare)
    gate_run = steps[gate].get('run', '')
    test.assertIn('build/native-library-smoke.sh', gate_run)
    test.assertIn(debug_path, gate_run)
    test.assertIn(debug_path, prepare_run)
    test.assertLess(gate, find('Required release app launch') if kind == 'macos' else find('Required startup and authenticated health smoke'))
    markers = ('native-launch-smoke', 'smoke-native-macos')
    if kind == 'macos':
        markers += ('updater-smoke', 'native-ui-smoke')
    for marker in markers:
        matching = [i for i, step in enumerate(steps) if marker in step.get('run', '')]
        test.assertTrue(matching, f'missing {marker}')
        test.assertGreater(min(matching), gate, marker)
    if kind == 'macos':
        test.assertEqual(steps[gate].get('if'), '${{ inputs.native_library_smoke }}')
        test.assertIn('native library smoke: skipped', steps[find('Report skipped native library smoke')]['run'])
        test.assertNotIn('continue-on-error', steps[gate])
        test.assertNotIn('for attempt', gate_run)
    else:
        test.assertNotIn('if', steps[gate])
        test.assertNotIn('continue-on-error', steps[gate])
    evidence = [i for i, step in enumerate(steps) if '--native-library-source' in step.get('run', '')]
    test.assertTrue(evidence, 'missing native-library evidence preparation')
    test.assertGreater(min(evidence), gate)


class NativeLibraryWorkflowTests(unittest.TestCase):
    def test_macos_opt_in_gate_has_correct_dependencies(self):
        _assert_workflow_contract(self, parse_workflow_steps((ROOT / '.github/workflows/macos.yml').read_text()), 'macos')

    def test_release_gate_has_correct_dependencies(self):
        _assert_workflow_contract(self, parse_workflow_steps((ROOT / '.github/workflows/release.yml').read_text()), 'release')

    def test_contract_rejects_debug_prepare_after_gate_and_wrong_artifact(self):
        import copy
        for kind, filename in (('macos', 'macos.yml'), ('release', 'release.yml')):
            with self.subTest(workflow=kind):
                original = parse_workflow_steps((ROOT / '.github/workflows' / filename).read_text())
                find = lambda fragment: next(i for i, step in enumerate(original) if fragment in step.get('name', '') or fragment in step.get('id', ''))
                gate = find('native-library-smoke') if kind == 'macos' else find('Required native library persisted-data smoke')
                prep = find('Prepare isolated debug bundle')
                producer = next(i for i, step in enumerate(original)
                                if 'bash build/build-native-macos.sh' in step.get('run', ''))
                mutant = copy.deepcopy(original)
                mutant[producer], mutant[gate] = mutant[gate], mutant[producer]
                with self.assertRaises(AssertionError): _assert_workflow_contract(self, mutant, kind)
                mutant = copy.deepcopy(original)
                mutant[producer], mutant[prep] = mutant[prep], mutant[producer]
                with self.assertRaises(AssertionError): _assert_workflow_contract(self, mutant, kind)
                mutant = copy.deepcopy(original)
                mutant[gate], mutant[prep] = mutant[prep], mutant[gate]
                with self.assertRaises(AssertionError): _assert_workflow_contract(self, mutant, kind)
                mutant = copy.deepcopy(original)
                mutant[gate]['run'] = mutant[gate]['run'].replace('$RUNNER_TEMP/chat-hub-debug-smoke/Chat Hub.app', '$RUNNER_TEMP/chat-hub/Chat Hub.app')
                with self.assertRaises(AssertionError): _assert_workflow_contract(self, mutant, kind)
                mutant = copy.deepcopy(original)
                gate = next(i for i, step in enumerate(mutant) if 'native-library-smoke' in step.get('name', '') or 'Required native library persisted-data smoke' in step.get('name', '') or step.get('id') == 'native-library-smoke')
                sanitizer = next(i for i, step in enumerate(mutant) if '--native-library-source' in step.get('run', ''))
                mutant[gate], mutant[sanitizer] = mutant[sanitizer], mutant[gate]
                with self.assertRaises(AssertionError): _assert_workflow_contract(self, mutant, kind)

    def test_wrapper_has_strict_flags_preflight_and_safe_executable(self):
        text=(ROOT/'build/native-library-smoke.sh').read_text()
        self.assertIn('"$#" -ne 5',text); self.assertIn('os.path.lexists(data)',text)
        self.assertIn('CFBundleIdentifier',text); self.assertIn('CFBundleExecutable',text)
        self.assertIn('pathlib.Path(executable).is_absolute()',text); self.assertIn('pwd.getpwuid(os.getuid())',text)
        self.assertIn('launchctl',text); self.assertIn('exec python3',text)
        self.assertNotRegex(text,r'\bexport HOME=|\brm\s+-')

    def test_embedded_preflight_safe_temp_home_and_specific_rejections(self):
        wrapper=(ROOT/'build/native-library-smoke.sh').read_text()
        code=compile(wrapper.split("<<'PY'\n",1)[1].split('\nPY\n',1)[0],'<preflight>','exec')
        # /tmp may be mounted noexec (os.access(..., X_OK) is then false
        # despite chmod); use the worktree filesystem for the executable fixture.
        with tempfile.TemporaryDirectory(dir=ROOT) as temp:
            root=pathlib.Path(temp); home=root/'home'; home.mkdir()
            app=root/'Example.app'; (app/'Contents/MacOS').mkdir(parents=True)
            def setup(bundle='com.example.app', executable='example'):
                (app/'Contents/Info.plist').write_bytes(plistlib.dumps({'CFBundleIdentifier':bundle,'CFBundleExecutable':executable}))
                if executable == 'example':
                    (app/'Contents'/'MacOS'/'example').write_text('x')
                    (app/'Contents'/'MacOS'/'example').chmod(0o755)
                else:
                    (app/'Contents/MacOS'/'example').unlink(missing_ok=True)
            setup(); binary=(app/'Contents'/'MacOS'/'example'); binary.write_text('x'); binary.chmod(0o755); called=[]
            def invoke(home_arg=home, env_extra=None, exe='example'):
                env={'HOME':str(home),'MUNINN_E2E_DIR':str(root/'e2e')}; env.update(env_extra or {})
                ns={'__name__':'__main__'}
                with mock.patch.dict(os.environ,env,clear=True), mock.patch('sys.platform','darwin'), mock.patch('pathlib.Path.home',return_value=home), mock.patch('pwd.getpwuid',return_value=type('Pw',(),{'pw_dir':str(home)})()), mock.patch.object(subprocess,'run',side_effect=lambda *a,**k:called.append(a)):
                    with mock.patch('sys.argv',['preflight',str(app),str(root/'ev'),str(home_arg)]): exec(code,ns)
            invoke(); self.assertEqual(len(called),1); self.assertEqual(called[0][0][0],'launchctl')
            called.clear()
            data=home/'Library/Application Support/com.example.app'; data.parent.mkdir(parents=True)
            data.write_text('x')
            with self.assertRaisesRegex(SystemExit,'default app data already exists'): invoke()
            data.unlink(); (data.parent/'com.example.app').symlink_to(root/'missing')
            with self.assertRaisesRegex(SystemExit,'default app data already exists'): invoke()
            (data.parent/'com.example.app').unlink()
            setup('../evil')
            with self.assertRaisesRegex(SystemExit,'bundle identifier must be strict'): invoke()
            setup(executable='../evil')
            with self.assertRaisesRegex(SystemExit,'bundle executable must be a safe filename'): invoke()
            setup()
            with self.assertRaisesRegex(SystemExit,'--disposable-home must identify'): invoke(root)
            with self.assertRaisesRegex(SystemExit,'forbidden data path override: MUNINN_DATA_DIR'): invoke(env_extra={'MUNINN_DATA_DIR':str(root)})
            with self.assertRaisesRegex(SystemExit,'HOME/pathlib home does not match'): invoke(env_extra={'HOME':str(root)})

if __name__=='__main__': unittest.main()
