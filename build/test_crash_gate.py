from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]
workflow = (ROOT / '.github/workflows/macos.yml').read_text()
release = (ROOT / '.github/workflows/release.yml').read_text()
smoke = (ROOT / 'build/smoke-native-macos.sh').read_text()
handler = (ROOT / 'src-tauri/src/crash_diagnostics.rs').read_text()
assert 'continue-on-error: true' not in workflow.split('Required release app launch and localhost health smoke')[1].split('Build debug binary')[0]
assert 'Required startup and authenticated health smoke' in release
for needle in ('open', 'osascript', 'service PID changed', 'for _ in range(15):', "health_path='/api/health'", 'OBJC_DEBUG_MISSING_POOLS=NO', 'RUST_BACKTRACE=1', 'NSUnbufferedIO=YES', 'chat-hub-desktop*.ips'):
    assert needle in smoke, needle
for needle in ('NSSetUncaughtExceptionHandler', 'callStackSymbols', 'reason', 'name'):
    assert needle in handler, needle
assert 'crash_diagnostics::install();' in (ROOT / 'src-tauri/src/main.rs').read_text()
