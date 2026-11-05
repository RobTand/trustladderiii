"""#771 fault injection, invoked only on a disposable BuildBuddy runner.

This adapter runs the normal required Rust proof targets against real-source
mutants. A frontend error or timeout is never accepted as mutant rejection.
"""
import argparse
import json
import os
from pathlib import Path
import re
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / 'tools/ci'))
import bazel_gate  # noqa: E402


CASES = {
    'witness_skipped': ('crates/kernel/src/witness.rs',
                        'kernel_while! { (i < self.nodes.len()) [',
                        'kernel_while! { (false) [',
                        '//crates/kernel:verus_witness'),
    'resume_stalled': ('crates/kernel/src/commands.rs',
                       'Command::Resume => w.nodes[i].paused = false,',
                       'Command::Resume => w.nodes[i].paused = true,',
                       '//crates/kernel:verus_commands'),
    'reply_stalled': ('crates/kernel/src/work_replies.rs',
                      'set_phase(m, Phase::Checking);',
                      'set_phase(m, Phase::Making);',
                      '//crates/kernel:verus_work_replies'),
    'ack_retained': ('crates/kernel/src/acknowledgements.rs',
                     '    w.nodes[i].effect = None;',
                     '    w.nodes[i].effect = Some(e);',
                     '//crates/kernel:verus_acknowledgements'),
    'delivery_retained': ('crates/kernel/src/dependencies.rs',
                          'Some(index) => self.dependencies[index] = dependency,',
                          'Some(index) => self.dependencies[index] = Dependency { '
                          'status: DependencyStatus::Pending, ..dependency },',
                          '//crates/kernel:verus_dependencies'),
    'delivery_dispatch': ('crates/kernel/src/facts.rs',
                          'Fact::Dependency { target, status } => w.dependency_fact(i, target, status, out),',
                          'Fact::Dependency { target, status } => { let _ = (target, status); },',
                          '//crates/kernel:verus_facts'),
    'apply_enforcement': ('crates/controller/src/input_errors.rs',
                          'world.counter_exhaustion()',
                          'world.counter_exhaustion().filter(|_| false)',
                          '//crates/controller:test_test_input_error_followups'),
    'translate_enforcement': ('crates/controller/src/lib.rs',
                              'kernel_boundary::validate_raw_event(event).map_err(Error::Invalid)?',
                              'event',
                              '//crates/controller:test_test_input_error_followups'),
    'headroom': ('crates/kernel/src/headroom.rs',
                 'node.generation > u64::MAX - GENERATION_STEP_BUDGET',
                 'node.generation > u64::MAX - (GENERATION_STEP_BUDGET - 1)',
                 '//crates/kernel:verus_kernel'),
    'step_bound': ('crates/kernel/src/allocation.rs',
                   '    w.next_question += 1;\n    let m = &mut w.nodes[i];',
                   '    w.next_question += 2;\n    let m = &mut w.nodes[i];',
                   '//crates/kernel:verus_allocation'),
    'command_id': ('crates/kernel/src/lib.rs',
                   'self.last_cmd.checked_add(1)', 'self.last_cmd.checked_add(0)',
                   '//crates/kernel:verus_kernel'),
    'command_order': ('crates/kernel/src/commands.rs',
                      'if id <= w.nodes[i].last_cmd {', 'if id == w.nodes[i].last_cmd {',
                      '//crates/kernel:verus_commands'),
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
                              name, extra=['--jobs=16', '--nocache_test_results', '--noremote_accept_cached'])
    log = workspace.root / ('bazel-testlogs/' + target[2:].replace(':', '/') + '/test.log')
    text = log.read_text() if log.is_file() else ''
    print(text, flush=True)
    return statuses, text


def libtest_summaries(log):
    summaries = []
    for line in log.splitlines():
        marker = line.find('] {')
        if marker < 0:
            continue
        try:
            event = json.loads(line[marker + 2:])
        except ValueError:
            continue
        if event.get('type') == 'suite':
            summaries.append(event)
    return summaries


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
        assert statuses.get(target) == 'PASSED', statuses
        proof = ':verus_' in target
        if proof:
            assert 'VERUS_PROOF' in log
        else:
            assert any(event.get('event') == 'ok' and event.get('passed', 0) > 0
                       for event in libtest_summaries(log)), (
                'controller qualification must execute nonzero tests')
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
        if proof:
            assert any(message in log for message in (
                'postcondition not satisfied', 'precondition not satisfied',
                'arithmetic overflow', 'invariant not satisfied', 'assertion failed',
            )), 'mutant must fail a real proof obligation'
            assert re.search(r'verification results:: [0-9]+ verified, [1-9][0-9]* errors', log)
        else:
            assert any(event.get('event') == 'failed' and event.get('failed', 0) > 0
                       for event in libtest_summaries(log)) and 'panicked at' in log, (
                'boundary mutant must fail an executed assertion, not compilation')
        verdicts, failed = bazel_gate.classify([target], statuses, {target})
        assert verdicts['rust'] == 'fail' and failed['rust'] == [target]
        report['rust_verdict'] = verdicts['rust']
        report['failed_rust_targets'] = failed['rust']
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
