"""Derive a remote executor budget class from repeated, uncached Bazel runs.

Run the Rust tests with --runs_per_test=N (N >= 10), --nocache_test_results
and --config=ci, so Bazel keeps each run's receipt, then point this at the
testlogs directory. The method matches docs/hosted-budget-tolerance.md: take
each complete run's largest elapsed / base-budget ratio, use the nearest-rank
p99 of those maxima, add 10% and round up to the next 0.05, floor 1.0.

For CPU migration, --cpu-receipts FILE FILE [...] tightens observed CPU caps
to ceil(2 * maximum CPU), with a one second floor. The default uses twice
the nearest-rank measured p95. Fallback samples are excluded. It requires
two receipts and does not claim a population percentile.
"""
import argparse
from collections import defaultdict
import hashlib
import json
import math
from pathlib import Path
import re

from bazel_parity import read_receipt

RUN = re.compile(r'run_(\d+)_of_(\d+)$')


def receipts(testlogs):
    """Yield (target, run, total, seconds) for each Rust receipt."""
    for outputs in sorted(testlogs.rglob('test.outputs')):
        run_directory = outputs.parent
        match = RUN.match(run_directory.name)
        receipt = read_receipt(outputs) if match else None
        if receipt and 'seconds' in receipt:
            target = str(run_directory.parent.relative_to(testlogs))
            yield target, int(match[1]), int(match[2]), receipt['seconds']


def summarize(testlogs, policy, runner):
    runs, targets = defaultdict(dict), set()
    for target, run, _, seconds in receipts(testlogs):
        targets.add(target)
        runs[run][target] = seconds
    complete = {run: found for run, found in runs.items() if set(found) == targets}
    if len(complete) < 10:
        raise ValueError(f'need ten complete runs; found {len(complete)} of {len(runs)}')
    maxima, overruns, observations = [], [], 0
    for run, found in sorted(complete.items()):
        worst = 0.0
        for seconds in found.values():
            for key, elapsed in seconds.items():
                name = key
                budget = policy['slow'].get(name, policy['seconds'])
                ratio = elapsed / budget
                observations += 1
                worst = max(worst, ratio)
                if ratio > 1:
                    overruns.append({'run': run, 'test': name, 'seconds': elapsed,
                                     'base_budget': budget, 'ratio': round(ratio, 6)})
        maxima.append(worst)
    p99 = sorted(maxima)[math.ceil(len(maxima) * .99) - 1]
    return {'runner_class': runner, 'complete_runs': len(complete), 'binaries': len(targets),
            'test_observations': observations, 'run_max_ratio_p99': p99,
            'multiplier': math.ceil(max(1.0, p99 * 1.10) * 20 - 1e-9) / 20,
            'overruns': overruns}


def calibrate_cpu(paths, policy):
    """Initialize measured CPU caps, or tighten existing ones, from at least two receipts."""
    if len(paths) < 2:
        raise ValueError('need at least two CPU receipts')
    maxima, records = {}, []
    for path in paths:
        raw = path.read_bytes()
        receipt = json.loads(raw)
        wall, cpu = receipt['seconds'], receipt.get('cpu_seconds', {})
        if set(cpu) != set(wall) or not cpu:
            raise ValueError(f'{path}: missing CPU samples')
        for name, seconds in cpu.items():
            if not math.isfinite(seconds) or seconds < 0:
                raise ValueError(f'{path}: invalid CPU sample {name}')
            if name not in receipt.get('cpu_fallbacks', {}):
                maxima[name] = max(maxima.get(name, 0), seconds)
        records.append({'file': path.name, 'sha256': hashlib.sha256(raw).hexdigest(),
                        'scenarios': len(wall), 'wall_seconds': sum(wall.values()),
                        'cpu_seconds': sum(cpu.values()),
                        'fallbacks': receipt.get('cpu_fallbacks', {}),
                        'max_wall_base_ratio': max(wall[name] / policy['slow'].get(name, policy['seconds'])
                                                   for name in wall)})
    previous = policy.get('cpu') or policy
    limits = dict(previous['slow'])
    default = previous['seconds']
    measured_scenarios = sorted(seconds for name, seconds in maxima.items() if not name.endswith('::__tail__'))
    if not measured_scenarios:
        raise ValueError('no measured CPU scenarios after excluding fallbacks')
    p95 = measured_scenarios[math.ceil(len(measured_scenarios) * .95) - 1]
    if 'cpu' not in policy:
        default = max(default, math.ceil(2 * p95))
    else:
        default = min(default, max(1, math.ceil(2 * p95)))
    for name in limits.keys() & maxima.keys():
        # Two runs cannot establish a percentile. Allow 2x observed CPU, rounded to seconds.
        measured = max(1, math.ceil(maxima[name] * 2))
        limits[name] = min(limits[name], measured) if 'cpu' in policy else measured
    return {'cpu': {'seconds': default, 'slow': limits},
            'method': 'ceil(2 * measured max CPU), default ceil(2 * measured p95), floor 1s; existing CPU caps only shrink',
            'measured_scenario_p95_cpu_seconds': p95,
            'uncalibrated_slow': sorted(limits.keys() - maxima.keys()),
            'receipts': records, 'max_cpu_seconds': dict(sorted(maxima.items()))}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('testlogs', type=Path, nargs='?')
    parser.add_argument('--cpu-receipts', type=Path, nargs='+')
    parser.add_argument('--runner-class')
    parser.add_argument('--policy', type=Path, default=Path('tools/ci/rust-test-budget.json'))
    args = parser.parse_args()
    policy = json.loads(args.policy.read_text())
    if args.cpu_receipts:
        summary = calibrate_cpu(args.cpu_receipts, policy)
    else:
        if args.testlogs is None or args.runner_class is None:
            parser.error('wall calibration needs testlogs and --runner-class')
        summary = summarize(args.testlogs, policy, args.runner_class)
    print(json.dumps(summary, indent=2, sort_keys=True))


if __name__ == '__main__':
    main()
