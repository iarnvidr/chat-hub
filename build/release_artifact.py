#!/usr/bin/env python3
"""Bind native artifacts to their exact trusted producing run; never rebuild."""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import re
import subprocess
import urllib.request
from urllib.parse import urlsplit
import zipfile

REPOSITORY = 'iarnvidr/chat-hub'
ARTIFACT = 'chat-hub-native-release'
WORKFLOW = '.github/workflows/macos.yml'


def sha(data):
    return hashlib.sha256(data).hexdigest()


def git(root, *args):
    return subprocess.run(['git', '-C', str(root), *args], check=True,
                          capture_output=True, text=True).stdout.strip()


def receipt(root):
    files = sorted((root / 'artifacts').glob('*.zip'))
    if len(files) != 1:
        raise ValueError('native producer must retain exactly one app archive')
    archive = files[0]
    checksum = archive.with_name(archive.name + '.sha256')
    if checksum.read_text().split()[0] != sha(archive.read_bytes()):
        raise ValueError('archive checksum mismatch')
    record = {'schema': 1, 'repository': os.environ['GITHUB_REPOSITORY'],
              'run_id': int(os.environ['GITHUB_RUN_ID']),
              'run_attempt': int(os.environ['GITHUB_RUN_ATTEMPT']),
              'event': os.environ['GITHUB_EVENT_NAME'],
              'head': os.environ['GITHUB_SHA'], 'tree': git(root, 'rev-parse', 'HEAD^{tree}'),
              'workflow_sha': os.environ['GITHUB_WORKFLOW_SHA'],
              'toolchain': {name: subprocess.run(command, capture_output=True, text=True, check=True).stdout.strip()
                            for name, command in {'rustc': ['rustc', '--version'], 'node': ['node', '--version'],
                                                  'macos': ['sw_vers', '-productVersion'],
                                                  'sdk': ['xcrun', '--show-sdk-version']}.items()},
              'platform': 'darwin-aarch64',
              'version': json.loads((root / 'src-tauri/tauri.conf.json').read_text())['version'],
              'files': {p.name: sha(p.read_bytes()) for p in (archive, checksum, archive.with_suffix('.dmg'))}}
    event = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text())
    pr = event.get('pull_request')
    if pr:
        record.update({'pr_number': pr['number'], 'pr_head_sha': pr['head']['sha'],
                       'tested_base': pr['base']['sha'],
                       'head_repository': pr['head']['repo']['full_name']})
    (root / 'artifacts/native-receipt.json').write_text(json.dumps(record, sort_keys=True) + '\n')


class SafeRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        redirected = super().redirect_request(req, fp, code, msg, headers, newurl)
        if redirected is not None and urlsplit(req.full_url).netloc != urlsplit(newurl).netloc:
            redirected.remove_header('Authorization')
        return redirected


def request(path, binary=False):
    url = path if path.startswith('https://') else 'https://api.github.com/' + path
    headers = {'Accept': 'application/vnd.github+json',
               'Authorization': 'Bearer ' + os.environ['GH_TOKEN'],
               'X-GitHub-Api-Version': '2022-11-28'}
    with urllib.request.build_opener(SafeRedirect()).open(urllib.request.Request(url, headers=headers), timeout=60) as response:
        data = response.read(512 * 1024 * 1024 + 1)
    if len(data) > 512 * 1024 * 1024:
        raise ValueError('artifact exceeds bounded download size')
    return data if binary else json.loads(data)


def validate(record, run, artifact, commit, tree, version):
    if (run.get('repository', {}).get('full_name') != REPOSITORY
            or run.get('head_repository', {}).get('full_name') != REPOSITORY
            or run.get('event') != 'workflow_dispatch' or run.get('head_branch') != 'main'
            or run.get('head_sha') != commit or run.get('conclusion') != 'success'
            or run.get('path') != WORKFLOW):
        raise ValueError('producer is not the exact successful trusted main native run')
    expected = {'schema': 1, 'repository': REPOSITORY, 'run_id': run['id'],
                'run_attempt': run['run_attempt'], 'event': 'workflow_dispatch', 'head': commit,
                'tree': tree, 'workflow_sha': commit, 'platform': 'darwin-aarch64',
                'version': version}
    if any(record.get(k) != v for k, v in expected.items()):
        raise ValueError('native receipt identity differs from its producing run or requested release')
    tools = record.get('toolchain', {})
    if set(tools) != {'rustc', 'node', 'macos', 'sdk'} or any(not isinstance(v, str) or not v for v in tools.values()):
        raise ValueError('native toolchain identity is incomplete')
    if artifact.get('expired') or artifact.get('name') != ARTIFACT:
        raise ValueError('native artifact missing or expired')
    files = record.get('files', {})
    name = f'Chat_Hub_{version}_macos-arm64.zip'
    if set(files) != {name, name + '.sha256', name.removesuffix('.zip') + '.dmg'} or any(
            not isinstance(v, str) or not re.fullmatch(r'[0-9a-f]{64}', v) for v in files.values()):
        raise ValueError('native receipt file inventory is incomplete or unexpected')


def retrieve(root, tag, api=request):
    version = json.loads((root / 'src-tauri/tauri.conf.json').read_text())['version']
    if tag != 'v' + version:
        raise ValueError('tag does not match source version')
    commit = git(root, 'rev-parse', 'HEAD^{commit}')
    tree = git(root, 'rev-parse', 'HEAD^{tree}')
    # The executing verifier comes from protected main, and only a producer using
    # the same reviewed native workflow is eligible. A PR cannot supply signer code.
    approved = api(f'repos/{REPOSITORY}/contents/{WORKFLOW}?ref=main')['sha']
    producing = api(f'repos/{REPOSITORY}/contents/{WORKFLOW}?ref={commit}')['sha']
    if approved != producing:
        raise ValueError('native workflow differs from approved main policy')
    candidates = []
    page = 1
    while page <= 10:
        runs = api(f'repos/{REPOSITORY}/actions/workflows/macos.yml/runs?head_sha={commit}&event=workflow_dispatch&status=success&per_page=100&page={page}')['workflow_runs']
        candidates.extend(r for r in runs if r.get('head_sha') == commit)
        if len(runs) < 100:
            break
        page += 1
    if not candidates:
        raise ValueError('no exact trusted native artifact; run a deliberate native build before release')
    if len(candidates) != 1:
        raise ValueError('multiple native producers for the requested commit; explicit accepted mapping is required')
    run = candidates[0]
    artifacts = api(f'repos/{REPOSITORY}/actions/runs/{run["id"]}/artifacts?per_page=100')['artifacts']
    matches = [a for a in artifacts if a['name'] == ARTIFACT and not a.get('expired')]
    if len(matches) != 1:
        raise ValueError('exact native run does not have one eligible artifact')
    artifact = matches[0]
    data = api(f'repos/{REPOSITORY}/actions/artifacts/{artifact["id"]}/zip', binary=True)
    if artifact.get('digest') != 'sha256:' + sha(data):
        raise ValueError('GitHub immutable artifact digest mismatch')
    with zipfile.ZipFile(io.BytesIO(data)) as bundle:
        names = bundle.namelist()
        if len(names) != 4 or len(names) != len(set(names)) or 'native-receipt.json' not in names:
            raise ValueError('unexpected native artifact inventory')
        if any(Path(n).name != n or bundle.getinfo(n).file_size > 512 * 1024 * 1024 for n in names):
            raise ValueError('unsafe native artifact member')
        record = json.loads(bundle.read('native-receipt.json'))
        validate(record, run, artifact, commit, tree, version)
        if set(names) != set(record['files']) | {'native-receipt.json'}:
            raise ValueError('native artifact inventory does not match receipt')
        payload = {n: bundle.read(n) for n in record['files']}
        if any(sha(payload[n]) != digest for n, digest in record['files'].items()):
            raise ValueError('native file checksum mismatch')
    target = root / 'artifacts'
    target.mkdir(exist_ok=True)
    for name, contents in payload.items():
        (target / name).write_bytes(contents)
    mapping = {'schema': 1, 'tag': tag, 'commit': commit, 'tree': tree,
               'producer_run_id': run['id'], 'producer_run_attempt': run['run_attempt'],
               'artifact_id': artifact['id'], 'artifact_digest': artifact['digest'],
               'files': record['files'], 'platform': record['platform']}
    mapping['toolchain'] = record['toolchain']
    (target / 'release-input.json').write_text(json.dumps(mapping, sort_keys=True) + '\n')
    return mapping


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('operation', choices=('receipt', 'retrieve'))
    parser.add_argument('--root', type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument('--tag')
    args = parser.parse_args()
    if args.operation == 'receipt':
        receipt(args.root)
    else:
        retrieve(args.root, args.tag)
