"""The seeded-break check must cover the required places and judge honestly."""
from pathlib import Path
import tempfile
import unittest

import bazel_gate
import seeded_breaks


class SeedTests(unittest.TestCase):
    def test_seeds_cover_every_required_place(self):
        paths = [path for _, path, _, _ in seeded_breaks.SEEDS]
        for crate in ['worktrees', 'config', 'store', 'controller', 'kernel', 'github']:
            self.assertTrue(any(path.startswith(f'crates/{crate}/src/') for path in paths), crate)
        self.assertTrue(any(path.startswith('harness/src/') for path in paths))
        self.assertTrue(any(path.startswith('workers/src/') for path in paths))
        self.assertTrue(any('/fixtures/' in path for path in paths))
        self.assertGreaterEqual(len(paths), 8)

    def test_mutations_change_the_file_and_missing_targets_fail(self):
        with tempfile.TemporaryDirectory() as root:
            Path(root, 'lib.rs').write_text('pub fn f() {}\n')
            seeded_breaks.mutate(root, 'lib.rs', ('append', seeded_breaks.COMPILE_ERROR))
            self.assertIn('compile_error!', Path(root, 'lib.rs').read_text())
            seeded_breaks.mutate(root, 'lib.rs', ('replace', 'x'))
            self.assertEqual(Path(root, 'lib.rs').read_text(), 'x')
            with self.assertRaises(bazel_gate.GateError):
                seeded_breaks.mutate(root, 'missing.rs', ('append', 'x'))


class VerdictTests(unittest.TestCase):
    def test_caught_only_when_an_expected_failing_target_was_selected(self):
        selected = ['//crates/store:test_test_postgres', '//harness/tests:test_harness']
        caught = seeded_breaks.verdict(selected, {'//harness/tests:test_harness': 'PASSED'}, '//crates/store')
        self.assertTrue(caught['caught'])
        self.assertEqual(caught['impacted_tests'], 2)
        missed = seeded_breaks.verdict(selected, {label: 'PASSED' for label in selected}, '//crates/store')
        self.assertFalse(missed['caught'])
        elsewhere = seeded_breaks.verdict(selected, {'//crates/store:test_test_postgres': 'PASSED'},
                                          '//crates/store')
        self.assertFalse(elsewhere['caught'])

    def test_worktrees_reach_requires_dependents_and_excludes_unrelated_crates(self):
        selected = ['//crates/worktrees:lib_test_tl3_worktrees', '//crates/config:test_test_config',
                    '//crates/store:test_test_postgres', '//crates/controller:test_test_workers']
        self.assertTrue(seeded_breaks.reach(selected)['ok'])
        self.assertEqual(seeded_breaks.reach(selected[:-1])['missing'], ['//crates/controller:test_test_workers'])
        polluted = seeded_breaks.reach(selected + ['//crates/kernel:lib_test_tl3_kernel'])
        self.assertEqual(polluted['unexpected'], ['//crates/kernel:lib_test_tl3_kernel'])

    def test_table_marks_misses(self):
        rows = [{'place': 'a', 'impacted_tests': 3, 'failing_selected': ['//a:t'], 'caught': True},
                {'place': 'b', 'impacted_tests': 0, 'failing_selected': [], 'caught': False}]
        text = seeded_breaks.table(rows)
        self.assertIn('| a | 3 | yes | `//a:t` |', text)
        self.assertIn('| b | 0 | NO | `-` |', text)


if __name__ == '__main__':
    unittest.main()
