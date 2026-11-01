"""The Bazel migration must preserve the gate's discovered tests."""
import unittest

import bazel_parity


class ParityTests(unittest.TestCase):
    def test_equal_inventories(self):
        inventory = {'rust': {'tl3-kernel/lib/tl3_kernel': ['replay::empty']},
                     'python': ['harness/tests/test_harness.py']}
        bazel_parity.compare(inventory, inventory)

    def test_missing_rust_binary_fails(self):
        with self.assertRaisesRegex(RuntimeError, 'tl3-kernel'):
            bazel_parity.compare({'rust': {'tl3-kernel/lib/tl3_kernel': ['a']}, 'python': []},
                                 {'rust': {}, 'python': []})

    def test_missing_rust_test_name_fails(self):
        with self.assertRaisesRegex(RuntimeError, 'second'):
            bazel_parity.compare({'rust': {'binary': ['first', 'second']}, 'python': []},
                                 {'rust': {'binary': ['first']}, 'python': []})

    def test_extra_tests_fail_too(self):
        with self.assertRaisesRegex(RuntimeError, 'extra'):
            bazel_parity.compare({'rust': {}, 'python': []},
                                 {'rust': {}, 'python': ['extra.py']})

    def test_empty_inventory_fails(self):
        with self.assertRaisesRegex(RuntimeError, 'empty'):
            bazel_parity.compare({'rust': {}, 'python': []}, {'rust': {}, 'python': []})

    def test_python_discovery_uses_the_existing_gate(self):
        files = ['harness/pyproject.toml', 'harness/tests/test_harness.py',
                 'harness/tests/fixtures/test_broken.py', 'deploy/github-app/test_setup.py',
                 'tools/ci/test_bazel_parity.py']
        self.assertEqual(bazel_parity.python_modules(files), [
            'deploy/github-app/test_setup.py', 'harness/tests/test_harness.py',
            'tools/ci/test_bazel_parity.py'])

    def test_custom_harness_receipt_requires_a_complete_scenario_count(self):
        self.assertEqual(bazel_parity.custom_names('test first ... ok\n1 example scenarios passed\n'), ['first'])
        with self.assertRaisesRegex(RuntimeError, 'scenario'):
            bazel_parity.custom_names('test first ... ok\n2 example scenarios passed\n')

    def test_duplicate_test_names_fail(self):
        inventory = {'rust': {'binary': ['same', 'same']}, 'python': ['test.py']}
        with self.assertRaisesRegex(RuntimeError, 'duplicate'):
            bazel_parity.compare(inventory, inventory)


if __name__ == '__main__':
    unittest.main()
