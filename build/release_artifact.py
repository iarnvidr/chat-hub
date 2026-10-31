#!/usr/bin/env python3
"""Bind Chat Hub native artifacts to an accepted merge; never rebuild for release."""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import stat
import subprocess
import urllib.parse
import urllib.request
from urllib.parse import urlsplit
import zipfile

REPOSITORY = 'iarnvidr/chat-hub'
NATIVE_WORKFLOW = '.github/workflows/macos.yml'
CHECK_WORKFLOW = '.github/workflows/check.yml'
MAPPING_WORKFLOW = '.github/workflows/accept-native.yml'
HELPER_PATH = 'build/release_artifact.py'
NATIVE_ARTIFACT_PREFIX = 'chat-hub-native-release'
CHECK_ARTIFACT_PREFIX = 'chat-hub-check-receipt'
MAPPING_ARTIFACT_PREFIX = 'chat-hub-accepted-native'
PLATFORM = 'darwin-aarch64'
MAX_ARTIFACT_BYTES = 512 * 1024 * 1024


def sha(data):
    return hashlib.sha256(data).hexdigest()


def git(root, *args):
    return subprocess.run(['git', '-C', str(root), *args], check=True,
                          capture_output=True, text=True).stdout.strip()


def version(root):
    return json.loads((root / 'src-tauri/tauri.conf.json').read_text())['version']


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
        data = response.read(MAX_ARTIFACT_BYTES + 1)
    if len(data) > MAX_ARTIFACT_BYTES:
        raise ValueError('artifact exceeds bounded download size')
    return data if binary else json.loads(data)


def zip_members(data, expected):
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        infos = archive.infolist()
        names = [info.filename for info in infos]
        if (len(names) != len(set(names)) or set(names) != set(expected)
                or any(PurePosixPath(name).is_absolute() or '..' in PurePosixPath(name).parts
                       or PurePosixPath(name).name != name for name in names)):
            raise ValueError('unexpected or unsafe artifact inventory')
        if any(info.file_size > MAX_ARTIFACT_BYTES for info in infos):
            raise ValueError('artifact member exceeds bounded size')
        return {name: archive.read(name) for name in names}


def artifact_bytes(artifact, api_fn):
    data = api_fn(f"repos/{REPOSITORY}/actions/artifacts/{artifact['id']}/zip", binary=True)
    if artifact.get('digest') != 'sha256:' + sha(data):
        raise ValueError('GitHub immutable artifact digest mismatch')
    return data


def blob_sha(path, revision, api_fn):
    quoted_path = urllib.parse.quote(path, safe='/')
    quoted_ref = urllib.parse.quote(revision, safe='')
    result = api_fn(f'repos/{REPOSITORY}/contents/{quoted_path}?ref={quoted_ref}')
    value = result.get('sha')
    if not isinstance(value, str) or not re.fullmatch(r'[0-9a-f]{40}', value):
        raise ValueError(f'could not resolve trusted policy file: {path}')
    return value


def verify_workflow(workflow_path, workflow_sha, merge_sha, api_fn):
    if not isinstance(workflow_sha, str) or not re.fullmatch(r'[0-9a-f]{40}', workflow_sha):
        raise ValueError('receipt has no valid workflow revision')
    accepted = blob_sha(workflow_path, merge_sha, api_fn)
    producing = blob_sha(workflow_path, workflow_sha, api_fn)
    if accepted != producing:
        raise ValueError(f'workflow differs from accepted main policy: {workflow_path}')
    return accepted


def verify_helper(helper_sha, merge_sha, api_fn):
    if not isinstance(helper_sha, str) or not re.fullmatch(r'[0-9a-f]{40}', helper_sha):
        raise ValueError('receipt has no valid artifact helper identity')
    accepted = blob_sha(HELPER_PATH, merge_sha, api_fn)
    if helper_sha != accepted:
        raise ValueError('artifact helper differs from the accepted main source')
    return accepted


def receipt(root):
    archives = sorted((root / 'artifacts').glob('Chat_Hub_*.zip'))
    if len(archives) != 1:
        raise ValueError('native producer must retain exactly one app archive')
    archive = archives[0]
    checksum = archive.with_name(archive.name + '.sha256')
    dmg = archive.with_suffix('.dmg')
    if not checksum.is_file() or not dmg.is_file():
        raise ValueError('native producer must retain app archive, checksum, and DMG')
    if checksum.read_text().split()[0] != sha(archive.read_bytes()):
        raise ValueError('archive checksum mismatch')
    event = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text())
    pr = event.get('pull_request') or {}
    pr_head = pr.get('head') or {}
    record = {
        'schema': 2, 'repository': os.environ['GITHUB_REPOSITORY'],
        'workflow_path': NATIVE_WORKFLOW,
        'run_id': int(os.environ['GITHUB_RUN_ID']),
        'run_attempt': int(os.environ['GITHUB_RUN_ATTEMPT']),
        'event': os.environ['GITHUB_EVENT_NAME'],
        'head': pr_head.get('sha') or os.environ['GITHUB_SHA'],
        'tree': git(root, 'rev-parse', 'HEAD^{tree}'),
        'workflow_sha': os.environ['GITHUB_WORKFLOW_SHA'],
        'helper_sha': git(root, 'rev-parse', f'HEAD:{HELPER_PATH}'),
        'toolchain': {name: subprocess.run(command, capture_output=True, text=True, check=True).stdout.strip()
                      for name, command in {'rustc': ['rustc', '--version'], 'node': ['node', '--version'],
                                            'macos': ['sw_vers', '-productVersion'],
                                            'sdk': ['xcrun', '--show-sdk-version']}.items()},
        'platform': PLATFORM, 'version': version(root),
        'files': {path.name: sha(path.read_bytes()) for path in (archive, checksum, dmg)},
    }
    if pr:
        record.update({'pr_number': pr['number'], 'pr_head_sha': pr_head.get('sha'),
                       'tested_base': (pr.get('base') or {}).get('sha'),
                       'head_repository': (pr_head.get('repo') or {}).get('full_name')})
    (root / 'artifacts/native-receipt.json').write_text(json.dumps(record, sort_keys=True) + '\n')


def check_receipt(root):
    if os.environ.get('GITHUB_EVENT_NAME') != 'pull_request':
        raise ValueError('check receipt is only produced for pull request runs')
    event = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text())
    pr = event.get('pull_request') or {}
    head = pr.get('head') or {}
    base = pr.get('base') or {}
    if not pr.get('number') or not head.get('sha') or not base.get('sha'):
        raise ValueError('pull request event is incomplete')
    record = {
        'schema': 1, 'repository': os.environ['GITHUB_REPOSITORY'],
        'workflow_path': CHECK_WORKFLOW,
        'run_id': int(os.environ['GITHUB_RUN_ID']),
        'run_attempt': int(os.environ['GITHUB_RUN_ATTEMPT']),
        'event': 'pull_request', 'head_sha': head['sha'], 'base_sha': base['sha'],
        'pr_number': pr['number'], 'tested_tree': git(root, 'rev-parse', 'HEAD^{tree}'),
        'workflow_sha': os.environ['GITHUB_WORKFLOW_SHA'],
        'helper_sha': git(root, 'rev-parse', f'HEAD:{HELPER_PATH}'),
    }
    target = root / 'artifacts'
    target.mkdir(exist_ok=True)
    (target / 'check-receipt.json').write_text(json.dumps(record, sort_keys=True) + '\n')


def artifact_name(run, kind):
    return f'{NATIVE_ARTIFACT_PREFIX}-{kind}-{run["id"]}-{run["run_attempt"]}'


def validate_run(run, artifact, event, head_sha, kind):
    if (run.get('repository', {}).get('full_name') != REPOSITORY
            or run.get('path') != NATIVE_WORKFLOW or run.get('event') != event
            or run.get('conclusion') != 'success' or run.get('head_sha') != head_sha):
        raise ValueError('native producer is not the exact successful run')
    if artifact.get('expired') or not isinstance(artifact.get('id'), int):
        raise ValueError('native artifact is missing or expired')
    if artifact.get('name') != artifact_name(run, kind):
        raise ValueError('native artifact name does not identify its producing run attempt')
    if not isinstance(artifact.get('digest'), str) or not re.fullmatch(r'sha256:[0-9a-f]{64}', artifact['digest']):
        raise ValueError('native artifact has no immutable digest')


def app_archive_is_safe(data):
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        infos = archive.infolist()
        names = [info.filename for info in infos]
        if not infos or len(names) != len(set(names)):
            raise ValueError('native app archive is empty or has duplicate paths')
        total = 0
        for info in infos:
            path = PurePosixPath(info.filename)
            mode = (info.external_attr >> 16) & 0xFFFF
            file_type = stat.S_IFMT(mode)
            if (path.is_absolute() or '..' in path.parts or not path.parts or '\\' in info.filename
                    or path.parts[0] not in ('Chat Hub.app', '__MACOSX')
                    or (path.parts[0] == '__MACOSX' and len(path.parts) < 2)
                    or info.flag_bits & 1 or info.file_size > MAX_ARTIFACT_BYTES
                    or file_type not in (0, stat.S_IFREG, stat.S_IFDIR, stat.S_IFLNK)):
                raise ValueError('native app archive contains an unsafe member')
            if file_type == stat.S_IFLNK:
                target = archive.read(info).decode('utf-8', errors='strict')
                if not safe_link_target(path, target):
                    raise ValueError('native app archive symlink escapes the app bundle')
            total += info.file_size
            if total > 1024 * 1024 * 1024:
                raise ValueError('native app archive exceeds the expanded size limit')


def safe_link_target(path, target):
    if not target or target.startswith('/') or '\\' in target:
        return False
    stack = list(path.parent.parts)
    for part in PurePosixPath(target).parts:
        if part in ('', '.'):
            continue
        if part == '..':
            if not stack:
                return False
            stack.pop()
        else:
            stack.append(part)
    return bool(stack) and stack[0] == 'Chat Hub.app'


def validate_files(record, members, build_version):
    archive_name = f'Chat_Hub_{build_version}_macos-arm64.zip'
    dmg_name = f'Chat_Hub_{build_version}_macos-arm64.dmg'
    expected_names = {archive_name, archive_name + '.sha256', dmg_name}
    files = record.get('files')
    if (not isinstance(files, dict) or set(files) != expected_names
            or any(not isinstance(value, str) or not re.fullmatch(r'[0-9a-f]{64}', value)
                   for value in files.values())):
        raise ValueError('native receipt file inventory is incomplete or unexpected')
    if any(sha(members[name]) != files[name] for name in expected_names):
        raise ValueError('native file checksum mismatch')
    checksum = members[archive_name + '.sha256'].decode('utf-8', errors='strict').split()
    if not checksum or checksum[0] != sha(members[archive_name]):
        raise ValueError('native archive checksum file does not match the app archive')
    app_archive_is_safe(members[archive_name])
    return files


def validate_pr_candidate(data, artifact, run, pr, merge_sha, merge_tree, build_version, api_fn):
    number = pr.get('number')
    head = (pr.get('head') or {}).get('sha')
    linked = run.get('pull_requests', [])
    tested_base = None
    if len(linked) == 1 and linked[0].get('number') == number:
        tested_base = (linked[0].get('base') or {}).get('sha')
    if (not number or not head or pr.get('merge_commit_sha') != merge_sha or not pr.get('merged')
            or (pr.get('base') or {}).get('ref') != 'main'
            or (pr.get('base') or {}).get('repo', {}).get('full_name') != REPOSITORY
            or (pr.get('head') or {}).get('repo', {}).get('full_name') != REPOSITORY
            or len(linked) != 1 or linked[0].get('number') != number or not tested_base):
        raise ValueError('candidate PR is not the exact merged same-repository main PR')
    validate_run(run, artifact, 'pull_request', head, 'pr')
    if run.get('head_repository', {}).get('full_name') != REPOSITORY:
        raise ValueError('fork-produced artifacts cannot be promoted')
    expected_files = {f'Chat_Hub_{build_version}_macos-arm64.zip',
                      f'Chat_Hub_{build_version}_macos-arm64.zip.sha256',
                      f'Chat_Hub_{build_version}_macos-arm64.dmg', 'native-receipt.json'}
    members = zip_members(data, expected_files)
    record = json.loads(members['native-receipt.json'])
    expected = {
        'schema': 2, 'repository': REPOSITORY, 'workflow_path': NATIVE_WORKFLOW,
        'run_id': run['id'], 'run_attempt': run['run_attempt'], 'event': 'pull_request',
        'head': head, 'pr_head_sha': head, 'pr_number': number,
        'head_repository': REPOSITORY, 'tested_base': tested_base,
        'tree': merge_tree, 'version': build_version, 'platform': PLATFORM,
    }
    if any(record.get(key) != value for key, value in expected.items()):
        raise ValueError('native receipt does not match the accepted PR tree, base, or run')
    tools = record.get('toolchain', {})
    if set(tools) != {'rustc', 'node', 'macos', 'sdk'} or any(not isinstance(value, str) or not value for value in tools.values()):
        raise ValueError('native toolchain identity is incomplete')
    files = validate_files(record, members, build_version)
    workflow_blob = verify_workflow(NATIVE_WORKFLOW, record.get('workflow_sha'), merge_sha, api_fn)
    helper_blob = verify_helper(record.get('helper_sha'), merge_sha, api_fn)
    return {
        'schema': 1, 'repository': REPOSITORY, 'accepted_commit': merge_sha,
        'accepted_tree': merge_tree, 'version': build_version, 'platform': PLATFORM,
        'pr_number': number, 'pr_head_sha': head, 'source_base_sha': tested_base,
        'source_run_id': run['id'], 'source_run_attempt': run['run_attempt'],
        'source_artifact_id': artifact['id'], 'source_artifact_digest': artifact['digest'],
        'source_artifact_name': artifact['name'], 'workflow_path': NATIVE_WORKFLOW,
        'workflow_sha': record['workflow_sha'], 'workflow_blob': workflow_blob,
        'helper_sha': helper_blob, 'toolchain': tools, 'files': files,
    }


def workflow_runs(workflow_path, head_sha, event, api_fn):
    workflow = Path(workflow_path).name
    found = []
    for page in range(1, 11):
        query = urllib.parse.urlencode({'head_sha': head_sha, 'event': event, 'status': 'success',
                                        'per_page': 100, 'page': page})
        batch = api_fn(f'repos/{REPOSITORY}/actions/workflows/{workflow}/runs?{query}').get('workflow_runs', [])
        found.extend(batch)
        if len(batch) < 100:
            break
    return found


def verify_check_run(run, pr, merge_sha, merge_tree, api_fn):
    head = pr['head']['sha']
    base_sha = run['pull_requests'][0]['base']['sha']
    artifacts = api_fn(f"repos/{REPOSITORY}/actions/runs/{run['id']}/artifacts?per_page=100").get('artifacts', [])
    name = f'{CHECK_ARTIFACT_PREFIX}-{run["id"]}-{run["run_attempt"]}'
    found = [item for item in artifacts if item.get('name') == name and not item.get('expired')]
    if len(found) != 1:
        raise ValueError('exact Linux check run has no unique unexpired receipt artifact')
    artifact = found[0]
    members = zip_members(artifact_bytes(artifact, api_fn), {'check-receipt.json'})
    record = json.loads(members['check-receipt.json'])
    expected = {
        'schema': 1, 'repository': REPOSITORY, 'workflow_path': CHECK_WORKFLOW,
        'run_id': run['id'], 'run_attempt': run['run_attempt'], 'event': 'pull_request',
        'head_sha': head, 'base_sha': base_sha, 'pr_number': pr['number'],
        'tested_tree': merge_tree,
    }
    if any(record.get(key) != value for key, value in expected.items()):
        raise ValueError('Linux check receipt does not identify the accepted PR tree, base, or run')
    workflow_blob = verify_workflow(CHECK_WORKFLOW, record.get('workflow_sha'), merge_sha, api_fn)
    helper_blob = verify_helper(record.get('helper_sha'), merge_sha, api_fn)
    return {
        'run_id': run['id'], 'run_attempt': run['run_attempt'], 'artifact_id': artifact['id'],
        'artifact_digest': artifact['digest'], 'workflow_sha': record['workflow_sha'],
        'workflow_path': CHECK_WORKFLOW, 'workflow_blob': workflow_blob, 'helper_sha': helper_blob,
        'tested_tree': merge_tree, 'base_sha': base_sha,
    }


def require_check_receipt(pr, tested_base, merge_sha, merge_tree, api_fn):
    head = pr['head']['sha']
    runs = workflow_runs(CHECK_WORKFLOW, head, 'pull_request', api_fn)
    matches = [run for run in runs if run.get('path') == CHECK_WORKFLOW
               and run.get('event') == 'pull_request' and run.get('head_sha') == head
               and run.get('conclusion') == 'success'
               and run.get('repository', {}).get('full_name') == REPOSITORY
               and run.get('head_repository', {}).get('full_name') == REPOSITORY
               and len(run.get('pull_requests', [])) == 1
               and run['pull_requests'][0].get('number') == pr['number']
               and (run['pull_requests'][0].get('base') or {}).get('sha') == tested_base]
    accepted = []
    failures = []
    for run in matches:
        try:
            receipt_ref = verify_check_run(run, pr, merge_sha, merge_tree, api_fn)
            accepted.append((run['id'], run['run_attempt'], receipt_ref))
        except (ValueError, KeyError, TypeError, OSError, zipfile.BadZipFile) as error:
            failures.append(str(error))
    if not accepted:
        reason = failures[-1] if failures else 'no successful Linux Checks run for the exact PR head and base'
        raise ValueError(f'accepted Linux check receipt unavailable: {reason}')
    return max(accepted, key=lambda item: (item[0], item[1]))[2]


def promote(root, merge_sha, api_fn=request):
    if git(root, 'rev-parse', 'HEAD^{commit}') != merge_sha:
        raise ValueError('promotion checkout does not match the actual main merge commit')
    merge_tree = git(root, 'rev-parse', 'HEAD^{tree}')
    build_version = version(root)
    summaries = api_fn(f'repos/{REPOSITORY}/commits/{merge_sha}/pulls')
    prs = [api_fn(f"repos/{REPOSITORY}/pulls/{item['number']}") for item in summaries
           if item.get('number') and (item.get('base') or {}).get('ref') == 'main']
    matches = [pr for pr in prs if pr.get('merged') and pr.get('merge_commit_sha') == merge_sha
               and (pr.get('base') or {}).get('ref') == 'main']
    if len(matches) != 1:
        raise ValueError('main commit must correspond to exactly one merged PR')
    pr = matches[0]
    if ((pr.get('head') or {}).get('repo', {}).get('full_name') != REPOSITORY
            or (pr.get('base') or {}).get('repo', {}).get('full_name') != REPOSITORY):
        raise ValueError('fork-produced artifacts cannot be promoted')
    head = pr['head']['sha']
    runs = workflow_runs(NATIVE_WORKFLOW, head, 'pull_request', api_fn)
    candidates = [run for run in runs if run.get('path') == NATIVE_WORKFLOW
                 and run.get('event') == 'pull_request' and run.get('head_sha') == head
                 and run.get('conclusion') == 'success'
                 and len(run.get('pull_requests', [])) == 1
                 and run['pull_requests'][0].get('number') == pr['number']]
    accepted = []
    errors = []
    for run in candidates:
        artifacts = api_fn(f"repos/{REPOSITORY}/actions/runs/{run['id']}/artifacts?per_page=100").get('artifacts', [])
        name = artifact_name(run, 'pr')
        found = [item for item in artifacts if item.get('name') == name and not item.get('expired')]
        if len(found) != 1:
            continue
        artifact = found[0]
        try:
            data = artifact_bytes(artifact, api_fn)
            record = validate_pr_candidate(data, artifact, run, pr, merge_sha,
                                            merge_tree, build_version, api_fn)
            check = require_check_receipt(pr, record['source_base_sha'], merge_sha, merge_tree, api_fn)
            accepted.append((run['id'], run['run_attempt'], record, check))
        except (ValueError, KeyError, TypeError, OSError, zipfile.BadZipFile) as error:
            errors.append(str(error))
    if not accepted:
        reason = errors[-1] if errors else 'no successful native PR artifact for the exact merged head and tree'
        raise ValueError(f'accepted native artifact unavailable: {reason}')
    _, _, record, check = max(accepted, key=lambda item: (item[0], item[1]))
    record['check_receipt'] = check
    record['acceptance'] = 'merged-pr'
    (root / 'accepted-native.json').write_text(json.dumps(record, sort_keys=True) + '\n')
    return record


def mapping_run(api_fn, run_id, commit):
    if not str(run_id).isdigit():
        raise ValueError('mapping run ID must be numeric')
    run = api_fn(f'repos/{REPOSITORY}/actions/runs/{run_id}')
    if (run.get('id') != int(run_id) or run.get('repository', {}).get('full_name') != REPOSITORY
            or run.get('path') != MAPPING_WORKFLOW or run.get('event') != 'push'
            or run.get('head_branch') != 'main' or run.get('head_sha') != commit
            or run.get('conclusion') != 'success'):
        raise ValueError('mapping run is not the successful trusted-main acceptance for this commit')
    return run


def read_mapping(api_fn, run, commit, tree, build_version):
    name = f'{MAPPING_ARTIFACT_PREFIX}-{commit}-{run["id"]}-{run["run_attempt"]}'
    artifacts = api_fn(f"repos/{REPOSITORY}/actions/runs/{run['id']}/artifacts?per_page=100").get('artifacts', [])
    found = [item for item in artifacts if item.get('name') == name and not item.get('expired')]
    if len(found) != 1:
        raise ValueError('accepted mapping artifact is missing, expired, or ambiguous')
    members = zip_members(artifact_bytes(found[0], api_fn), {'accepted-native.json'})
    accepted = json.loads(members['accepted-native.json'])
    if (accepted.get('repository') != REPOSITORY or accepted.get('accepted_commit') != commit
            or accepted.get('accepted_tree') != tree or accepted.get('version') != build_version
            or accepted.get('platform') != PLATFORM):
        raise ValueError('accepted mapping does not match the release commit, tree, version, or platform')
    return accepted


def source_artifact(accepted, api_fn):
    run_id, attempt = accepted.get('source_run_id'), accepted.get('source_run_attempt')
    if not isinstance(run_id, int) or not isinstance(attempt, int):
        raise ValueError('accepted native run identity is malformed')
    run = api_fn(f'repos/{REPOSITORY}/actions/runs/{run_id}')
    if (run.get('id') != run_id or run.get('run_attempt') != attempt
            or run.get('path') != NATIVE_WORKFLOW or run.get('event') != 'pull_request'
            or run.get('conclusion') != 'success' or run.get('head_sha') != accepted.get('pr_head_sha')
            or run.get('repository', {}).get('full_name') != REPOSITORY
            or run.get('head_repository', {}).get('full_name') != REPOSITORY):
        raise ValueError('source native run is not the accepted successful same-repository PR run')
    artifacts = api_fn(f'repos/{REPOSITORY}/actions/runs/{run_id}/artifacts?per_page=100').get('artifacts', [])
    found = [item for item in artifacts if item.get('id') == accepted.get('source_artifact_id')
             and item.get('name') == accepted.get('source_artifact_name')
             and item.get('digest') == accepted.get('source_artifact_digest')
             and not item.get('expired')]
    if len(found) != 1:
        raise ValueError('source native artifact ID or immutable digest changed after acceptance')
    pr = api_fn(f"repos/{REPOSITORY}/pulls/{accepted['pr_number']}")
    if (not pr.get('merged') or pr.get('merge_commit_sha') != accepted.get('accepted_commit')
            or (pr.get('base') or {}).get('repo', {}).get('full_name') != REPOSITORY
            or (pr.get('head') or {}).get('repo', {}).get('full_name') != REPOSITORY):
        raise ValueError('accepted PR is no longer the exact same-repository merge')
    return run, pr, found[0]


def verify_check_receipt(accepted, merge_sha, merge_tree, api_fn):
    reference = accepted.get('check_receipt')
    if not isinstance(reference, dict):
        raise ValueError('accepted mapping has no Linux check receipt reference')
    run_id, attempt = reference.get('run_id'), reference.get('run_attempt')
    if not isinstance(run_id, int) or not isinstance(attempt, int):
        raise ValueError('Linux check receipt run identity is malformed')
    run = api_fn(f'repos/{REPOSITORY}/actions/runs/{run_id}')
    if (run.get('id') != run_id or run.get('run_attempt') != attempt
            or run.get('path') != CHECK_WORKFLOW or run.get('event') != 'pull_request'
            or run.get('conclusion') != 'success' or run.get('head_sha') != accepted.get('pr_head_sha')
            or run.get('repository', {}).get('full_name') != REPOSITORY
            or run.get('head_repository', {}).get('full_name') != REPOSITORY
            or len(run.get('pull_requests', [])) != 1
            or run['pull_requests'][0].get('number') != accepted.get('pr_number')
            or (run['pull_requests'][0].get('base') or {}).get('sha') != accepted.get('source_base_sha')):
        raise ValueError('Linux receipt is not from the successful accepted PR and exact base')
    artifacts = api_fn(f'repos/{REPOSITORY}/actions/runs/{run_id}/artifacts?per_page=100').get('artifacts', [])
    name = f'{CHECK_ARTIFACT_PREFIX}-{run_id}-{attempt}'
    found = [item for item in artifacts if item.get('id') == reference.get('artifact_id')
             and item.get('name') == name and item.get('digest') == reference.get('artifact_digest')
             and not item.get('expired')]
    if len(found) != 1:
        raise ValueError('accepted Linux check artifact is missing, expired, or has changed identity')
    members = zip_members(artifact_bytes(found[0], api_fn), {'check-receipt.json'})
    record = json.loads(members['check-receipt.json'])
    expected = {'schema': 1, 'repository': REPOSITORY, 'workflow_path': CHECK_WORKFLOW,
                'run_id': run_id, 'run_attempt': attempt, 'event': 'pull_request',
                'head_sha': accepted['pr_head_sha'], 'base_sha': accepted['source_base_sha'],
                'pr_number': accepted['pr_number'], 'tested_tree': merge_tree,
                'workflow_sha': reference.get('workflow_sha'), 'helper_sha': accepted.get('helper_sha')}
    if any(record.get(key) != value for key, value in expected.items()):
        raise ValueError('Linux check artifact does not match its accepted run, policy, and merge tree')
    workflow_blob = verify_workflow(CHECK_WORKFLOW, record.get('workflow_sha'), merge_sha, api_fn)
    helper_blob = verify_helper(record.get('helper_sha'), merge_sha, api_fn)
    if (reference.get('workflow_path') != CHECK_WORKFLOW or reference.get('tested_tree') != merge_tree
            or reference.get('workflow_blob') != workflow_blob or reference.get('helper_sha') != helper_blob):
        raise ValueError('Linux check mapping differs from the accepted merge policy')
    return reference


def manual_main_candidate(run_id, merge_sha, merge_tree, build_version, api_fn):
    if not str(run_id).isdigit():
        raise ValueError('trusted-main native run ID must be numeric')
    run = api_fn(f'repos/{REPOSITORY}/actions/runs/{run_id}')
    if (run.get('id') != int(run_id) or run.get('path') != NATIVE_WORKFLOW
            or run.get('event') != 'workflow_dispatch' or run.get('head_branch') != 'main'
            or run.get('head_sha') != merge_sha or run.get('conclusion') != 'success'
            or run.get('repository', {}).get('full_name') != REPOSITORY):
        raise ValueError('fallback must be an exact successful trusted-main manual native run for this tag commit')
    artifacts = api_fn(f'repos/{REPOSITORY}/actions/runs/{run_id}/artifacts?per_page=100').get('artifacts', [])
    name = artifact_name(run, 'main')
    found = [item for item in artifacts if item.get('name') == name and not item.get('expired')]
    if len(found) != 1:
        raise ValueError('trusted-main fallback artifact is missing, expired, or ambiguous')
    artifact = found[0]
    expected_names = {f'Chat_Hub_{build_version}_macos-arm64.zip',
                      f'Chat_Hub_{build_version}_macos-arm64.zip.sha256',
                      f'Chat_Hub_{build_version}_macos-arm64.dmg', 'native-receipt.json'}
    members = zip_members(artifact_bytes(artifact, api_fn), expected_names)
    record = json.loads(members['native-receipt.json'])
    expected = {'schema': 2, 'repository': REPOSITORY, 'workflow_path': NATIVE_WORKFLOW,
                'run_id': run['id'], 'run_attempt': run['run_attempt'],
                'event': 'workflow_dispatch', 'head': merge_sha, 'tree': merge_tree,
                'version': build_version, 'platform': PLATFORM}
    if any(record.get(key) != value for key, value in expected.items()):
        raise ValueError('manual native receipt does not match the exact trusted-main commit')
    if any(key in record for key in ('pr_number', 'pr_head_sha', 'head_repository')):
        raise ValueError('manual native receipt unexpectedly identifies a PR build')
    tools = record.get('toolchain', {})
    if set(tools) != {'rustc', 'node', 'macos', 'sdk'} or any(not isinstance(v, str) or not v for v in tools.values()):
        raise ValueError('manual native toolchain identity is incomplete')
    files = validate_files(record, members, build_version)
    workflow_blob = verify_workflow(NATIVE_WORKFLOW, record.get('workflow_sha'), merge_sha, api_fn)
    helper_blob = verify_helper(record.get('helper_sha'), merge_sha, api_fn)
    accepted = {'schema': 1, 'repository': REPOSITORY, 'accepted_commit': merge_sha,
                'accepted_tree': merge_tree, 'version': build_version, 'platform': PLATFORM,
                'source_run_id': run['id'], 'source_run_attempt': run['run_attempt'],
                'source_artifact_id': artifact['id'], 'source_artifact_digest': artifact['digest'],
                'source_artifact_name': artifact['name'], 'workflow_path': NATIVE_WORKFLOW,
                'workflow_sha': record['workflow_sha'], 'workflow_blob': workflow_blob,
                'helper_sha': helper_blob, 'toolchain': tools, 'files': files,
                'acceptance': 'trusted-main-manual'}
    return members, accepted


def retrieve(root, tag, api_fn=request, mapping_run_id=None, trusted_main_run_id=None):
    if bool(mapping_run_id) == bool(trusted_main_run_id):
        raise ValueError('provide exactly one of --mapping-run-id or --trusted-main-run-id')
    build_version = version(root)
    if tag != 'v' + build_version:
        raise ValueError('tag does not match source version')
    merge_sha = git(root, 'rev-parse', 'HEAD^{commit}')
    merge_tree = git(root, 'rev-parse', 'HEAD^{tree}')
    if trusted_main_run_id:
        members, accepted = manual_main_candidate(trusted_main_run_id, merge_sha,
                                                 merge_tree, build_version, api_fn)
    else:
        mapping = mapping_run(api_fn, mapping_run_id, merge_sha)
        accepted = read_mapping(api_fn, mapping, merge_sha, merge_tree, build_version)
        verify_check_receipt(accepted, merge_sha, merge_tree, api_fn)
        run, pr, artifact = source_artifact(accepted, api_fn)
        data = artifact_bytes(artifact, api_fn)
        verified = validate_pr_candidate(data, artifact, run, pr, merge_sha,
                                         merge_tree, build_version, api_fn)
        for key, value in verified.items():
            if accepted.get(key) != value:
                raise ValueError('source PR artifact no longer matches its accepted main mapping')
        expected_files = set(accepted['files']) | {'native-receipt.json'}
        members = zip_members(data, expected_files)
    payload = {name: members[name] for name in accepted['files']}
    if any(sha(contents) != digest for name, contents in payload.items()
           for digest in [accepted['files'][name]]):
        raise ValueError('native file bytes differ from the accepted mapping')
    target = root / 'artifacts'
    target.mkdir(exist_ok=True)
    for name, contents in payload.items():
        (target / name).write_bytes(contents)
    release_input = {key: value for key, value in accepted.items()
                     if key not in ('check_receipt',)}
    release_input['tag'] = tag
    release_input['acceptance'] = accepted.get('acceptance', 'merged-pr')
    (target / 'release-input.json').write_text(json.dumps(release_input, sort_keys=True) + '\n')
    return release_input


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest='operation', required=True)
    receipt_parser = sub.add_parser('receipt')
    receipt_parser.add_argument('--root', type=Path, default=Path(__file__).resolve().parents[1])
    check_parser = sub.add_parser('check-receipt')
    check_parser.add_argument('--root', type=Path, default=Path.cwd())
    promote_parser = sub.add_parser('promote')
    promote_parser.add_argument('--root', type=Path, default=Path.cwd())
    promote_parser.add_argument('--merge-sha', required=True)
    retrieve_parser = sub.add_parser('retrieve')
    retrieve_parser.add_argument('--root', type=Path, default=Path.cwd())
    retrieve_parser.add_argument('--tag', required=True)
    retrieve_parser.add_argument('--mapping-run-id')
    retrieve_parser.add_argument('--trusted-main-run-id')
    args = parser.parse_args()
    try:
        if args.operation == 'receipt':
            receipt(args.root)
        elif args.operation == 'check-receipt':
            check_receipt(args.root)
        elif args.operation == 'promote':
            promote(args.root, args.merge_sha)
        else:
            retrieve(args.root, args.tag, mapping_run_id=args.mapping_run_id,
                     trusted_main_run_id=args.trusted_main_run_id)
    except Exception as exc:
        raise SystemExit(f'Chat Hub release acceptance blocked: {exc}') from exc
