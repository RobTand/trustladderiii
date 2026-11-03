"""Selection must fail closed: any doubt about what to test fails the gate."""
# rust_runner is loaded by path from the //tools/bazel:sources data, not as a py dependency.
# gazelle:ignore rust_runner
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import bazel_gate

TESTS = {'//crates/kernel:lib_test_tl3_kernel', '//crates/store:test_test_postgres',
         '//harness/tests:test_harness'}
USED = {'crates/kernel/src/lib.rs', 'crates/store/src/lib.rs', 'harness/src/tl3_harness/cli.py',
        'crates/kernel/README.md'}


def plan(changed, impacted=(), deleted=()):
    return bazel_gate.plan(set(changed), set(deleted), set(impacted), USED, TESTS)


class FakeWorkspace:
    """Records calls; answers from canned git and Bazel facts."""

    def __init__(self, changed, impacted, statuses, diff_error=None, build_ok=True, warnings=None):
        self.changed, self.impacted, self.statuses = changed, impacted, statuses
        self.diff_error, self.ran, self.built, self.build_ok = diff_error, [], [], build_ok
        self.warnings = warnings or []

    def ensure_commit(self, sha):
        if not sha:
            raise bazel_gate.GateError('empty base or head')

    def changes(self, base, head):
        return set(self.changed), set()

    def bazel_diff(self, base, head):
        if self.diff_error:
            raise bazel_gate.GateError(self.diff_error)
        return set(self.impacted)

    def checkout(self, sha):
        pass

    def tests(self):
        return set(TESTS)

    def rust_tests(self):
        return {label for label in TESTS if label.startswith('//crates')}

    def used_sources(self):
        return set(USED)

    def budget_warnings(self, labels):
        return [warning for warning in self.warnings if warning['label'] in labels]

    def test(self, targets, config, metadata, log_name):
        self.ran.append((config, list(targets)))
        return {label: self.statuses.get(label, 'PASSED') for label in targets}

    def build(self, targets, config, metadata, log_name):
        self.built.append((config, list(targets)))
        return self.build_ok

    def inputs_check(self):
        return []

    def budget_check(self, base):
        return []

    def graph_check(self, base, head):
        return {'errors': [], 'summary': 'U unchanged', 'mermaid': {}}


class PlanTests(unittest.TestCase):
    def test_impacted_tests_are_selected_and_packages_become_scopes(self):
        decision = plan(['crates/kernel/src/lib.rs'],
                        ['//crates/kernel:tl3_kernel', '//crates/kernel:lib_test_tl3_kernel'])
        self.assertEqual(decision['selected'], ['//crates/kernel:lib_test_tl3_kernel'])
        self.assertEqual(decision['scopes'], ['//crates/kernel'])
        self.assertFalse(decision['all_scopes'])

    def test_build_and_lock_and_workflow_changes_test_everything(self):
        for path in ['MODULE.bazel', 'MODULE.bazel.lock', '.bazelrc', '.bazelversion', 'Cargo.lock',
                     'workers/uv.lock', 'crates/store/Cargo.toml', '.github/workflows/gate.yml',
                     'tools/bazel/rust.bzl', 'rust-toolchain.toml']:
            with self.subTest(path=path):
                decision = plan([path, 'crates/kernel/src/lib.rs'])
                self.assertEqual(decision['selection'], 'fallback')
                self.assertEqual(decision['selected'], sorted(TESTS))
                self.assertTrue(decision['all_scopes'])

    def test_docs_only_change_selects_nothing_and_passes(self):
        decision = plan(['docs/bazel.md', 'README.md', 'deploy/dl380g10/GO-LIVE.md'])
        self.assertEqual(decision['selection'], 'irrelevant')
        self.assertEqual(decision['selected'], [])
        self.assertEqual(decision['scopes'], [])

    def test_a_used_markdown_file_still_selects_its_tests(self):
        decision = plan(['crates/kernel/README.md'], ['//crates/kernel:lib_test_tl3_kernel'])
        self.assertEqual(decision['selected'], ['//crates/kernel:lib_test_tl3_kernel'])

    def test_unread_build_relevant_file_fails_with_the_fix(self):
        with self.assertRaises(bazel_gate.GateError) as failure:
            plan(['crates/kernel/src/lib.rs', 'deploy/new/config.toml'], ['//crates/kernel:tl3_kernel'])
        self.assertIn('deploy/new/config.toml', str(failure.exception))
        self.assertIn('docs/bazel.md', str(failure.exception))

    def test_relevant_change_with_no_impacted_targets_fails(self):
        with self.assertRaisesRegex(bazel_gate.GateError, 'no impacted targets'):
            plan(['crates/kernel/src/lib.rs'], [])

    def test_impacted_targets_without_tests_are_still_built(self):
        decision = plan(['crates/kernel/src/lib.rs'],
                        ['//crates/kernel:tl3_kernel', '//crates/kernel:lib_test_tl3_kernel'])
        self.assertEqual(decision['build'], ['//crates/kernel:tl3_kernel'])
        self.assertEqual(plan(['MODULE.bazel'])['build'], ['//...'])
        self.assertEqual(plan(['docs/bazel.md'])['build'], [])

    def test_gazelle_settings_force_everything(self):
        self.assertEqual(plan(['gazelle_python.yaml'])['selection'], 'fallback')

    def test_adding_a_python_test_with_generated_build_files_selects_it(self):
        # The documented flow: add workers/tests/test_turns.py, run tools/bazel/generate.sh.
        used = USED | {'workers/tests/test_turns.py', 'workers/tests/BUILD.bazel', 'workers/src/BUILD.bazel'}
        tests = TESTS | {'//workers/tests:test_turns'}
        decision = bazel_gate.plan(
            {'workers/tests/test_turns.py', 'workers/tests/BUILD.bazel', 'workers/src/BUILD.bazel'}, set(),
            {'//workers/tests:test_turns'}, used, tests)
        self.assertEqual(decision['selected'], ['//workers/tests:test_turns'])

    def test_a_build_file_edit_bazel_diff_cannot_see_tests_everything(self):
        used = USED | {'crates/kernel/BUILD.bazel', 'tools/bazel/rust.bzl'}
        for path in ['crates/kernel/BUILD.bazel', 'tools/bazel/rust.bzl']:
            with self.subTest(path=path):
                decision = bazel_gate.plan({path}, set(), set(), used, TESTS)
                self.assertEqual(decision['selection'], 'fallback')
                self.assertEqual(decision['selected'], sorted(TESTS))

    def test_empty_diff_fails(self):
        with self.assertRaisesRegex(bazel_gate.GateError, 'empty diff'):
            plan([])


class BazelDiffCommandTests(unittest.TestCase):
    def test_native_commands_export_edges_and_select_with_distances(self):
        generate, impacted = bazel_gate.diff_commands('diff', '/repo', 'bazel',
                                                      '/t/base.json', '/t/head.json', '/t/out.json', '/t/deps.json')
        command = generate('/t/base.json', '/t/base-deps.json')
        self.assertEqual(command[:2], ['diff', 'generate-hashes'])
        self.assertIn('--includeTargetType', command)
        self.assertEqual(command[command.index('--depEdgesFile') + 1], '/t/base-deps.json')
        self.assertIn('get-impacted-targets', impacted)
        self.assertEqual(impacted[impacted.index('--depEdgesFile') + 1], '/t/deps.json')
        for flag, value in [('--workspacePath', '/repo'), ('--bazelPath', 'bazel'),
                            ('--targetType', 'Rule'), ('--startingHashes', '/t/base.json'),
                            ('--finalHashes', '/t/head.json'), ('--output', '/t/out.json')]:
            self.assertEqual(impacted[impacted.index(flag) + 1], value)



class SkipDiffTests(unittest.TestCase):
    def test_only_unread_irrelevant_files_skip_bazel_diff(self):
        self.assertTrue(bazel_gate.needs_no_diff({'docs/a.md', 'README.md'}, set(), USED))
        self.assertFalse(bazel_gate.needs_no_diff({'crates/kernel/README.md'}, set(), USED))
        self.assertFalse(bazel_gate.needs_no_diff({'docs/a.md'}, {'docs/b.md'}, USED))
        self.assertFalse(bazel_gate.needs_no_diff({'crates/kernel/src/lib.rs'}, set(), USED))


class ClassifyTests(unittest.TestCase):
    RUST = {'//crates/kernel:lib_test_tl3_kernel'}

    def test_missing_status_is_a_failure(self):
        verdicts, failed = bazel_gate.classify(
            ['//crates/kernel:lib_test_tl3_kernel', '//harness/tests:test_harness'],
            {'//harness/tests:test_harness': 'PASSED'}, self.RUST)
        self.assertEqual(verdicts, {'rust': 'fail', 'python': 'pass'})
        self.assertEqual(failed['rust'], ['//crates/kernel:lib_test_tl3_kernel'])

    def test_unselected_language_is_skipped_not_failed(self):
        verdicts, _ = bazel_gate.classify(['//harness/tests:test_harness'],
                                          {'//harness/tests:test_harness': 'PASSED'}, self.RUST)
        self.assertEqual(verdicts, {'rust': 'skipped', 'python': 'pass'})

    def test_bep_statuses_are_read_per_label(self):
        with tempfile.TemporaryDirectory() as directory:
            bep = Path(directory, 'bep.json')
            bep.write_text('\n'.join(json.dumps(event) for event in [
                {'id': {'started': {}}},
                {'id': {'testSummary': {'label': '//a:t'}}, 'testSummary': {'overallStatus': 'FAILED'}},
                {'id': {'testSummary': {'label': '//b:t'}}, 'testSummary': {'overallStatus': 'PASSED'}},
            ]))
            self.assertEqual(bazel_gate.test_statuses(bep), {'//a:t': 'FAILED', '//b:t': 'PASSED'})
            self.assertEqual(bazel_gate.test_statuses(Path(directory, 'missing.json')), {})


class FailFastTests(unittest.TestCase):
    def result(self, label='//a:failed', status='FAILED'):
        return {'id': {'testResult': {'label': label}}, 'testResult': {'status': status}}

    def test_batch_assertion_failure_cancels_immediately(self):
        watcher = bazel_gate.FailFast('batch-draft')
        watcher.event(self.result(), 10)
        self.assertEqual(watcher.failure, {'label': '//a:failed', 'reason': 'FAILED'})
        self.assertTrue(watcher.should_cancel(10))

    def test_timeout_is_a_definitive_failure(self):
        watcher = bazel_gate.FailFast('batch-draft')
        watcher.event(self.result(status='TIMEOUT'), 10)
        self.assertTrue(watcher.should_cancel(10))
        self.assertEqual(watcher.failure, {'label': '//a:failed', 'reason': 'TIMEOUT'})

    def test_budget_warning_and_quarantined_failures_do_not_cancel(self):
        for status, quarantine in [('PASSED', set()), ('FAILED', {'//a:failed'}),
                                    ('TIMEOUT', {'//a:failed'})]:
            with self.subTest(status=status, quarantine=quarantine):
                watcher = bazel_gate.FailFast('batch-draft', quarantine=quarantine)
                watcher.event(self.result(status=status), 10)
                self.assertFalse(watcher.should_cancel(1000))
                self.assertIsNone(watcher.failure)

    def test_no_status_summary_does_not_count_as_a_test_that_ran(self):
        watcher = bazel_gate.FailFast('pr')
        watcher.event({'id': {'testSummary': {'label': '//a:pending'}},
                       'testSummary': {'overallStatus': 'NO_STATUS'}}, 0)
        self.assertEqual(watcher.observed, set())

    def test_pr_collects_failures_for_at_most_sixty_seconds(self):
        watcher = bazel_gate.FailFast('pr')
        watcher.event(self.result(), 10)
        self.assertFalse(watcher.should_cancel(69.9))
        watcher.event(self.result('//a:another'), 69)
        self.assertEqual(watcher.failures, {'//a:failed', '//a:another'})
        self.assertTrue(watcher.should_cancel(70))
        self.assertEqual(watcher.failure['label'], '//a:failed')

    def test_main_never_cancels(self):
        watcher = bazel_gate.FailFast('main')
        watcher.event(self.result(), 0)
        watcher.event({'id': {'targetCompleted': {'label': '//a:broken'}},
                       'completed': {'success': False}}, 1)
        self.assertFalse(watcher.should_cancel(1000))

    def test_build_failure_cancels_but_failed_test_completion_is_not_a_build_failure(self):
        watcher = bazel_gate.FailFast('batch-draft')
        watcher.event({'id': {'targetCompleted': {'label': '//a:test'}},
                       'completed': {'success': True}}, 1)
        self.assertFalse(watcher.should_cancel(1))
        watcher.event({'id': {'targetCompleted': {'label': '//a:broken'}},
                       'completed': {'success': False}}, 2)
        self.assertEqual(watcher.failure, {'label': '//a:broken', 'reason': 'BUILD_FAILURE'})
        self.assertTrue(watcher.should_cancel(2))

    def test_build_failure_uses_proto_default_when_false_is_omitted(self):
        watcher = bazel_gate.FailFast('batch-draft')
        watcher.event({'id': {'targetCompleted': {'label': '//a:broken'}}, 'completed': {}}, 0)
        self.assertEqual(watcher.failure, {'label': '//a:broken', 'reason': 'BUILD_FAILURE'})
        self.assertTrue(watcher.should_cancel(0))

    def test_stream_reads_only_complete_new_lines(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory, 'bep.json')
            stream = bazel_gate.BepStream(path)
            self.assertEqual(stream.read(), [])
            line = json.dumps(self.result())
            path.write_text(line[:20])
            self.assertEqual(stream.read(), [])
            with path.open('a') as output:
                output.write(line[20:] + '\n')
            self.assertEqual(stream.read(), [self.result()])
            self.assertEqual(stream.read(), [])

    def test_abort_fails_gate_even_if_all_summaries_passed(self):
        self.assertFalse(bazel_gate.passed({'mode': 'batch-draft', 'verdicts': {'rust': 'pass'},
                                          'aborted_early': {'label': '//a:broken',
                                                            'reason': 'BUILD_FAILURE'}}))

    def test_abort_skips_followup_work_and_records_missing(self):
        label = '//crates/kernel:lib_test_tl3_kernel'
        workspace = FakeWorkspace(['MODULE.bazel'], [], {label: 'FAILED'})
        workspace.aborted_early = {'label': label, 'reason': 'FAILED'}
        workspace.test = lambda *args, **kwargs: {label: 'FAILED'}
        workspace.inputs_check = lambda: self.fail('checked inputs after abort')
        result = bazel_gate.gate_pr(workspace, 'a' * 40, 'b' * 40, {}, mode='batch-draft')
        self.assertEqual(result['aborted_early'], workspace.aborted_early)
        self.assertEqual(result['not_run'], sorted(TESTS - {label}))
        self.assertEqual(workspace.built, [])

    def test_pr_finishes_naturally_within_grace_without_interrupt(self):
        from unittest.mock import patch
        workspace = bazel_gate.Workspace(mode='pr')
        with patch.object(bazel_gate, 'BepStream') as stream, \
                patch.object(bazel_gate.subprocess, 'Popen') as popen, \
                patch.object(bazel_gate.time, 'sleep'):
            stream.return_value.read.side_effect = [[self.result()], [self.result('//a:another')], []]
            process = popen.return_value.__enter__.return_value
            process.poll.side_effect = [None, 3]
            workspace.test(['//a:failed', '//a:another'], 'remote', {}, 'pr')
            process.send_signal.assert_not_called()
        self.assertIsNone(workspace.aborted_early)
        self.assertEqual(workspace.not_run, [])

    def test_workspace_interrupts_bazel_and_reaps_it(self):
        import signal
        from unittest.mock import patch
        workspace = bazel_gate.Workspace(mode='batch-draft')
        events = [self.result()]
        with patch.object(bazel_gate, 'BepStream') as stream, \
                patch.object(bazel_gate.subprocess, 'Popen') as popen:
            stream.return_value.read.side_effect = lambda: events.copy() if events else []
            process = popen.return_value.__enter__.return_value
            process.poll.return_value = None
            process.wait.return_value = 8
            workspace.test(['//a:failed', '//a:pending'], 'remote', {}, 'pr')
            process.send_signal.assert_called_once_with(signal.SIGINT)
            process.wait.assert_called()
        self.assertEqual(workspace.aborted_early, {'label': '//a:failed', 'reason': 'FAILED'})


class GateTests(unittest.TestCase):
    def test_mergify_batch_keeps_the_supplied_base_for_graph_and_selection(self):
        batch_base, batch_head = 'c' * 40, 'd' * 40
        workspace = FakeWorkspace(['crates/kernel/src/lib.rs'], [], {})
        calls = []
        def graph_check(base, head):
            calls.append(('graph', base, head))
            return {'errors': [], 'summary': 'unchanged'}
        def bazel_diff(base, head):
            calls.append(('selection', base, head))
            return {'//crates/kernel:lib_test_tl3_kernel'}
        workspace.graph_check = graph_check
        workspace.bazel_diff = bazel_diff
        bazel_gate.gate_pr(workspace, batch_base, batch_head, {})
        self.assertEqual(calls, [('graph', batch_base, batch_head),
                                 ('selection', batch_base, batch_head)])

    def test_graph_regression_fails_even_a_docs_only_pr(self):
        workspace = FakeWorkspace(['docs/bazel.md'], [], {})
        calls = []
        def graph_check(base, head):
            calls.append((base, head))
            return {'errors': ['U worsened'], 'summary': 'U 10% → 20%', 'mermaid': {}}
        workspace.graph_check = graph_check
        result = bazel_gate.gate_pr(workspace, 'a' * 40, 'b' * 40, {})
        self.assertEqual(calls, [('a' * 40, 'b' * 40)])
        self.assertEqual(result['graph_impact']['errors'], ['U worsened'])
        self.assertFalse(bazel_gate.passed({**result, 'mode': 'pr'}))

    def test_pr_gate_runs_selection_on_amd64_and_reports_failures(self):
        workspace = FakeWorkspace(['crates/kernel/src/lib.rs'], ['//crates/kernel:lib_test_tl3_kernel'],
                                  {'//crates/kernel:lib_test_tl3_kernel': 'FAILED'})
        result = bazel_gate.gate_pr(workspace, 'a' * 40, 'b' * 40, {})
        self.assertEqual(workspace.ran, [('remote', ['//crates/kernel:lib_test_tl3_kernel'])])
        self.assertEqual(result['verdicts'], {'rust': 'fail', 'python': 'skipped'})
        self.assertFalse(bazel_gate.passed({**result, 'mode': 'pr', 'error': None}))

    def test_pr_gate_builds_impacted_targets_that_no_test_covers(self):
        # Only a binary changed: no test is selected, but a broken build must still fail.
        for build_ok in [True, False]:
            with self.subTest(build_ok=build_ok):
                workspace = FakeWorkspace(['crates/kernel/src/lib.rs'], ['//crates/kernel:tl3_kernel'], {},
                                          build_ok=build_ok)
                result = bazel_gate.gate_pr(workspace, 'a' * 40, 'b' * 40, {})
                self.assertEqual(workspace.built, [('remote', ['//crates/kernel:tl3_kernel'])])
                self.assertEqual(bazel_gate.passed({**result, 'mode': 'pr'}), build_ok)

    def test_inputs_check_includes_every_generated_input_check(self):
        workspace = bazel_gate.Workspace(bazel='bazel-under-test')
        commands = []
        workspace.run = lambda *command, **_: commands.append(' '.join(map(str, command))) or 0
        workspace.inputs_check()
        for needle in ['bazel_parity.py static', 'requirements.py --check', 'rust_builds.py --check',
                       'bazel-under-test run //:gazelle -- -mode=diff']:
            self.assertTrue(any(needle in command for command in commands), needle)

    def test_test_and_build_keep_their_own_diagnostic_logs(self):
        # Later helper invocations write the default log names; the test run's logs must survive.
        workspace = bazel_gate.Workspace(bazel='bazel')
        commands = []
        workspace.run = lambda *command, **_: commands.append(list(map(str, command))) or 0
        workspace.test(['//a:t'], 'remote', {}, 'pr')
        workspace.build(['//a:b'], 'remote', {}, 'pr')
        for command, prefix in zip(commands, ['pr', 'pr-build']):
            self.assertIn(f'--remote_grpc_log={prefix}-bazel-remote-grpc.log', command)
            self.assertIn(f'--execution_log_compact_file={prefix}-execution_log.binpb.zst', command)

    def test_every_diagnostic_log_reaches_the_artifacts(self):
        with tempfile.TemporaryDirectory() as root, tempfile.TemporaryDirectory() as output:
            names = ['bazel-remote-grpc.log', 'pr-bazel-remote-grpc.log', 'pr-execution_log.binpb.zst']
            for name in names:
                Path(root, name).write_text('log')
            bazel_gate.write_result({'mode': 'pr'}, Path(output), root=Path(root))
            self.assertEqual(sorted(path.name for path in Path(output).iterdir()),
                             sorted(names + [bazel_gate.RESULT]))

    def test_used_sources_include_the_build_files_bazel_reads(self):
        workspace = bazel_gate.Workspace(bazel='bazel')
        answers = {'kind("source file", deps(//...))': ['//workers/tests:test_turns.py', '@pip//x:y.py'],
                   'buildfiles(//...)': ['//workers/tests:BUILD.bazel', '//tools/bazel:python.bzl',
                                         '@rules_python//python:defs.bzl']}
        workspace.query = lambda expression: answers[expression]
        self.assertEqual(workspace.used_sources(), {'workers/tests/test_turns.py', 'workers/tests/BUILD.bazel',
                                                    'tools/bazel/python.bzl'})

    def test_passing_overrun_warns_in_pr_and_batch_without_a_second_test_run(self):
        label = '//crates/kernel:lib_test_tl3_kernel'
        warning = {'label': label, 'test': 'b::slow', 'measured': 56, 'limit': 30, 'metric': 'wall'}
        for mode, metadata in [('pr', {}), ('batch-draft', {'PULL_REQUEST_NUMBER': '12,13'})]:
            with self.subTest(mode=mode):
                workspace = FakeWorkspace(['crates/kernel/src/lib.rs'], [label], {label: 'PASSED'},
                                          warnings=[warning])
                result = bazel_gate.gate_pr(workspace, 'a' * 40, 'b' * 40, metadata, mode=mode)
                self.assertEqual(workspace.ran, [('remote', [label])])
                self.assertEqual(result['budget_warnings'], [warning])
                self.assertTrue(bazel_gate.passed({**result, 'mode': 'pr', 'error': None}))

    def test_assertions_and_timeouts_fail_without_a_second_test_run(self):
        label = '//crates/kernel:lib_test_tl3_kernel'
        for status in ['FAILED', 'TIMEOUT']:
            with self.subTest(status=status):
                workspace = FakeWorkspace(['crates/kernel/src/lib.rs'], [label], {label: status})
                result = bazel_gate.gate_pr(workspace, 'a' * 40, 'b' * 40, {})
                self.assertEqual(workspace.ran, [('remote', [label])])
                self.assertFalse(bazel_gate.passed({**result, 'mode': 'pr', 'error': None}))

    def test_main_collects_warnings_before_next_architecture_and_never_retries(self):
        label = '//crates/store:test_test_postgres'
        warning = {'label': label, 'test': 'slow', 'measured': 43, 'limit': 30, 'metric': 'wall'}
        workspace = FakeWorkspace(['crates/kernel/src/lib.rs'], ['//crates/kernel:lib_test_tl3_kernel'],
                                  {label: 'TIMEOUT'}, warnings=[warning])
        result = bazel_gate.gate_main(workspace, 'a' * 40, 'b' * 40, {})
        self.assertEqual([config for config, _ in workspace.ran], ['remote', 'remote-arm64'])
        for architecture in ['amd64', 'arm64']:
            self.assertEqual(result['architectures'][architecture]['budget_warnings'], [warning])
        self.assertEqual(result['budget_warnings'],
                         [{**warning, 'architecture': arch} for arch in ['amd64', 'arm64']])
        self.assertFalse(bazel_gate.passed({**result, 'mode': 'main', 'error': None}))

    def test_warnings_are_read_from_plain_or_zipped_test_outputs(self):
        import zipfile
        warning = {'test': 'slow', 'measured': 56, 'limit': 30, 'metric': 'wall'}
        with tempfile.TemporaryDirectory() as root:
            plain = Path(root, 'bazel-testlogs/crates/a/t/test.outputs')
            plain.mkdir(parents=True)
            (plain / 'budget-warnings.json').write_text(json.dumps([warning]))
            zipped = Path(root, 'bazel-testlogs/crates/a/z/test.outputs')
            zipped.mkdir(parents=True)
            with zipfile.ZipFile(zipped / 'outputs.zip', 'w') as archive:
                archive.writestr('budget-warnings.json', json.dumps([warning]))
            workspace = bazel_gate.Workspace(root=Path(root))
            self.assertEqual(workspace.budget_warnings(['//crates/a:t', '//crates/a:z', '//crates/a:real']),
                             [{**warning, 'label': label} for label in ['//crates/a:t', '//crates/a:z']])

    def test_runner_records_warnings_and_preserves_the_budget_growth_check(self):
        import sys
        sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'bazel'))
        import rust_runner
        config = {'seconds': 30.0, 'slow': {}}
        with tempfile.TemporaryDirectory() as outputs:
            rust_runner.enforce_budget({'b::slow': 56.0}, config, 'baseline', outputs)
            self.assertEqual(json.loads(Path(outputs, 'budget-warnings.json').read_text()),
                             [{'test': 'b::slow', 'measured': 56.0, 'limit': 30.0, 'metric': 'wall'}])
            self.assertFalse(Path(outputs, 'budget-overrun.txt').exists())
        with tempfile.TemporaryDirectory() as outputs:
            with self.assertRaisesRegex(RuntimeError, 'may not grow'):
                rust_runner.enforce_budget({'b::fast': 1.0}, {'seconds': 99.0, 'slow': {}}, 'baseline', outputs,
                                           baseline=config)
            self.assertEqual(list(Path(outputs).iterdir()), [])

    def test_runner_warns_for_cpu_and_wall_backstop_overruns(self):
        import sys
        sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'bazel'))
        import rust_runner
        config = {'seconds': 10, 'slow': {}, 'cpu': {'seconds': 2, 'slow': {}}}
        for wall, cpu, metrics in [(1, 6, ['cpu']), (301, .1, ['wall_backstop']),
                                   (301, 6, ['cpu', 'wall_backstop'])]:
            with self.subTest(wall=wall, cpu=cpu), tempfile.TemporaryDirectory() as outputs:
                rust_runner.enforce_budget({'b::scenario': wall}, config, 'buildbuddy-amd64',
                                           outputs, cpu_seconds={'b::scenario': cpu})
                warnings = json.loads(Path(outputs, 'budget-warnings.json').read_text())
                self.assertEqual([warning['metric'] for warning in warnings], metrics)
                self.assertFalse(Path(outputs, 'budget-overrun.txt').exists())

    def test_bazel_diff_error_fails_the_gate(self):
        workspace = FakeWorkspace(['crates/kernel/src/lib.rs'], [], {}, diff_error='bazel-diff exited 1')
        with self.assertRaisesRegex(bazel_gate.GateError, 'bazel-diff'):
            bazel_gate.gate_pr(workspace, 'a' * 40, 'b' * 40, {})

    def test_empty_base_fails_the_gate(self):
        workspace = FakeWorkspace(['crates/kernel/src/lib.rs'], [], {})
        with self.assertRaisesRegex(bazel_gate.GateError, 'empty base'):
            bazel_gate.gate_pr(workspace, '', 'b' * 40, {})

    def test_main_runs_everything_on_both_architectures_and_flags_misses(self):
        workspace = FakeWorkspace(['crates/kernel/src/lib.rs'], ['//crates/kernel:lib_test_tl3_kernel'],
                                  {'//crates/store:test_test_postgres': 'FAILED'})
        result = bazel_gate.gate_main(workspace, 'a' * 40, 'b' * 40, {})
        self.assertEqual([config for config, _ in workspace.ran], ['remote', 'remote-arm64'])
        self.assertTrue(all(sorted(targets) == sorted(TESTS) for _, targets in workspace.ran))
        self.assertEqual(result['architectures']['amd64']['missed'], ['//crates/store:test_test_postgres'])
        self.assertFalse(bazel_gate.passed({**result, 'mode': 'main', 'error': None}))

    def test_main_still_runs_everything_when_selection_fails(self):
        workspace = FakeWorkspace(['crates/kernel/src/lib.rs'], [], {}, diff_error='bazel-diff exited 1')
        result = bazel_gate.gate_main(workspace, 'a' * 40, 'b' * 40, {})
        self.assertEqual(result['selection'], 'error')
        self.assertEqual(len(workspace.ran), 2)

    def test_stale_inputs_fail_even_when_tests_pass(self):
        result = {'mode': 'pr', 'error': None, 'inputs': ['bazel run //:gazelle -- -mode=diff'],
                  'verdicts': {'rust': 'pass', 'python': 'pass'}}
        self.assertFalse(bazel_gate.passed(result))


class Review606BindingTests(unittest.TestCase):
    def test_review606_bazel_receives_the_exact_ci_binding_metadata(self):
        arguments = ['bazel_gate.py', 'pr', '--base', 'a' * 40, '--head', 'b' * 40]
        binding = {'TL3_CI_RUN_ID': '6', 'TL3_CI_RUN_ATTEMPT': '2', 'TL3_CI_CHECK_NAME': 'Bazel'}
        with patch.dict(os.environ, binding), patch.object(bazel_gate.sys, 'argv', arguments), \
                patch.object(bazel_gate, 'Workspace'), patch.object(bazel_gate, 'write_result'), \
                patch.object(bazel_gate, 'gate_pr', return_value={'verdicts': {'rust': 'pass'}}) as gate:
            self.assertEqual(bazel_gate.main(), 0)
        metadata = gate.call_args.args[3]
        for key, value in binding.items():
            self.assertEqual(metadata.get(key), value, 'Bazel results must retain the trusted CI binding')


if __name__ == '__main__':
    unittest.main()
