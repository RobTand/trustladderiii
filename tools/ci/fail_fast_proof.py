"""Bounded planted-failure proof. Run only as a bbrun.py step, at --jobs=16.

The synthetic selection has the same number of targets as a fallback selection,
without compiling or executing the repository's full suite. Raw BEP stays in
/tmp and is read in-process. Only this derived summary becomes an artifact.
"""
import argparse
import json
import os
from pathlib import Path
import tempfile
import time

import bazel_gate

JOBS = 16
SLEEP = 4

PLANTED = '''import os
from pathlib import Path
import sys
import time
mode = sys.argv[1]
if mode == 'budget':
    Path(os.environ['TEST_UNDECLARED_OUTPUTS_DIR'], 'budget-overrun.txt').write_text('budget only')
    raise SystemExit(1)
if mode == 'timeout':
    time.sleep(10)
if mode == 'slow':
    time.sleep(4)
assert mode == 'slow', 'planted early assertion failure'
'''


def metrics(name, elapsed, workspace, statuses):
    path = Path(tempfile.gettempdir(), f'{name}-bep.json')
    invocation, seconds, remote = None, 0.0, 0
    for event in bazel_gate.BepStream(path).read():
        if 'started' in event:
            invocation = event['started']['uuid']
        result = event.get('testResult')
        if result:
            duration = result.get('testAttemptDuration', '0s')
            seconds += float(duration.removesuffix('s'))
            remote += result.get('executionInfo', {}).get('strategy') == 'remote'
    return {'invocation': invocation, 'wall_seconds': round(elapsed, 3),
            'completed': sum(status != 'NO_STATUS' for status in statuses.values()), 'statuses': statuses,
            'remote_test_results': remote, 'test_seconds': round(seconds, 3),
            'aborted_early': workspace.aborted_early, 'not_run': workspace.not_run}


def run(targets, mode, name, quarantine=(), extra=()):
    workspace = bazel_gate.Workspace(mode=mode, quarantine=quarantine)
    start = time.monotonic()
    statuses = workspace.test(targets, 'remote', {'ROLE': 'FAIL_FAST_PROOF'}, name,
                              extra=['--jobs=16', '--nocache_test_results', *extra])
    return metrics(name, time.monotonic() - start, workspace, statuses)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--guards-only', action='store_true')
    args = parser.parse_args()
    if os.environ.get('BBRUN_FINAL_SCAN') != '1':
        raise SystemExit('Use the shared bbrun.py step wrapper for the required final artifact scan')
    count = len(bazel_gate.Workspace().tests())
    assert count > JOBS, 'fallback must be larger than one wave'
    print(f'Fallback-sized proof: {count} targets, --jobs=16', flush=True)
    with tempfile.TemporaryDirectory(prefix='fail_fast_fixture_', dir=bazel_gate.ROOT) as directory:
        package = Path(directory)
        prefix = f'//{package.name}:'
        (package / 'planted.py').write_text(PLANTED)
        rules = ['load("@rules_python//python:py_test.bzl", "py_test")']
        for name, mode in [('budget', 'budget'), ('timeout', 'timeout'), ('quarantine', 'fail'),
                           *[(f't{index:04}', 'fail' if index == 0 else 'slow') for index in range(count)]]:
            rules.append(f'py_test(name={name!r}, srcs=["planted.py"], main="planted.py", '
                         f'args=[{mode!r}], size="small")')
        (package / 'BUILD.bazel').write_text('\n'.join(rules) + '\n')
        guards = run([prefix + name for name in ['budget', 'timeout', 'quarantine']],
                     'batch-draft', 'fail-fast-guards', quarantine={prefix + 'quarantine'},
                     extra=['--test_timeout=2'])
        assert guards['aborted_early'] is None, guards
        assert guards['statuses'] == {prefix + 'budget': 'FAILED', prefix + 'timeout': 'TIMEOUT',
                                     prefix + 'quarantine': 'FAILED'}, guards
        if args.guards_only:
            print(json.dumps({'guards': guards}), flush=True)
            return
        targets = [prefix + f't{index:04}' for index in range(count)]
        baseline = run(targets, 'main', 'fail-fast-baseline')
        assert baseline['completed'] == count, baseline
        assert baseline['statuses'][targets[0]] == 'FAILED', baseline
        cancelled = run(targets, 'batch-draft', 'fail-fast-cancelled')
        assert cancelled['aborted_early'] == {'label': targets[0], 'reason': 'FAILED'}, cancelled
        assert cancelled['not_run'], 'cancellation must leave targets without results'
        assert cancelled['remote_test_results'], 'proof must use remote execution'
        # Each completed sleeper used at least SLEEP executor-seconds. At most
        # JOBS actions ran concurrently in the cancelled invocation; charging
        # all slots for its entire wall time overestimates its executor usage.
        lower_bound = ((count - 1) * SLEEP - JOBS * cancelled['wall_seconds']) / 60
        report = {'fallback_targets': count, 'jobs': JOBS, 'sleep_seconds': SLEEP,
                  'guards': guards, 'baseline': baseline, 'cancelled': cancelled,
                  'wall_minutes_saved': round((baseline['wall_seconds'] - cancelled['wall_seconds']) / 60, 3),
                  'executor_minutes_saved_lower_bound': round(lower_bound, 3)}
        print(json.dumps(report, sort_keys=True), flush=True)
        output = Path(os.environ['BUILDBUDDY_ARTIFACTS_DIRECTORY'], 'fail-fast-proof.json')
        output.write_text(json.dumps(report, indent=2, sort_keys=True) + '\n')


if __name__ == '__main__':
    main()
