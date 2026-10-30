import pathlib
import unittest
from unittest import mock
import os
import plistlib
import subprocess
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]

class NativeLibraryWorkflowTests(unittest.TestCase):
    def test_macos_opt_in_gate_precedes_all_production_startup_and_debug_smokes(self):
        text = (ROOT / '.github/workflows/macos.yml').read_text()
        gate=text.index('id: native-library-smoke')
        for marker in ('Required release app launch', 'Build debug binary for native UI interactions', 'id: updater-smoke', 'Required native launch, UI, API', 'Optional updater UI interaction'):
            self.assertLess(gate,text.index(marker),marker)
        self.assertIn('native_library_smoke:',text)
        self.assertIn("if: ${{ inputs.native_library_smoke }}",text)
        section=text[gate:text.index('Resolve native-smoke Cargo artifact path')]
        self.assertEqual(section.count('build/native-library-smoke.sh'),1)
        self.assertNotIn('continue-on-error',section); self.assertNotIn('for attempt',section)
        self.assertIn('MUNINN_E2E_DIR="$(mktemp -d "$RUNNER_TEMP/chat-hub-native-library-e2e.XXXXXX")"',section)
        self.assertIn('pwd.getpwuid(os.getuid()).pw_dir',section)
        self.assertIn('native library smoke: ran (success)',section)
        self.assertIn('native library smoke: ran (failure',section)
        self.assertIn('native library smoke: skipped',section)

    def test_release_gate_is_unconditional_before_startup_after_dmg(self):
        text=(ROOT/'.github/workflows/release.yml').read_text()
        gate=text.index('Required native library persisted-data smoke')
        self.assertLess(gate,text.index('Upload only reviewed DMG'))
        section=text[gate:text.index('      - uses: actions/upload-artifact',gate)]
        self.assertEqual(section.count('build/native-library-smoke.sh'),1)
        self.assertNotIn('continue-on-error',section); self.assertNotRegex(section,r'\n\s+if:')
        self.assertIn('mktemp -d',section); self.assertIn('--allow-default-app-data --disposable-home',section)
        self.assertIn('--native-library-source',text)

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
