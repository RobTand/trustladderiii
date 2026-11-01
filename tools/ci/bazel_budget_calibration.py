"""Derive a remote executor budget class from repeated, uncached Bazel runs.

Run the Rust tests with --runs_per_test=N (N >= 10), --nocache_test_results
and --config=ci, so Bazel keeps each run's receipt, then point this at the
testlogs directory. The method matches docs/hosted-budget-tolerance.md: take
each complete run's largest elapsed / base-budget ratio, use the nearest-rank
p99 of those maxima, add 10% and round up to the next 0.05, floor 1.0.
"""
import argparse
from collections import defaultdict
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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('testlogs', type=Path)
    parser.add_argument('--runner-class', required=True)
    parser.add_argument('--policy', type=Path, default=Path('tools/ci/rust-test-budget.json'))
    args = parser.parse_args()
    policy = json.loads(args.policy.read_text())
    print(json.dumps(summarize(args.testlogs, policy, args.runner_class), indent=2))


if __name__ == '__main__':
    main()
