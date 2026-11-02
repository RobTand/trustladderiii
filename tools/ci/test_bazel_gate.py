"""Selection must fail closed: any doubt about what to test fails the gate."""
# rust_runner is loaded by path from the //tools/bazel:sources data, not as a py dependency.
# gazelle:ignore rust_runner
import json
from pathlib import Path
import tempfile
import unittest

import bazel_gate

TESTS = {'//crates/kernel:lib_test_tl3_kernel', '//crates/store:test_test_postgres',
         '//harness/tests:test_harness'}
USED = {'crates/kernel/src/lib.rs', 'crates/store/src/lib.rs', 'harness/src/tl3_harness/cli.py',
        'crates/kernel/README.md'}


def plan(changed, impacted=(), deleted=()):
    return bazel_gate.plan(set(changed), set(deleted), set(impacted), USED, TESTS)


class FakeWorkspace:
    """Records calls; answers from canned git and Bazel facts."""

    def __init__(self, changed, impacted, statuses, diff_error=None, build_ok=True, budget=None):
        self.changed, self.impacted, self.statuses = changed, impacted, statuses
        self.diff_error, self.ran, self.built, self.build_ok = diff_error, [], [], build_ok
        # label -> status of its one budget rerun; only these failures are budget-only.
        self.budget = budget or {}

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

    def budget_only(self, labels):
        return {label for label in labels if label in self.budget}

    def test(self, targets, config, metadata, log_name, extra=()):
        self.ran.append((config, list(targets)))
        if log_name.endswith('budget-rerun'):
            assert '--nocache_test_results' in extra
            return {label: self.budget[label] for label in targets}
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


class GateTests(unittest.TestCase):
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

    def test_a_budget_only_failure_is_rerun_once_and_can_pass(self):
        label = '//crates/kernel:lib_test_tl3_kernel'
        workspace = FakeWorkspace(['crates/kernel/src/lib.rs'], [label], {label: 'FAILED'},
                                  budget={label: 'PASSED'})
        result = bazel_gate.gate_pr(workspace, 'a' * 40, 'b' * 40, {})
        self.assertEqual(workspace.ran, [('remote', [label]), ('remote', [label])])
        self.assertEqual(result['verdicts']['rust'], 'pass')
        self.assertEqual(result['budget_reruns'], {label: 'PASSED'})

    def test_a_confirmed_budget_overrun_still_fails(self):
        label = '//crates/kernel:lib_test_tl3_kernel'
        workspace = FakeWorkspace(['crates/kernel/src/lib.rs'], [label], {label: 'FAILED'},
                                  budget={label: 'FAILED'})
        result = bazel_gate.gate_pr(workspace, 'a' * 40, 'b' * 40, {})
        self.assertEqual(result['verdicts']['rust'], 'fail')

    def test_real_failures_are_never_retried(self):
        label = '//crates/kernel:lib_test_tl3_kernel'
        workspace = FakeWorkspace(['crates/kernel/src/lib.rs'], [label], {label: 'FAILED'})
        result = bazel_gate.gate_pr(workspace, 'a' * 40, 'b' * 40, {})
        self.assertEqual(len(workspace.ran), 1)
        self.assertEqual(result['budget_reruns'], {})

    def test_post_merge_runs_rerun_budget_only_failures_per_architecture(self):
        label = '//crates/store:test_test_postgres'
        workspace = FakeWorkspace(['crates/kernel/src/lib.rs'], ['//crates/kernel:lib_test_tl3_kernel'],
                                  {label: 'FAILED'}, budget={label: 'PASSED'})
        result = bazel_gate.gate_main(workspace, 'a' * 40, 'b' * 40, {})
        self.assertEqual(result['architectures']['amd64']['missed'], [])
        self.assertEqual(result['architectures']['amd64']['budget_reruns'], {label: 'PASSED'})
        self.assertTrue(bazel_gate.passed({**result, 'mode': 'main', 'error': None}))

    def test_budget_markers_are_read_from_plain_or_zipped_test_outputs(self):
        import zipfile
        with tempfile.TemporaryDirectory() as root:
            plain = Path(root, 'bazel-testlogs/crates/a/t/test.outputs')
            plain.mkdir(parents=True)
            (plain / bazel_gate.BUDGET_MARKER).write_text('Per-test time budget exceeded')
            zipped = Path(root, 'bazel-testlogs/crates/a/z/test.outputs')
            zipped.mkdir(parents=True)
            with zipfile.ZipFile(zipped / 'outputs.zip', 'w') as archive:
                archive.writestr(bazel_gate.BUDGET_MARKER, 'Per-test time budget exceeded')
            Path(root, 'bazel-testlogs/crates/a/real/test.outputs').mkdir(parents=True)
            workspace = bazel_gate.Workspace(root=Path(root))
            self.assertEqual(workspace.budget_only(['//crates/a:t', '//crates/a:z', '//crates/a:real']),
                             {'//crates/a:t', '//crates/a:z'})

    def test_runner_marks_only_budget_overruns(self):
        import sys
        sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'bazel'))
        import rust_runner
        config = {'seconds': 10.0, 'slow': {}}
        with tempfile.TemporaryDirectory() as outputs:
            with self.assertRaisesRegex(RuntimeError, 'budget exceeded'):
                rust_runner.enforce_budget({'b::slow': 48.0}, config, 'baseline', outputs)
            self.assertTrue(Path(outputs, bazel_gate.BUDGET_MARKER).is_file())
        with tempfile.TemporaryDirectory() as outputs:
            with self.assertRaisesRegex(RuntimeError, 'may not grow'):
                rust_runner.enforce_budget({'b::fast': 1.0}, {'seconds': 99.0, 'slow': {}}, 'baseline', outputs,
                                           baseline=config)
            self.assertFalse(Path(outputs, bazel_gate.BUDGET_MARKER).exists())

    def test_runner_reruns_cpu_overruns_but_never_wall_backstops(self):
        import sys
        sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'bazel'))
        import rust_runner
        config = {'seconds': 10, 'slow': {}, 'cpu': {'seconds': 2, 'slow': {}}}
        for wall, cpu, retry in [(1, 6, True), (166, .1, False), (166, 6, False)]:
            with self.subTest(wall=wall, cpu=cpu), tempfile.TemporaryDirectory() as outputs:
                with self.assertRaises(RuntimeError):
                    rust_runner.enforce_budget({'b::scenario': wall}, config, 'buildbuddy-amd64',
                                               outputs, cpu_seconds={'b::scenario': cpu})
                self.assertEqual(Path(outputs, bazel_gate.BUDGET_MARKER).exists(), retry)

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


if __name__ == '__main__':
    unittest.main()
