"""Run Cargo test binaries concurrently and enforce per-test wall time budgets."""

from concurrent.futures import ThreadPoolExecutor
import json
import math
import os
from pathlib import Path
import re
import subprocess
import time
import tomllib

ROOT = Path(__file__).resolve().parents[2]
POLICY = Path('tools/ci/rust-test-budget.json')
MARKER = re.compile(r'^test (.+?) \.\.\. ok(?:[ ;(].*)?$')


def check_budget(results, config, baseline):
    seconds, slow = config['seconds'], config['slow']
    if (not isinstance(seconds, (int, float)) or not math.isfinite(seconds) or seconds <= 0
            or seconds > baseline['seconds'] or not slow.keys() <= baseline['slow'].keys()
            or any(not isinstance(limit, (int, float)) or not math.isfinite(limit) or limit <= 0
                   or limit > baseline['slow'][name] for name, limit in slow.items())):
        raise RuntimeError('Test budgets and the explicit slow list may not grow')
    failures = [f'{name}: {elapsed:.3f}s > {slow.get(name, seconds):.3f}s'
                for name, elapsed in sorted(results.items()) if elapsed > slow.get(name, seconds)]
    if failures:
        raise RuntimeError('Per-test time budget exceeded:\n' + '\n'.join(failures))


def execute_binary(binary, custom, arguments=()):
    name = binary['name']
    command = list(binary['command'])
    if not custom:
        command.extend(['-Z', 'unstable-options', '--format=json', '--report-time', '--test-threads=1'])
    command.extend(arguments)
    env = dict(os.environ)
    if not custom:
        env['RUSTC_BOOTSTRAP'] = '1'
    started = previous = time.monotonic()
    results = {}
    suite = False
    passed = 0
    failures = []
    # Diagnostics may end without a newline. Keep them off the completion stream.
    with subprocess.Popen(command, stdout=subprocess.PIPE, text=True, env=env) as process:
        for line in process.stdout:
            now = time.monotonic()
            print(f'[{name}] {line.rstrip()}', flush=True)
            if custom:
                match = MARKER.fullmatch(line.rstrip())
                if match:
                    key = f'{name}::{match[1]}'
                    results[key] = results.get(key, 0) + now - previous
                    previous = now
                continue
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                continue
            if event.get('type') == 'test':
                key = f"{name}::{event['name']}"
                if event['event'] in {'ignored', 'failed'}:
                    failures.append(f'{key}: failed or skipped')
                if event['event'] == 'ok':
                    if key in results or 'exec_time' not in event:
                        failures.append(f'{key}: duplicate or missing timing')
                    else:
                        results[key] = event['exec_time']
            if event.get('type') == 'suite' and event.get('event') in {'ok', 'failed'}:
                suite = True
                passed += event.get('passed', 0)
                if event.get('failed', 0) or event.get('ignored', 0):
                    failures.append(f'{name}: failed or skipped suite')
        returncode = process.wait()
    elapsed = time.monotonic() - started
    print(json.dumps({'binary': name, 'seconds': elapsed, 'tests': len(results),
                      'returncode': returncode}), flush=True)
    if returncode:
        failures.append(f'{name}: exit {returncode}')
    if custom:
        # This also budgets setup-only binaries and work after the last named scenario.
        results[f'{name}::__tail__'] = time.monotonic() - previous
    elif not suite or len(results) != passed:
        failures.append(f'{name}: missing suite or test result')
    if failures:
        raise RuntimeError('\n'.join(failures))
    return results


def execute_binaries(binaries, jobs):
    results = {}
    with ThreadPoolExecutor(max_workers=jobs) as pool:
        futures = [pool.submit(execute_binary, binary, binary['custom']) for binary in binaries]
        for future in futures:
            results.update(future.result())
    return results


def collect_binaries(manifest):
    command = ['cargo', 'test', '--manifest-path', str(manifest), '--workspace',
               '--all-targets', '--locked', '--no-run', '--message-format=json']
    artifacts = []
    subprocess.run(['cargo', '--version'], check=True)
    with subprocess.Popen(command, stdout=subprocess.PIPE, text=True, cwd=ROOT) as process:
        for line in process.stdout:
            event = json.loads(line)
            if event.get('reason') == 'compiler-artifact' and event['profile']['test'] and event['executable']:
                artifacts.append(event)
        if process.wait():
            raise RuntimeError('Cargo test build failed')
    binaries = []
    for artifact in artifacts:
        target = artifact['target']
        config = tomllib.loads(Path(artifact['manifest_path']).read_text())
        custom = any(test.get('harness') is False and test['name'] == target['name']
                     for test in config.get('test', []))
        package = config['package']['name']
        if package == 'tl3-controller' and target['name'] == 'github':
            for group in ['flow', 'notices']:
                binaries.append({'name': f'{package}/github-{group}',
                                 'command': [artifact['executable'], f'--gate-group={group}'],
                                 'custom': custom})
        else:
            binaries.append({'name': f"{package}/{target['name']}",
                             'command': [artifact['executable']], 'custom': custom})
    # Start the longest binaries first to avoid a serial tail at the end of the gate.
    longest = ['workers', 'github-flow', 'github-notices', 'publishing', 'mergify',
               'prismabuild', 'crash_safety', 'simulation', 'looks', 'hindsight',
               'postgres', 'tl3_controller', 'worktrees', 'fixture_lifecycle']
    order = {name: index for index, name in enumerate(longest)}
    return sorted(binaries, key=lambda binary: (order.get(binary['name'].split('/')[-1], len(order)), binary['name']))


def baseline_policy(config):
    """Compare with the target branch, never with the PR's changed policy."""
    event_path = os.environ.get('GITHUB_EVENT_PATH')
    ref = 'origin/main'
    if event_path:
        event = json.loads(Path(event_path).read_text())
        if 'pull_request' in event:
            ref = event['pull_request']['base']['sha']
        elif 'merge_group' in event:
            ref = event['merge_group']['base_sha']
    exists = subprocess.run(['git', 'cat-file', '-e', f'{ref}^{{commit}}'], cwd=ROOT,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    if exists.returncode:
        subprocess.run(['git', 'fetch', '--no-tags', '--depth=1', 'origin', ref],
                       cwd=ROOT, check=True)
    previous = subprocess.run(['git', 'show', f'{ref}:{POLICY}'], cwd=ROOT,
                              capture_output=True, text=True)
    if previous.returncode:
        # Bootstrap only: the target branch predates the budget file. Future PRs
        # compare to the policy actually merged on that target branch.
        return config
    return json.loads(previous.stdout)


def test_workspace(manifest, jobs=None, report=None, enforce=True, shard=None):
    if shard is not None:
        raise NotImplementedError("hosted shard selection")
    jobs = int(os.environ.get('RUST_TEST_THREADS', '4')) if jobs is None else jobs
    if jobs < 1:
        raise ValueError('Test concurrency must be positive')
    config = json.loads((ROOT / POLICY).read_text())
    baseline = baseline_policy(config) if enforce else config
    check_budget({}, config, baseline)
    binaries = collect_binaries(manifest)
    started = time.monotonic()
    results = execute_binaries(binaries, jobs)
    output = {'seconds': time.monotonic() - started, 'jobs': jobs, 'tests': results}
    for name, seconds in sorted(results.items()):
        print(json.dumps({'test': name, 'seconds': seconds}), flush=True)
    path = report or ROOT / 'target/ci-test-times.json'
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(output, indent=2, sort_keys=True) + '\n')
    print(json.dumps({'test_wall_seconds': output['seconds'], 'jobs': jobs,
                      'timings': str(path)}), flush=True)
    if enforce:
        check_budget(results, config, baseline)
    return output


if __name__ == '__main__':
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest-path', type=Path, default=Path('Cargo.toml'))
    parser.add_argument('--jobs', type=int)
    parser.add_argument('--report', type=Path)
    parser.add_argument('--measure', action='store_true', help='Report timings without enforcing budgets')
    args = parser.parse_args()
    test_workspace(args.manifest_path, jobs=args.jobs, report=args.report, enforce=not args.measure)
