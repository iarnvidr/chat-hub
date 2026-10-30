from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]
workflow = (ROOT / '.github/workflows/macos.yml').read_text()
release = (ROOT / '.github/workflows/release.yml').read_text()
smoke = (ROOT / 'build/smoke-native-macos.sh').read_text()
handler = (ROOT / 'src-tauri/src/crash_diagnostics.rs').read_text()
startup_step = workflow.split('      - name: Required release app launch and localhost health smoke\n', 1)[1].split('\n      - ', 1)[0]
assert 'continue-on-error:' not in startup_step
assert 'run: bash build/smoke-native-macos.sh "$RUNNER_TEMP/chat-hub/Chat Hub.app"' in startup_step
assert 'Required startup and authenticated health smoke' in release
for needle in ('open', 'osascript', 'service PID changed', 'for _ in range(15):', "health_path='/api/health'", 'OBJC_DEBUG_MISSING_POOLS=NO', 'RUST_BACKTRACE=1', 'NSUnbufferedIO=YES', 'chat-hub-desktop*.ips'):
    assert needle in smoke, needle
for needle in ('NSSetUncaughtExceptionHandler', 'callStackSymbols', 'reason', 'name'):
    assert needle in handler, needle
assert 'crash_diagnostics::install();' in (ROOT / 'src-tauri/src/main.rs').read_text()
