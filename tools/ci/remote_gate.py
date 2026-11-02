"""Start the Bazel gate on a BuildBuddy Remote Bazel runner and report its result.

Runs in the GitHub job. The runner executes tools/ci/bazel_gate.py and leaves
gate-result.json as a BuildBuddy artifact; this script fetches it. A failed,
timed-out or disconnected run, or a missing or malformed result, fails here.
"""
import argparse
import json
import os
import shlex
from pathlib import Path
import sys
import time
import urllib.error
import urllib.request

API = 'https://app.buildbuddy.io/api/v1/'
IMAGE = ('docker://gcr.io/flame-public/rbe-ubuntu24-04@sha256:'
         '5f0c01e5f6d79e58d7a996330de33aa38ce599bf8431eaf27521e7602daadf50')
RESULT = 'gate-result.json'
# The runner image has no uv; the gate's requirements check needs `uv export`.
UV_VERSION = '0.10.10'
UV_SHA256 = '3e1027f26ce8c7e4c32e2277a7fed2cb410f2f1f9320d3df97653d40e21f415b'
INSTALL_UV = f'''set -euo pipefail
cd "$(mktemp -d)"
curl -fsSLo uv.tar.gz https://github.com/astral-sh/uv/releases/download/{UV_VERSION}/uv-x86_64-unknown-linux-gnu.tar.gz
echo "{UV_SHA256}  uv.tar.gz" | sha256sum -c -
tar -xzf uv.tar.gz
install -D -m 755 uv-x86_64-unknown-linux-gnu/uv "$HOME/.local/bin/uv"'''
SCHEMA = 'tl3.bazel-gate.v1'
ARTIFACTS = ['bazel-remote-grpc.log', 'execution_log.binpb.zst']
DONE = 'COMPLETE_INVOCATION_STATUS'
LOST = 'DISCONNECTED_INVOCATION_STATUS'


class RemoteError(RuntimeError):
    pass


class BuildBuddy:
    def __init__(self, key, api=API):
        self.key, self.api = key, api

    def call(self, method, body, timeout=120, raw=False):
        """JSON in; JSON out, or the raw body for GetFile."""
        request = urllib.request.Request(
            self.api + method, data=json.dumps(body).encode(),
            headers={'x-buildbuddy-api-key': self.key, 'Content-Type': 'application/json'})
        try:
            with urllib.request.urlopen(request, timeout=timeout) as response:
                return response.read() if raw else json.load(response)
        except (urllib.error.URLError, TimeoutError, ValueError) as error:
            raise RemoteError(f'BuildBuddy {method} failed: {error}') from error


def run_request(args):
    if not args.base or not args.head:
        raise RemoteError('empty base or head: the queue-aware refs were unavailable')
    command = ['python3', 'tools/ci/bazel_gate.py', args.mode, '--base', args.base,
               '--head', args.head, '--branch', args.branch]
    return {
        'repo': args.repo,
        'branch': args.branch,
        'commit_sha': args.head,
        'steps': [{'run': INSTALL_UV},
                  {'run': 'export PATH="$HOME/.local/bin:$PATH" && ' + shlex.join(command)}],
        'env': {'GIT_REPO_DEFAULT_BRANCH': 'main'},
        'platform_properties': {
            # The runner hosts only the Bazel client; actions go to remote execution.
            'EstimatedCPU': str(args.cpu),
            'EstimatedMemory': args.memory,
            'EstimatedFreeDiskBytes': '50GB',
            'container-image': IMAGE,
            'workload-isolation-type': 'firecracker',
            'recycle-runner': 'true',
        },
        'timeout': f'{args.timeout}m',
        'wait_until': 'STARTED',
    }


def wait(client, invocation_id, deadline, interval=15, clock=time.monotonic, sleep=time.sleep,
         retries=3):
    """Wait for the run's own terminal status. Returns the invocation record."""
    errors = 0
    while True:
        try:
            found = client.call('GetInvocation', {'selector': {'invocation_id': invocation_id},
                                                  'include_artifacts': True}).get('invocation', [])
            errors = 0
        except RemoteError:
            # One API blip must not fail a whole merge-queue batch.
            errors += 1
            if errors > retries or clock() > deadline:
                raise
            sleep(interval)
            continue
        invocation = found[0] if found else {}
        status = invocation.get('invocationStatus')
        if status == DONE:
            return invocation
        if status == LOST:
            raise RemoteError(f'remote run {invocation_id} disconnected')
        if clock() > deadline:
            raise RemoteError(f'remote run {invocation_id} did not finish in time')
        sleep(interval)


def fetch(client, invocation, name):
    for artifact in invocation.get('artifacts', []):
        if artifact.get('name') == name:
            return client.call('GetFile', {'uri': artifact['uri']}, timeout=300, raw=True)
    return None


def parse_result(raw, head):
    if raw is None:
        raise RemoteError(f'the remote run left no {RESULT}')
    try:
        result = json.loads(raw)
    except json.JSONDecodeError as error:
        raise RemoteError(f'{RESULT} is not JSON: {error}') from error
    if result.get('schema') != SCHEMA or result.get('head') != head:
        raise RemoteError(f'{RESULT} has the wrong schema or head')
    return result


def outputs(result):
    """GitHub step outputs. Scopes are ready only when detection succeeded."""
    ready = result.get('error') is None and result.get('mode') == 'pr'
    values = {
        'scopes_ready': str(ready).lower(),
        'scopes': ','.join(result.get('scopes', [])) if ready else '',
        'all_scopes': str(bool(result.get('all_scopes'))).lower(),
    }
    inputs_ok = not result.get('error') and not result.get('inputs')
    if result.get('mode') == 'pr':
        for language in ['rust', 'python']:
            verdict = result.get('verdicts', {}).get(language)
            values[language] = 'success' if inputs_ok and verdict in ('pass', 'skipped') else 'failure'
    return values


def post_merge_record(result, prs, seconds):
    """One machine-readable line per post-merge run."""
    failures = {arch: sorted(info['failed']['rust'] + info['failed']['python'])
                for arch, info in result.get('architectures', {}).items()}
    missed = {arch: info['missed'] for arch, info in result.get('architectures', {}).items()}
    if result.get('error') or result.get('inputs'):
        verdict = 'gate-error'
    elif missed.get('amd64'):
        verdict = 'detection-miss'
    elif failures.get('amd64'):
        verdict = 'selected-but-failed'
    elif failures.get('arm64'):
        verdict = 'arm64-only'
    else:
        verdict = 'pass'
    return {'merge_sha': result['head'], 'base_sha': result['base'], 'prs': prs,
            'verdict': verdict, 'selection': result.get('selection'),
            'selected': len(result.get('selected', [])), 'tests': result.get('tests'),
            'failures': failures, 'missed': missed, 'seconds': round(seconds, 1),
            'error': result.get('error'), 'inputs': result.get('inputs')}


def explain(result, prs):
    lines = [f"Bazel gate ({result['mode']}) {result['base'][:12]}..{result['head'][:12]}"]
    if result.get('error'):
        lines.append(f"Detection failed: {result['error']}")
    if result.get('inputs'):
        lines.append('Stale generated inputs or parity drift; fix with tools/bazel/generate.sh: '
                     + '; '.join(result['inputs']))
    if result['mode'] == 'pr':
        lines.append(f"Selection: {result.get('selection')}, {len(result.get('selected', []))} tests")
        for language, labels in result.get('failed', {}).items():
            for label in labels:
                lines.append(f'FAILED ({language}): {label}')
    else:
        lines.append(f"Merged PRs: {', '.join(f'#{pr}' for pr in prs) or 'unknown'}")
        for arch, info in result.get('architectures', {}).items():
            for label in info['failed']['rust'] + info['failed']['python']:
                caught = 'not selected: detection miss' if label in info['missed'] else 'was selected'
                if arch == 'arm64':
                    caught = 'arm64 runs only after merge'
                lines.append(f'FAILED on {arch}: {label} ({caught})')
    return '\n'.join(lines)


def aggregate(language, needs):
    """The Rust and Python checks: every prerequisite job succeeded, scopes were
    uploaded, and the Bazel gate passed this language."""
    if not needs or any(job.get('result') != 'success' for name, job in needs.items()
                        if name != 'bazel'):
        raise RuntimeError(f'Every prerequisite must succeed: {needs}')
    gate = needs.get('bazel', {})
    outputs = gate.get('outputs', {})
    if gate.get('result') not in ('success', 'failure') or outputs.get('scopes_uploaded') != 'true':
        raise RuntimeError(f'The Bazel gate did not finish detection and scope upload: {gate}')
    if outputs.get(language) != 'success':
        raise RuntimeError(f'The Bazel gate failed {language}: see the Bazel job')


def write_github(path_variable, text):
    path = os.environ.get(path_variable)
    if path:
        with open(path, 'a') as handle:
            handle.write(text)


def main():
    if sys.argv[1:2] == ['aggregate']:
        try:
            aggregate(sys.argv[2], json.loads(os.environ['NEEDS_JSON']))
        except RuntimeError as error:
            sys.exit(str(error))
        return 0
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=['pr', 'main'])
    parser.add_argument('--base', default='')
    parser.add_argument('--head', required=True)
    parser.add_argument('--branch', required=True)
    parser.add_argument('--repo', default='https://github.com/RobTand/trustladderiii')
    parser.add_argument('--prs', default='', help='comma-separated merged PR numbers')
    parser.add_argument('--cpu', type=int, default=8)
    parser.add_argument('--memory', default='16GB')
    parser.add_argument('--timeout', type=int, default=60, help='minutes')
    parser.add_argument('--output-dir', type=Path, default=Path('gate-output'))
    args = parser.parse_args()
    prs = [int(pr) for pr in args.prs.split(',') if pr]
    started = time.monotonic()
    args.output_dir.mkdir(parents=True, exist_ok=True)
    try:
        client = BuildBuddy(os.environ['BUILDBUDDY_API_KEY'])
        invocation_id = client.call('Run', run_request(args), timeout=600).get('invocationId')
        if not invocation_id:
            raise RemoteError('BuildBuddy Run returned no invocation id')
        print(f'Remote Bazel run: https://app.buildbuddy.io/invocation/{invocation_id}', flush=True)
        invocation = wait(client, invocation_id, started + (args.timeout + 10) * 60)
        for name in ARTIFACTS:
            data = fetch(client, invocation, name)
            if data is not None:
                (args.output_dir / name).write_bytes(data)
        result = parse_result(fetch(client, invocation, RESULT), args.head)
    except RemoteError as error:
        print(f'::error::{error}', flush=True)
        write_github('GITHUB_OUTPUT', 'scopes_ready=false\nrust=failure\npython=failure\n')
        return 1
    (args.output_dir / RESULT).write_text(json.dumps(result, indent=2, sort_keys=True))
    summary = explain(result, prs)
    print(summary, flush=True)
    write_github('GITHUB_STEP_SUMMARY', '```\n' + summary + '\n```\n')
    write_github('GITHUB_OUTPUT', ''.join(f'{k}={v}\n' for k, v in outputs(result).items()))
    ok = invocation.get('success') and not result.get('error') and not result.get('inputs')
    if args.mode == 'main':
        record = post_merge_record(result, prs, time.monotonic() - started)
        line = json.dumps(record, sort_keys=True)
        (args.output_dir / 'post-merge-record.jsonl').write_text(line + '\n')
        print('POST_MERGE_RECORD ' + line, flush=True)
        write_github('GITHUB_STEP_SUMMARY', f'\n`POST_MERGE_RECORD {line}`\n')
        ok = ok and record['verdict'] == 'pass'
    else:
        ok = ok and all(value == 'success' for key, value in outputs(result).items()
                        if key in ('rust', 'python'))
    return 0 if ok else 1


if __name__ == '__main__':
    sys.exit(main())
