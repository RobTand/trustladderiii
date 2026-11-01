"""Repeat the budget-flake regressions and retain raw libtest/custom timings."""
import argparse
import json
import os
from pathlib import Path
import subprocess

import rust_gate


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repeats', type=int, default=3)
    parser.add_argument('--only', choices=('production', 'scope', 'looks', 'publishing',
                                          'tl3_controller', 'rest'))
    parser.add_argument('--report', type=Path, default=Path('target/fixture-costs.json'))
    args = parser.parse_args()
    if args.repeats < 1:
        parser.error('--repeats must be positive')
    targets = [
        ('tl3-controller', 'production', False, []),
        ('tl3-controller', 'scope', False, []),
        ('tl3-controller', 'looks', True, []),
        ('tl3-controller', 'publishing', True, ['repo-conventions']),
        ('tl3-controller', 'tl3_controller', False,
         ['adapters::worktrees::tests::github_installation_and_synchronous_token_bridge', '--exact']),
        ('tl3-github', 'rest', False, []),
    ]
    if args.only:
        targets = [target for target in targets if target[1] == args.only]
    binaries = []
    for package, target, custom, arguments in targets:
        command = ['cargo', 'test', '--locked', '-p', package,
                   '--lib' if target == 'tl3_controller' else '--test']
        if target != 'tl3_controller':
            command.append(target)
        result = subprocess.run([*command, '--no-run', '--message-format=json'],
                                text=True, stdout=subprocess.PIPE, check=True)
        for line in result.stdout.splitlines():
            row = json.loads(line)
            if (row.get('reason') == 'compiler-artifact' and row.get('executable')
                    and row['target']['name'] == target):
                binaries.append(({'name': f'{package}/{target}', 'command': [row['executable']]},
                                 custom, arguments))
    assert len(binaries) == len(targets), 'Cargo must emit exactly the requested test targets'
    policy = json.loads((rust_gate.ROOT / rust_gate.POLICY).read_text())
    reports, failures = [], []
    for repeat in range(args.repeats):
        for binary, custom, arguments in binaries:
            try:
                times = rust_gate.execute_binary(binary, custom, arguments)
                if binary['name'] == 'tl3-controller/looks':
                    expected = {f'tl3-controller/looks::closed_looks_{case}' for case in (
                        'permanent', 'startup', 'held', 'healthy', 'closing', 'stopping',
                        'pending-cancel', 'closed-pending')}
                    assert expected <= times.keys(), f'unbudgeted closed look cases: {expected - times.keys()}'
                rust_gate.check_budget(times, policy, policy)
                row = {'repeat': repeat + 1, 'binary': binary['name'], 'tests': times}
                reports.append(row)
                print(json.dumps({'fixture_costs': row}), flush=True)
            except (AssertionError, RuntimeError) as error:
                failures.append(f"{binary['name']} repeat {repeat + 1}: {error}")
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps({'host': os.uname().nodename, 'reports': reports,
                                       'failures': failures}, indent=2) + '\n')
    assert not failures, '\n'.join(failures)


if __name__ == '__main__':
    main()
