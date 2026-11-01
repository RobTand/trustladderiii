"""Selection must fail closed: any doubt about what to test fails the gate."""
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

    def __init__(self, changed, impacted, statuses, diff_error=None):
        self.changed, self.impacted, self.statuses = changed, impacted, statuses
        self.diff_error, self.ran = diff_error, []

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

    def test(self, targets, config, metadata, log_name):
        self.ran.append((config, list(targets)))
        return {label: self.statuses.get(label, 'PASSED') for label in targets}

    def inputs_check(self):
        return []

    def budget_check(self, base):
        return []


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

    def test_empty_diff_fails(self):
        with self.assertRaisesRegex(bazel_gate.GateError, 'empty diff'):
            plan([])


class BazelDiffCommandTests(unittest.TestCase):
    def test_v42_commands_name_the_workspace_and_rule_targets(self):
        generate, impacted = bazel_gate.diff_commands('java', 'diff.jar', '/repo', 'bazel',
                                                      '/t/base.json', '/t/head.json', '/t/out.txt')
        self.assertEqual(generate('/t/base.json')[3:], ['generate-hashes', '--workspacePath', '/repo',
                                                        '--bazelPath', 'bazel', '--excludeExternalTargets',
                                                        '--includeTargetType', '/t/base.json'])
        self.assertIn('get-impacted-targets', impacted)
        for flag, value in [('--workspacePath', '/repo'), ('--bazelPath', 'bazel'),
                            ('--targetType', 'Rule'), ('--startingHashes', '/t/base.json'),
                            ('--finalHashes', '/t/head.json'), ('--output', '/t/out.txt')]:
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
    def test_pr_gate_runs_selection_on_amd64_and_reports_failures(self):
        workspace = FakeWorkspace(['crates/kernel/src/lib.rs'], ['//crates/kernel:lib_test_tl3_kernel'],
                                  {'//crates/kernel:lib_test_tl3_kernel': 'FAILED'})
        result = bazel_gate.gate_pr(workspace, 'a' * 40, 'b' * 40, {})
        self.assertEqual(workspace.ran, [('remote', ['//crates/kernel:lib_test_tl3_kernel'])])
        self.assertEqual(result['verdicts'], {'rust': 'fail', 'python': 'skipped'})
        self.assertFalse(bazel_gate.passed({**result, 'mode': 'pr', 'error': None}))

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
