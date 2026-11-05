"""#743 controller/GitHub proof qualification on a disposable BuildBuddy runner.

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
import remote_gate  # noqa: E402


CASES = {'fit_monotone': ('crates/controller/src/budget/limits.rs',
                  '    size < limit',
                  '    size != limit',
                  '//crates/controller:verus_budget_limits'),
 'budget_bounds': ('crates/controller/src/budget/limits.rs',
                   'allowed.saturating_sub(used)',
                   'allowed.saturating_add(used)',
                   '//crates/controller:verus_budget_limits'),
 'cutoff_exact': ('crates/controller/src/budget/cutoff.rs',
                  '* 100 >=',
                  '* 100 >',
                  '//crates/controller:verus_budget_cutoff_witness'),
 'compaction_reduces': ('crates/controller/src/budget/limits.rs',
                        'after < before',
                        'after <= before',
                        '//crates/controller:verus_budget_limits'),
 'reserve_preserved': ('crates/controller/src/budget/limits.rs',
                       '    maker\n        &&',
                       '    maker\n        ||',
                       '//crates/controller:verus_budget_limits'),
 'exhaustion_exact': ('crates/controller/src/budget/limits.rs',
                      'if turns >= allowed {',
                      'if turns > allowed {',
                      '//crates/controller:verus_budget_limits'),
 'reviewer_total_and_sound': ('crates/controller/src/reviewer/decision.rs',
                              'if verdict == Verdict::Approve && !matches {\n',
                              'if verdict == Verdict::Approve && matches {\n',
                              '//crates/controller:verus_reviewer'),
 'postmortem_total_and_sound': ('crates/controller/src/postmortem_reply/decision.rs',
                                'Operation::Add => !item.before_nonempty && item.after_nonempty',
                                'Operation::Add => item.after_nonempty',
                                '//crates/controller:verus_postmortem'),
 'classification_total_and_sound': ('crates/controller/src/classifier/decision.rs',
                                    '    if !c.selected_matches {',
                                    '    if c.selected_matches {',
                                    '//crates/controller:verus_classifier'),
 'http_status_class_total': ('crates/github/src/status.rs',
                             '    status == 429 || status == 403 && rate_signal',
                             '    status == 429 || status == 403',
                             '//crates/github:verus_status')}


def run(workspace, target, name):
    statuses = workspace.test([target], 'remote', {'ROLE': 'FV_PORT_QUALIFICATION'},
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
    parser.add_argument('--expected-head', help='require this exact baseline before seeding')
    args = parser.parse_args()
    workspace = bazel_gate.Workspace()
    base = workspace.run('git', 'rev-parse', 'HEAD').strip()
    filename, before, after, target = CASES[args.case]
    if args.expected_head:
        assert base == args.expected_head, 'qualification baseline differs from the requested final head'
    report = {'case': args.case, 'head': base, 'target': target}
    try:
        statuses, log = run(workspace, target, 'fv-port-baseline-' + args.case)
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
        workspace.run('git', '-c', 'user.name=FV port qualification', '-c',
                      'user.email=fv-port@invalid', 'commit', '--quiet', '--no-verify',
                      '-am', 'Seed ' + args.case + ' in the executable source\n\n'
                      'Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>')
        report['mutant'] = workspace.run('git', 'rev-parse', 'HEAD').strip()
        report['mutant_parent'] = workspace.run('git', 'rev-parse', 'HEAD^').strip()
        assert report['mutant_parent'] == base, 'mutant must be a direct child of the baseline'
        statuses, log = run(workspace, target, 'fv-port-mutant-' + args.case)
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
        outputs = remote_gate.outputs({'mode': 'pr', 'verdicts': verdicts})
        assert outputs['rust'] == 'failure'
        try:
            remote_gate.aggregate('rust', {'bazel': {'result': 'failure',
                'outputs': outputs | {'scopes_uploaded': 'true'}}})
        except RuntimeError:
            pass
        else:
            raise AssertionError('required Rust aggregate accepted the mutant')
        report['rust_verdict'] = verdicts['rust']
        report['failed_rust_targets'] = failed['rust']
        report['required_rust_check'] = 'failure'
        report['passed'] = True
    finally:
        workspace.checkout(base)
        output = Path(os.environ['BUILDBUDDY_ARTIFACTS_DIRECTORY'])
        output.mkdir(parents=True, exist_ok=True)
        (output / ('fv-port-' + args.case + '.json')).write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report), flush=True)


if __name__ == '__main__':
    main()
