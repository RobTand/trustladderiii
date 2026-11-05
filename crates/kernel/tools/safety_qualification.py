"""#771 fault injection, invoked only on a disposable BuildBuddy runner.

This adapter runs the normal required Rust proof targets against real-source
mutants. A frontend error or timeout is never accepted as mutant rejection.
"""
import argparse
import json
import os
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / 'tools/ci'))
import bazel_gate  # noqa: E402


CASES = {
    'headroom': ('crates/kernel/src/headroom.rs',
                 'node.generation > u64::MAX - GENERATION_STEP_BUDGET',
                 'node.generation > u64::MAX - (GENERATION_STEP_BUDGET - 1)',
                 '//crates/kernel:verus_kernel'),
    'step_bound': ('crates/kernel/src/lib.rs',
                   '    w.next_question += 1;\n    let m = &mut w.nodes[i];',
                   '    w.next_question += 2;\n    let m = &mut w.nodes[i];',
                   '//crates/kernel:verus_kernel'),
    'command_id': ('crates/kernel/src/lib.rs',
                   'self.last_cmd.checked_add(1)', 'self.last_cmd.checked_add(0)',
                   '//crates/kernel:verus_kernel'),
    'command_order': ('crates/kernel/src/lib.rs',
                      'if id <= w.nodes[i].last_cmd {', 'if id == w.nodes[i].last_cmd {',
                      '//crates/kernel:verus_kernel'),
    'raw_command': ('crates/controller/src/kernel_boundary.rs',
                    'Err("commands must come from the steering constructors, which assign next_command_id")',
                    'Ok(event)',
                    '//crates/kernel:verus_controller_boundary'),
    'steering_id': ('crates/controller/src/kernel_boundary.rs',
                    'request.next_command_id()', 'Some(request.last_cmd)',
                    '//crates/kernel:verus_controller_boundary'),
}


def run(workspace, target, name):
    statuses = workspace.test([target], 'remote', {'ROLE': 'KERNEL_SAFETY_QUALIFICATION'},
                              name, extra=['--jobs=16', '--nocache_test_results'])
    log = workspace.root / ('bazel-testlogs/' + target[2:].replace(':', '/') + '/test.log')
    text = log.read_text() if log.is_file() else ''
    print(text, flush=True)
    return statuses, text


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('case', choices=CASES)
    args = parser.parse_args()
    workspace = bazel_gate.Workspace()
    base = workspace.run('git', 'rev-parse', 'HEAD').strip()
    filename, before, after, target = CASES[args.case]
    report = {'case': args.case, 'head': base, 'target': target}
    try:
        statuses, log = run(workspace, target, 'safety-baseline-' + args.case)
        assert statuses.get(target) == 'PASSED' and 'VERUS_PROOF' in log, statuses
        path = workspace.root / filename
        source = path.read_text()
        assert source.count(before) == 1, 'mutant must select exactly one source expression'
        path.write_text(source.replace(before, after, 1))
        workspace.run('git', '-c', 'user.name=Kernel safety qualification', '-c',
                      'user.email=kernel-safety@invalid', 'commit', '--quiet', '--no-verify',
                      '-am', 'Seed ' + args.case + ' in the executable source')
        report['mutant'] = workspace.run('git', 'rev-parse', 'HEAD').strip()
        statuses, log = run(workspace, target, 'safety-mutant-' + args.case)
        assert statuses.get(target) == 'FAILED', statuses
        assert any(message in log for message in (
            'postcondition not satisfied', 'precondition not satisfied',
            'arithmetic overflow', 'invariant not satisfied', 'assertion failed',
        )), 'mutant must fail a real proof obligation'
        assert 'verification results::' in log and '0 errors' not in log
        verdicts, failed = bazel_gate.classify([target], statuses, {target})
        assert verdicts['rust'] == 'failure' and failed['rust'] == [target]
        report['required_rust_check'] = 'failure'
        report['passed'] = True
    finally:
        workspace.checkout(base)
        output = Path(os.environ['BUILDBUDDY_ARTIFACTS_DIRECTORY'])
        output.mkdir(parents=True, exist_ok=True)
        (output / ('safety-' + args.case + '.json')).write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report), flush=True)


if __name__ == '__main__':
    main()
