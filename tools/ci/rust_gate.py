"""Run Cargo test binaries concurrently and enforce per-test CPU budgets with wall time backstops."""

from concurrent.futures import ThreadPoolExecutor
import json
import math
import os
from pathlib import Path
import re
import subprocess
import sys
from fnmatch import fnmatchcase
import time
import tomllib

import shards

ROOT = Path(__file__).resolve().parents[2]
POLICY = Path('tools/ci/rust-test-budget.json')
MARKER = re.compile(r'^test (.+?) \.\.\. ok(?:[ ;(].*)?$')
SCENARIO_COUNT = re.compile(r'^(\d+) .+ scenarios passed(?:;.*)?$')
# Fixed calibration from hosted artifacts; see docs/hosted-budget-tolerance.md.
BUDGET_MULTIPLIERS = {'baseline': 1.0, 'hosted-x64': 1.25, 'hosted-arm64': 1.15,
                      # BuildBuddy remote execution; docs/bazel-budget-calibration.json.
                      'buildbuddy-amd64': 1.65}


def runner_class():
    if os.environ.get('RUNNER_ENVIRONMENT') != 'github-hosted':
        return 'baseline'
    arch = os.environ.get('RUNNER_ARCH')
    if os.environ.get('RUNNER_OS') == 'Linux' and arch in ('X64', 'ARM64'):
        return f'hosted-{arch.lower()}'
    raise ValueError('Unmeasured hosted runner class')


WALL_BACKSTOP_MULTIPLIER = 10


def validate_limits(config, baseline):
    seconds, slow = config['seconds'], config['slow']
    if (not isinstance(seconds, (int, float)) or not math.isfinite(seconds) or seconds <= 0
            or seconds > baseline['seconds'] or not slow.keys() <= baseline['slow'].keys()
            or any(slow.get(name, seconds) > limit for name, limit in baseline['slow'].items())
            or any(not isinstance(limit, (int, float)) or not math.isfinite(limit) or limit <= 0
                   or limit > baseline['slow'][name] for name, limit in slow.items())):
        raise RuntimeError('Test budgets and the explicit slow list may not grow')


def check_budget(results, config, baseline, runner_class='baseline', cpu_seconds=None):
    validate_limits(config, baseline)
    cpu = config.get('cpu')
    if baseline.get('cpu') and not cpu:
        raise RuntimeError('CPU budgets may not grow or be removed')
    if cpu is not None:
        # On the migration PR, the target branch's wall caps bound the new CPU caps.
        validate_limits(cpu, baseline.get('cpu') or baseline)
        for key in ['wall_only', 'wall_only_platforms']:
            fallbacks = config.get(key, {})
            if (not isinstance(fallbacks, dict) or not all(fallbacks.values())
                    or (baseline.get('cpu') and not fallbacks.keys() <= baseline.get(key, {}).keys())):
                raise RuntimeError('Wall-only fallback lists may not grow')
    if runner_class not in BUDGET_MULTIPLIERS:
        raise ValueError(f'Unknown runner class: {runner_class}')
    multiplier = BUDGET_MULTIPLIERS[runner_class]
    failures = []
    for name, elapsed in sorted(results.items()):
        wall_limit = config['slow'].get(name, config['seconds']) * multiplier
        wall_only = (cpu is None or sys.platform in config.get('wall_only_platforms', {})
                     or any(fnmatchcase(name, pattern) for pattern in config.get('wall_only', {})))
        if wall_only:
            if elapsed > wall_limit:
                failures.append(f'{name}: wall {elapsed:.3f}s > {wall_limit:.3f}s '
                                f'({runner_class} x{multiplier})')
            continue
        measured = (cpu_seconds or {}).get(name)
        if measured is None or not math.isfinite(measured) or measured < 0:
            raise RuntimeError(f'Missing CPU timing or invalid CPU sample: {name}')
        cpu_limit = cpu['slow'].get(name, cpu['seconds'])
        if measured > cpu_limit:
            failures.append(f'{name}: CPU {measured:.3f}s > {cpu_limit:.3f}s')
        backstop = wall_limit * WALL_BACKSTOP_MULTIPLIER
        if elapsed > backstop:
            failures.append(f'{name}: wall backstop {elapsed:.3f}s > {backstop:.3f}s')
    if failures:
        raise RuntimeError('Per-test time budget exceeded:\n' + '\n'.join(failures))


def process_cpu_seconds(pid):
    """Linux task time plus waited-for children, in seconds, while the PID is owned."""
    try:
        # comm can contain spaces and closing parentheses; fields after its last ')' are stable.
        fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
        return sum(int(fields[index]) for index in (11, 12, 13, 14)) / os.sysconf('SC_CLK_TCK')
    except (OSError, ValueError, IndexError) as error:
        raise RuntimeError(f'CPU sampling failed for test PID {pid}: {error}') from error


def execute_binary(binary, custom, arguments=(), cpu_seconds=None):
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
    cpu_seconds = {} if cpu_seconds is None else cpu_seconds
    previous_cpu = 0.0
    sample_cpu = sys.platform == 'linux'
    suite = False
    passed = 0
    failures = []
    scenario_counts = []
    markers = 0
    # Diagnostics may end without a newline. Keep them off the completion stream.
    with subprocess.Popen(command, stdout=subprocess.PIPE, text=True, env=env) as process:
        for line in process.stdout:
            now = time.monotonic()
            print(f'[{name}] {line.rstrip()}', flush=True)
            if custom:
                match = MARKER.fullmatch(line.rstrip())
                if match:
                    current_cpu = process_cpu_seconds(process.pid) if sample_cpu else 0.0
                    markers += 1
                    key = f'{name}::{match[1]}'
                    if key in results:
                        failures.append(f'{name}: duplicate timing marker {match[1]}')
                    else:
                        results[key] = now - previous
                        if sample_cpu:
                            cpu_seconds[key] = current_cpu - previous_cpu
                    previous = now
                    previous_cpu = current_cpu
                count = SCENARIO_COUNT.fullmatch(line.rstrip())
                if count:
                    scenario_counts.append(int(count[1]))
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
                    current_cpu = process_cpu_seconds(process.pid) if sample_cpu else 0.0
                    if key in results or 'exec_time' not in event:
                        failures.append(f'{key}: duplicate or missing timing')
                    else:
                        results[key] = event['exec_time']
                        if sample_cpu:
                            cpu_seconds[key] = current_cpu - previous_cpu
                    previous_cpu = current_cpu
            if event.get('type') == 'suite' and event.get('event') in {'ok', 'failed'}:
                suite = True
                passed += event.get('passed', 0)
                if event.get('failed', 0) or event.get('ignored', 0):
                    failures.append(f'{name}: failed or skipped suite')
        if sample_cpu:
            # Keep the exited child as a zombie until its final counters are sampled.
            # Popen.wait/poll would reap it and remove /proc/<pid>/stat.
            os.waitid(os.P_PID, process.pid, os.WEXITED | os.WNOWAIT)
            final_cpu = process_cpu_seconds(process.pid)
        returncode = process.wait()
    elapsed = time.monotonic() - started
    print(json.dumps({'binary': name, 'seconds': elapsed, 'tests': len(results),
                      'returncode': returncode}), flush=True)
    if returncode:
        failures.append(f'{name}: exit {returncode}')
    if custom:
        if len(scenario_counts) != 1:
            failures.append(f'{name}: expected one scenario count declaration; received {len(scenario_counts)}')
        elif markers != scenario_counts[0]:
            failures.append(f'{name}: declared {scenario_counts[0]} scenarios; received {markers} timing markers')
        # This also budgets setup-only binaries and work after the last named scenario.
        results[f'{name}::__tail__'] = time.monotonic() - previous
        if sample_cpu:
            cpu_seconds[f'{name}::__tail__'] = final_cpu - previous_cpu
    elif not suite or len(results) != passed:
        failures.append(f'{name}: missing suite or test result')
    if not custom and cpu_seconds:
        # Charge libtest teardown to the final scenario instead of losing it after EOF.
        cpu_seconds[next(reversed(cpu_seconds))] += final_cpu - previous_cpu
    if failures:
        raise RuntimeError('\n'.join(failures))
    return results


def execute_binaries(binaries, jobs, cpu_seconds=None):
    results = {}
    with ThreadPoolExecutor(max_workers=jobs) as pool:
        samples = [{} for _ in binaries]
        futures = [pool.submit(execute_binary, binary, binary['custom'], cpu_seconds=sample)
                   for binary, sample in zip(binaries, samples)]
        for future, sample in zip(futures, samples):
            results.update(future.result())
            if cpu_seconds is not None:
                cpu_seconds.update(sample)
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


def prepare_binaries(binaries):
    started = time.monotonic()
    for binary in binaries:
        if binary['name'] == 'tl3-store/fixture_lifecycle':
            # Archive download is dependency setup, like the worker venv sync.
            # Real scenarios still create fresh databases and enforce their budgets.
            subprocess.run([*binary['command'], '--prepare-cache'], check=True)
    seconds = time.monotonic() - started
    print(json.dumps({'dependency_setup_seconds': seconds}), flush=True)
    return seconds


def test_workspace(manifest, jobs=None, report=None, enforce=True, shard=None):
    jobs = int(os.environ.get('RUST_TEST_THREADS', '4')) if jobs is None else jobs
    if jobs < 1:
        raise ValueError('Test concurrency must be positive')
    config = json.loads((ROOT / POLICY).read_text())
    baseline = baseline_policy(config) if enforce else config
    current_class = runner_class()
    check_budget({}, config, baseline, runner_class=current_class)
    binaries = collect_binaries(manifest)
    setup_seconds = prepare_binaries(binaries)
    if shard is not None:
        index, total = shard
        selected = set(shards.partition([binary['name'] for binary in binaries], total,
                                        shards.weights('rust'))[index])
        binaries = [binary for binary in binaries if binary['name'] in selected]
        if not binaries:
            raise RuntimeError('Empty Rust shard')
        print(json.dumps({'rust_shard': index, 'total': total,
                          'binaries': [binary['name'] for binary in binaries]}), flush=True)
    started = time.monotonic()
    cpu_seconds = {}
    results = execute_binaries(binaries, jobs, cpu_seconds=cpu_seconds)
    output = {'seconds': time.monotonic() - started, 'setup_seconds': setup_seconds,
              'jobs': jobs, 'shard': shard, 'tests': results, 'cpu_seconds': cpu_seconds,
              'runner_class': current_class, 'budget_multiplier': BUDGET_MULTIPLIERS[current_class]}
    for name, seconds in sorted(results.items()):
        print(json.dumps({'test': name, 'seconds': seconds}), flush=True)
    path = report or ROOT / 'target/ci-test-times.json'
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(output, indent=2, sort_keys=True) + '\n')
    print(json.dumps({'test_wall_seconds': output['seconds'], 'jobs': jobs,
                      'timings': str(path)}), flush=True)
    if enforce:
        check_budget(results, config, baseline, runner_class=current_class, cpu_seconds=cpu_seconds)
    return output


if __name__ == '__main__':
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest-path', type=Path, default=Path('Cargo.toml'))
    parser.add_argument('--jobs', type=int)
    parser.add_argument('--report', type=Path)
    parser.add_argument('--shard', type=shards.parse_shard, help='Zero-based INDEX/TOTAL')
    parser.add_argument('--prepare-only', action='store_true', help='Build targets and install test dependencies without running suites')
    parser.add_argument('--measure', action='store_true', help='Report timings without enforcing budgets')
    args = parser.parse_args()
    if args.prepare_only:
        prepare_binaries(collect_binaries(args.manifest_path))
    else:
        test_workspace(args.manifest_path, jobs=args.jobs, report=args.report, enforce=not args.measure, shard=args.shard)
