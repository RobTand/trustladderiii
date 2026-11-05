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
import remote_gate  # noqa: E402


CASES = {
    'retire_upstream_stalled': ('crates/kernel/src/facts.rs',
        '    if m.phase != Phase::Queued || m.waiting_incident {\n        return;\n    }\n    m.run_incidents = (m.run_incidents + 1).min(2);',
        '    if m.phase != Phase::Queued || m.waiting_incident {\n        return;\n    }\n    m.run_incidents = m.run_incidents.min(2);',
        '//crates/kernel:verus_facts_upstream_stalled'),
    'retire_outside_push': ('crates/kernel/src/facts.rs',
        '        update_recheck_head(&mut w.nodes[i], head);',
        '        let _ = head;',
        '//crates/kernel:verus_facts_outside_push'),
    'retire_ci_failed': ('crates/kernel/src/facts.rs',
        '    failed_round(w, i, ReviewStop::CiFailed, out);',
        '    failed_round(w, i, ReviewStop::ChecksFailed, out);',
        '//crates/kernel:verus_facts_ci_tests'),
    'retire_publishing_ci_tests': ('crates/kernel/src/facts.rs',
        '    let before_enqueue =\n        m.phase == Phase::Publishing && m.current() == Some(Item::Do(EffectKind::Enqueue));',
        '    let before_enqueue = false;',
        '//crates/kernel:verus_facts_ci_tests'),
    'retire_new_request': ('crates/kernel/src/events.rs',
        '                node.priority = priority;\n                w.nodes.push(node);\n            }\n            kernel_proof! { assert(retirement_wide::creation(before_apply@, *w, repo, priority, None)); }',
        '                node.priority = Priority::Unranked;\n                w.nodes.push(node);\n            }\n            kernel_proof! { assert(retirement_wide::creation(before_apply@, *w, repo, priority, None)); }',
        '//crates/kernel:verus_events'),
    'retire_new_adoption': ('crates/kernel/src/events.rs',
        '                node.proposed = Some(head);',
        '                node.proposed = None;',
        '//crates/kernel:verus_events'),
    'retire_gate_grant_w2': ('crates/kernel/src/gate_transition.rs',
        '            m.holds = true;',
        '            m.holds = false;',
        '//crates/kernel:verus_gate_transition'),
    'retire_gate_grant_w3': ('crates/kernel/src/gate_transition.rs',
        '            set_phase(m, r.phase());',
        '            let _ = r;',
        '//crates/kernel:verus_gate_transition'),
    'selector_inventory': ('crates/kernel/src/allocation.rs',
                           'use super::*;',
                           'use super::*;\nkernel_fn! { fn selector_inventory_unlisted() '
                           '-> (result: bool) [ ensures result, ] { true } }',
                           '//crates/kernel:test_test_proof_map'),
    'retire_merge_stopping': ('crates/kernel/src/facts.rs',
                             'm.merged = Some(head);\n    m.pr_open = false;',
                             'if !matches!(m.phase, Phase::Stopping(_)) { m.merged = Some(head); }\n    m.pr_open = false;',
                             '//crates/kernel:verus_facts_merge_open'),
    'retire_merge_closed': ('crates/kernel/src/facts.rs',
                           'm.merged = Some(head);\n            m.phase = Phase::Closed(Outcome::Merged);',
                           'm.merged = None;\n            m.phase = Phase::Closed(Outcome::Merged);',
                           '//crates/kernel:verus_facts_merge'),
    'retire_priority': ('crates/kernel/src/facts.rs',
                        'w.nodes[i].priority = priority;', 'let _ = priority;',
                        '//crates/kernel:verus_facts_priority'),
    'retire_pr_closed': ('crates/kernel/src/facts.rs',
                         'm.pr_open = false;\n        if !m.phase.is_stopping()',
                         'let _ = m.pr_open;\n        if !m.phase.is_stopping()',
                         '//crates/kernel:verus_facts_pr_closed'),
    'retire_conflict': ('crates/kernel/src/facts.rs',
                        'set_phase(m, Phase::Rebasing);', 'let _ = m;',
                        '//crates/kernel:verus_facts_conflict'),
    'retire_merge_reviewing': ('crates/kernel/src/facts.rs',
                             'm.merged = Some(head);\n    m.pr_open = false;',
                             'if m.phase != Phase::Reviewing { m.merged = Some(head); }\n    m.pr_open = false;',
                             '//crates/kernel:verus_facts_merge_open'),
    'retire_merge_publishing': ('crates/kernel/src/facts.rs',
                             'm.merged = Some(head);\n    m.pr_open = false;',
                             'if m.phase != Phase::Publishing { m.merged = Some(head); }\n    m.pr_open = false;',
                             '//crates/kernel:verus_facts_merge_open'),
    'retire_merge_queued': ('crates/kernel/src/facts.rs',
                             'm.merged = Some(head);\n    m.pr_open = false;',
                             'if m.phase != Phase::Queued { m.merged = Some(head); }\n    m.pr_open = false;',
                             '//crates/kernel:verus_facts_merge_open'),
    'retire_merge_rebasing': ('crates/kernel/src/facts.rs',
                             'm.merged = Some(head);\n    m.pr_open = false;',
                             'if m.phase != Phase::Rebasing { m.merged = Some(head); }\n    m.pr_open = false;',
                             '//crates/kernel:verus_facts_merge_open'),
    'retire_merge_awaiting_rob': ('crates/kernel/src/facts.rs',
                             'm.merged = Some(head);\n    m.pr_open = false;',
                             'if m.phase != Phase::AwaitingRob { m.merged = Some(head); }\n    m.pr_open = false;',
                             '//crates/kernel:verus_facts_merge_open'),
    'retire_merge_unplanned': ('crates/kernel/src/facts.rs',
                             'm.merged = Some(head);\n    m.pr_open = false;',
                             'if !matches!(m.phase, Phase::Unplanned) { m.merged = Some(head); }\n    m.pr_open = false;',
                             '//crates/kernel:verus_facts_merge_open'),
    'retire_merge_planning': ('crates/kernel/src/facts.rs',
                             'm.merged = Some(head);\n    m.pr_open = false;',
                             'if !matches!(m.phase, Phase::Planning { .. }) { m.merged = Some(head); }\n    m.pr_open = false;',
                             '//crates/kernel:verus_facts_merge_open'),
    'retire_merge_ready': ('crates/kernel/src/facts.rs',
                             'm.merged = Some(head);\n    m.pr_open = false;',
                             'if !matches!(m.phase, Phase::Ready(_)) { m.merged = Some(head); }\n    m.pr_open = false;',
                             '//crates/kernel:verus_facts_merge_open'),
    'retire_merge_making': ('crates/kernel/src/facts.rs',
                             'm.merged = Some(head);\n    m.pr_open = false;',
                             'if !matches!(m.phase, Phase::Making) { m.merged = Some(head); }\n    m.pr_open = false;',
                             '//crates/kernel:verus_facts_merge_open'),
    'retire_merge_checking': ('crates/kernel/src/facts.rs',
                             'm.merged = Some(head);\n    m.pr_open = false;',
                             'if !matches!(m.phase, Phase::Checking) { m.merged = Some(head); }\n    m.pr_open = false;',
                             '//crates/kernel:verus_facts_merge_open'),
    'retire_incident': ('crates/kernel/src/facts.rs',
                        'm.incident_needs_rob = false;',
                        'let _ = m.incident_needs_rob;',
                        '//crates/kernel:verus_facts_incident_cleared'),
    'retire_dequeued': ('crates/kernel/src/facts.rs',
                        'm.paused = true;', 'm.paused = false;',
                        '//crates/kernel:verus_facts_pr_dequeued'),
    'retire_infrastructure': ('crates/kernel/src/facts.rs',
                              'm.run_failures += 1;', 'm.run_failures += 0;',
                              '//crates/kernel:verus_facts_infrastructure_retry'),
    'retire_merge_ingesting': ('crates/kernel/src/facts.rs',
                               'm.merged = Some(head);\n    m.pr_open = false;',
                               'if m.phase != Phase::Ingesting { m.merged = Some(head); }\n    m.pr_open = false;',
                               '//crates/kernel:verus_facts_merge_open'),
    'retire_merge_auditing': ('crates/kernel/src/facts.rs',
                              'm.merged = Some(head);\n    m.pr_open = false;',
                              'if m.phase != Phase::Auditing { m.merged = Some(head); }\n    m.pr_open = false;',
                              '//crates/kernel:verus_facts_merge_open'),
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
                     '    let _ = e;',
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
    'leaf_bypass_overlap': ('crates/kernel/src/scheduler.rs',
                            '        if !w.footprints_disjoint(waiter.footprint, me.footprint) { return false; }',
                            '        if false { return false; }',
                            '//crates/kernel:verus_scheduler_first_in_line'),
    'leaf_grant_ticket': ('crates/kernel/src/allocation.rs',
                          '        m.phase = phase;\n        m.pos = 0;\n        m.ticket = None;',
                          '        m.phase = phase;\n        m.pos = 0;\n        let _ = m.ticket;',
                          '//crates/kernel:verus_allocation_set_phase'),
    'leaf_slot_release': ('crates/kernel/src/ranks.rs',
                          '        m.holds = false;\n        m.pos += 1;',
                          '        m.holds = true;\n        m.pos += 1;',
                          '//crates/kernel:verus_ranks'),
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
    parser.add_argument('--expected-head', help='require this exact baseline before seeding')
    args = parser.parse_args()
    workspace = bazel_gate.Workspace()
    base = workspace.run('git', 'rev-parse', 'HEAD').strip()
    filename, before, after, target = CASES[args.case]
    if args.expected_head:
        assert base == args.expected_head, 'qualification baseline differs from the requested final head'
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
                      '-am', 'Seed ' + args.case + ' in the executable source\n\n'
                      'Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>')
        report['mutant'] = workspace.run('git', 'rev-parse', 'HEAD').strip()
        report['mutant_parent'] = workspace.run('git', 'rev-parse', 'HEAD^').strip()
        assert report['mutant_parent'] == base, 'mutant must be a direct child of the baseline'
        statuses, log = run(workspace, target, 'safety-mutant-' + args.case)
        assert statuses.get(target) == 'FAILED', statuses
        if proof:
            assert any(message in log for message in (
                'postcondition not satisfied', 'precondition not satisfied',
                'arithmetic overflow', 'invariant not satisfied', 'assertion failed',
                'Call to non-static function fails to satisfy `callee.requires(args)`',
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
        (output / ('safety-' + args.case + '.json')).write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report), flush=True)


if __name__ == '__main__':
    main()
