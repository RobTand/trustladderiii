"""Report per-test hosted distributions and tests near their fixed limits."""
import argparse
from collections import defaultdict
import hashlib
import json
import math
from pathlib import Path
import re
import statistics
import zipfile


WRITER = ('tl3-controller/production::'
          'configured_writer_executes_the_writer_ladder_and_reuses_its_durable_reply')


def summarize(reports, policy):
    samples = defaultdict(lambda: defaultdict(list))
    for report in reports:
        for name, elapsed in report['tests'].items():
            samples[report['arch']][name].append((elapsed, report['run_id']))
    result = {}
    for arch, tests in sorted(samples.items()):
        factor = {'X64': 1.25, 'ARM64': 1.15}[arch]
        distributions = {}
        for name, observations in sorted(tests.items()):
            values = sorted(elapsed for elapsed, _ in observations)
            maximum, run = max(observations)
            base = policy['slow'].get(name, policy['seconds'])
            distributions[name] = {
                'n': len(values), 'median': statistics.median(values),
                'p95': values[math.ceil(.95 * len(values)) - 1], 'max': maximum,
                'max_run': run, 'base_budget': base, 'effective_limit': base * factor,
                'max_ratio': maximum / (base * factor),
            }
        result[arch] = {'tests': distributions, 'near_limit': [
            name for name, row in distributions.items() if row['max_ratio'] >= .9]}
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('artifacts', type=Path, help='Fresh directory of RUN-rust-times-INDEX-ARCH.zip')
    parser.add_argument('--policy', type=Path, default=Path(__file__).with_name('rust-test-budget.json'))
    args = parser.parse_args()
    reports, inputs, seen = [], [], set()
    coverage = defaultdict(lambda: defaultdict(list))
    for file in sorted(args.artifacts.glob('*-rust-times-*.zip')):
        match = re.fullmatch(r'(\d+)-rust-times-(\d+)-(X64|ARM64)\.zip', file.name)
        if not match:
            raise ValueError(f'Unexpected artifact filename: {file.name}')
        run, index, arch = match.groups()
        with zipfile.ZipFile(file) as archive:
            names = [name for name in archive.namelist() if name.endswith('.json')]
            if len(names) != 1:
                raise ValueError(f'Expected one timing report in {file.name}')
            report = json.loads(archive.read(names[0]))
        if report['shard'][0] != int(index):
            raise ValueError(f'Shard metadata mismatch in {file.name}')
        identity = (run, arch, int(index))
        if identity in seen:
            raise ValueError(f'Duplicate shard: {file.name}')
        seen.add(identity)
        coverage[arch][run].append(report['shard'])
        reports.append({'run_id': int(run), 'arch': arch, 'tests': report['tests']})
        inputs.append({'file': file.name, 'sha256': hashlib.sha256(file.read_bytes()).hexdigest()})
    policy_bytes = args.policy.read_bytes()
    classes = summarize(reports, json.loads(policy_bytes))
    for arch, summary in classes.items():
        parity = 0 if arch == 'ARM64' else 1
        runs = {}
        for run, shards in sorted(coverage[arch].items()):
            totals = {total for _, total in shards}
            if len(totals) != 1:
                raise ValueError(f'Inconsistent shard totals in run {run}')
            total = totals.pop()
            indices = sorted(index for index, _ in shards)
            runs[run] = {'shards': indices, 'total': total,
                         'complete': indices == list(range(parity, total, 2))}
        summary['coverage'] = runs
        summary['complete_runs'] = sum(row['complete'] for row in runs.values())
        if summary['complete_runs'] < 20:
            raise ValueError(f'{arch}: need at least 20 complete hosted runs')
        tests = summary.pop('tests')
        summary['writer'] = tests.get(WRITER)
        summary['near_limit'] = {name: tests[name] for name in summary['near_limit']}
    print(json.dumps({'classes': classes, 'inputs': inputs,
                      'policy_sha256': hashlib.sha256(policy_bytes).hexdigest()}, indent=2))


if __name__ == '__main__':
    main()
