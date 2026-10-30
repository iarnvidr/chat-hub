#!/usr/bin/env bash
set -euo pipefail
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
if [[ "$#" -ne 5 || "$3" != --allow-default-app-data || "$4" != --disposable-home ]]; then
  echo 'usage: native-library-smoke.sh APP EVIDENCE --allow-default-app-data --disposable-home REAL_HOME' >&2
  exit 2
fi
app="$1" evidence="$2" disposable_home="$5"
python3 - "$app" "$evidence" "$disposable_home" <<'PY'
import os, pathlib, plistlib, pwd, re, subprocess, sys
app, evidence, supplied_home = map(pathlib.Path, sys.argv[1:])
if sys.platform != 'darwin':
    raise SystemExit('native library smoke requires macOS')
real_home = pathlib.Path(pwd.getpwuid(os.getuid()).pw_dir).resolve(strict=True)
if pathlib.Path.home().resolve(strict=True) != real_home or pathlib.Path(os.environ.get('HOME', '')).resolve(strict=True) != real_home:
    raise SystemExit('HOME/pathlib home does not match actual account home')
if supplied_home.resolve(strict=True) != real_home:
    raise SystemExit('--disposable-home must identify the actual account home')
for name in ('MUNINN_E2E_DATA_DIR', 'MUNINN_DATA_DIR', 'XDG_DATA_HOME'):
    if os.environ.get(name): raise SystemExit(f'forbidden data path override: {name}')
if not os.environ.get('MUNINN_E2E_DIR'): raise SystemExit('MUNINN_E2E_DIR is required')
app = app.resolve(strict=True)
if not app.is_dir() or app.suffix != '.app': raise SystemExit('expected an existing .app bundle')
with (app / 'Contents/Info.plist').open('rb') as f: info = plistlib.load(f)
bundle_id = info.get('CFBundleIdentifier')
if not isinstance(bundle_id, str) or len(bundle_id) > 255 or not re.fullmatch(r'[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?(?:\.[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?)+', bundle_id):
    raise SystemExit('bundle identifier must be strict reverse-DNS (no empty/dot segments)')
data = real_home / 'Library/Application Support' / bundle_id
if os.path.lexists(data): raise SystemExit(f'default app data already exists (including symlink): {data}; refusing to modify')
executable = info.get('CFBundleExecutable')
if (not isinstance(executable, str) or not executable or executable in ('.', '..') or '/' in executable or '\\\\' in executable or pathlib.Path(executable).is_absolute()):
    raise SystemExit('bundle executable must be a safe filename without path separators')
binary = app / 'Contents/MacOS' / executable
if not binary.is_file() or not os.access(binary, os.X_OK): raise SystemExit('bundle executable is missing or not executable')
subprocess.run(['launchctl', 'print', f'gui/{os.getuid()}'], check=True, stdout=subprocess.DEVNULL)
PY
mkdir -p "$evidence"
exec python3 "$script_dir/native-library-smoke.py" "$app" "$evidence" --allow-default-app-data --disposable-home "$disposable_home"
