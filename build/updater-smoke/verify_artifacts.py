#!/usr/bin/env python3
"""Validate generated harness fixture structure without trusting a mocked updater."""
import json, pathlib, sys, tarfile
root = pathlib.Path(sys.argv[1])
metas = {}
for v in ('old', 'new'):
    meta = json.loads((root / f'{v}.json').read_text())
    metas[v] = meta
    archive = root / v / 'Muninn.app.tar.gz'
    signature = pathlib.Path(str(archive) + '.sig')
    if not archive.is_file() or archive.stat().st_size < 1024:
        raise SystemExit(f'FAIL: missing/implausible {v} actual app tar.gz archive')
    if not signature.is_file() or signature.read_text().strip() != meta.get('signature', '').strip():
        raise SystemExit(f'FAIL: {v} metadata signature does not match detached .sig file')
    if not meta.get('version') or not meta.get('signature'):
        raise SystemExit(f'FAIL: missing Tauri version/signature in {v} metadata')
    try:
        with tarfile.open(archive, 'r:gz') as tf:
            members = tf.getnames()
            if not members or any(not (m == 'Muninn.app' or m.startswith('Muninn.app/')) for m in members):
                raise SystemExit(f'FAIL: {v} archive is not rooted at Muninn.app')
            plist = tf.extractfile('Muninn.app/Contents/Info.plist')
            if plist is None:
                raise SystemExit(f'FAIL: {v} archive lacks bundle Info.plist')
            import plistlib
            bundle_version = plistlib.loads(plist.read()).get('CFBundleShortVersionString')
            if bundle_version != meta['version']:
                raise SystemExit(f'FAIL: {v} plist version does not match metadata')
    except (tarfile.TarError, OSError) as exc:
        raise SystemExit(f'FAIL: {v} archive is not valid gzip tar: {exc}')
if metas['old']['version'] != '0.2.14-smoke.1' or metas['new']['version'] != '0.2.14-smoke.2':
    raise SystemExit('FAIL: expected ordered smoke versions 0.2.14-smoke.1 then .2')
if metas['old']['version'] == metas['new']['version']:
    raise SystemExit('FAIL: versions are not distinct')
