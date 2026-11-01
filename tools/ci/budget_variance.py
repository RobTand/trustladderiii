"""Audit downloaded rust-times artifacts; never adjust CI limits automatically."""

import argparse
from collections import defaultdict
import hashlib
import json
import math
from pathlib import Path
import re
import zipfile


def percentile99(values):
    return sorted(values)[math.ceil(len(values) * .99) - 1]


def summarize(reports, policy):
    classes = defaultdict(list)
    for row in reports:
        classes[row['runner_class']].append(row)
    output = {}
    for runner, rows in sorted(classes.items()):
        parity = {'hosted-arm64': 0, 'hosted-x64': 1}[runner]
        runs = defaultdict(list)
        ratios, overruns = [], []
        for row in rows:
            runs[row['run_id']].append(row['report'])
            for name, elapsed in row['report']['tests'].items():
                budget = policy['slow'].get(name, policy['seconds'])
                ratio = elapsed / budget
                ratios.append(ratio)
                if ratio > 1:
                    overruns.append({'run_id': row['run_id'], 'test': name,
                                     'seconds': elapsed, 'base_budget': budget, 'ratio': ratio})
        maxima = []
        coverage = []
        for run_id, shards in sorted(runs.items()):
            totals = {report['shard'][1] for report in shards}
            if len(totals) != 1:
                raise ValueError(f'Inconsistent shard totals for run {run_id}')
            total = totals.pop()
            indices = [report['shard'][0] for report in shards]
            if len(indices) != len(set(indices)):
                raise ValueError(f'Duplicate shard for run {run_id}')
            complete = set(indices) == set(range(parity, total, 2))
            maximum = max(elapsed / policy['slow'].get(name, policy['seconds'])
                          for report in shards for name, elapsed in report['tests'].items())
            coverage.append({'run_id': run_id, 'complete': complete,
                             'shards': sorted(indices), 'total_shards': total,
                             'max_ratio': maximum})
            if complete:
                maxima.append(maximum)
        if len(maxima) < 10:
            raise ValueError(f'{runner}: need at least 10 complete runs for calibration')
        p99 = percentile99(maxima)
        output[runner] = {
            'complete_runs': len(maxima), 'partial_runs': len(runs) - len(maxima),
            'reports': len(rows), 'test_observations': len(ratios),
            'all_ratios_p99': percentile99(ratios), 'max_ratio': max(ratios),
            'run_max_ratio_p99': p99,
            'multiplier': math.ceil(max(1.0, p99 * 1.10) * 20) / 20,
            'overruns': overruns, 'runs': coverage,
        }
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('artifacts', type=Path, help='Directory of RUN-rust-times-INDEX-ARCH.zip')
    parser.add_argument('--policy', type=Path, default=Path(__file__).with_name('rust-test-budget.json'))
    args = parser.parse_args()
    reports, inputs = [], []
    for path in sorted(args.artifacts.glob('*-rust-times-*.zip')):
        match = re.fullmatch(r'(\d+)-rust-times-(\d+)-(X64|ARM64)\.zip', path.name)
        if not match:
            raise ValueError(f'Unexpected artifact filename: {path.name}')
        run, index, arch = match.groups()
        with zipfile.ZipFile(path) as archive:
            names = [name for name in archive.namelist() if name.endswith('.json')]
            if len(names) != 1:
                raise ValueError(f'Expected one timing report in {path.name}')
            report = json.loads(archive.read(names[0]))
        if report['shard'][0] != int(index):
            raise ValueError(f'Shard metadata mismatch in {path.name}')
        reports.append({'run_id': int(run), 'runner_class': f'hosted-{arch.lower()}',
                        'report': report})
        inputs.append({'file': path.name, 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()})
    policy_bytes = args.policy.read_bytes()
    print(json.dumps({'classes': summarize(reports, json.loads(policy_bytes)),
                      'policy_sha256': hashlib.sha256(policy_bytes).hexdigest(),
                      'artifact_manifest_sha256': hashlib.sha256(
                          json.dumps(inputs, sort_keys=True).encode()).hexdigest()}, indent=2))


if __name__ == '__main__':
    main()
