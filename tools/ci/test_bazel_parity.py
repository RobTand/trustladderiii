"""The Bazel migration must preserve the gate's discovered tests."""
import json
from pathlib import Path
import tempfile
import unittest

import bazel_parity


def package(root, name, targets, manifest=''):
    directory = Path(root, 'crates', name)
    directory.mkdir(parents=True)
    (directory / 'Cargo.toml').write_text(f'[package]\nname = "{name}"\n' + manifest)
    return {'name': name, 'manifest_path': str(directory / 'Cargo.toml'),
            'targets': [{'kind': [kind], 'name': target, 'src_path': str(directory / source)}
                        for kind, target, source in targets]}


def rule(name, main, args, tags=()):
    return json.dumps({'type': 'RULE', 'rule': {'name': name, 'attribute': [
        {'name': 'main', 'stringValue': main}, {'name': 'args', 'stringListValue': args},
        {'name': 'tags', 'stringListValue': list(tags)}]}})


class ParityTests(unittest.TestCase):
    def test_equal_inventories(self):
        inventory = {'rust': {'tl3-kernel/lib/tl3_kernel': ['replay::empty']},
                     'python': ['harness/tests/test_harness.py']}
        bazel_parity.compare(inventory, inventory)

    def test_missing_rust_binary_fails(self):
        with self.assertRaisesRegex(RuntimeError, 'tl3-kernel'):
            bazel_parity.compare({'rust': {'tl3-kernel/lib/tl3_kernel': ['a']}, 'python': []},
                                 {'rust': {}, 'python': []})

    def test_binaries_without_tests_need_no_cargo_timing(self):
        # rust_gate's timing file has no key for a binary with zero tests.
        bazel_parity.compare({'rust': {'binary': ['a']}, 'python': []},
                             {'rust': {'binary': ['a'], 'tl3-config/tl3_config': []}, 'python': []})
        with self.assertRaisesRegex(RuntimeError, 'empty'):
            bazel_parity.compare({'rust': {'binary': ['a'], 'empty': ['x']}, 'python': []},
                                 {'rust': {'binary': ['a'], 'empty': []}, 'python': []})

    def test_missing_rust_test_name_fails(self):
        with self.assertRaisesRegex(RuntimeError, 'second'):
            bazel_parity.compare({'rust': {'binary': ['first', 'second']}, 'python': []},
                                 {'rust': {'binary': ['first']}, 'python': []})

    def test_extra_tests_fail_too(self):
        with self.assertRaisesRegex(RuntimeError, 'extra'):
            bazel_parity.compare({'rust': {}, 'python': []},
                                 {'rust': {}, 'python': ['extra.py']})

    def test_missing_python_file_names_the_repair_command(self):
        with self.assertRaisesRegex(RuntimeError, 'workers/tests/test_new.py') as failure:
            bazel_parity.compare({'rust': {}, 'python': ['workers/tests/test_new.py']},
                                 {'rust': {}, 'python': []})
        self.assertIn('tools/bazel/generate.sh', str(failure.exception))

    def test_empty_inventory_fails(self):
        with self.assertRaisesRegex(RuntimeError, 'empty'):
            bazel_parity.compare({'rust': {}, 'python': []}, {'rust': {}, 'python': []})

    def test_python_discovery_uses_the_existing_gate(self):
        files = ['harness/pyproject.toml', 'harness/tests/test_harness.py',
                 'harness/tests/fixtures/test_broken.py', 'harness/src/tl3_harness/test_tool.py', 'deploy/github-app/test_setup.py',
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



class ReceiptTests(unittest.TestCase):
    def test_unzipped_and_zipped_receipts_are_both_read(self):
        import io
        import zipfile
        with tempfile.TemporaryDirectory() as root:
            plain = Path(root, 'crates/k/test_test_a/test.outputs')
            plain.mkdir(parents=True)
            (plain / 'inventory.json').write_text(json.dumps({'rust': {'k/a': ['one']}}))
            zipped = Path(root, 'workers/tests/test_b/test.outputs')
            zipped.mkdir(parents=True)
            data = io.BytesIO()
            with zipfile.ZipFile(data, 'w') as archive:
                archive.writestr('inventory.json', json.dumps({'python': ['workers/tests/test_b.py']}))
            (zipped / 'outputs.zip').write_bytes(data.getvalue())
            self.assertEqual(bazel_parity.bazel_receipts(Path(root)),
                             {'rust': {'k/a': ['one']}, 'python': ['workers/tests/test_b.py']})


class StaticParityTests(unittest.TestCase):
    def cargo(self, root):
        return bazel_parity.cargo_binaries({'packages': [
            package(root, 'tl3-controller', [
                ('lib', 'tl3_controller', 'src/lib.rs'),
                ('test', 'github', 'tests/github.rs'),
                ('test', 'nested', 'tests/nested/main.rs'),
                ('test', 'renamed', 'scenarios/renamed.rs'),
                ('custom-build', 'build-script-build', 'build.rs'),
            ], '[[test]]\nname = "github"\nharness = false\n'
               '[[test]]\nname = "renamed"\npath = "scenarios/renamed.rs"\nharness = false\n'),
        ]}, root)

    def test_cargo_metadata_covers_main_rs_layouts_custom_paths_and_harness(self):
        with tempfile.TemporaryDirectory() as root:
            self.assertEqual(self.cargo(Path(root)), {
                'tl3-controller/tl3_controller': ('crates/tl3-controller/src/lib.rs', 'libtest'),
                'tl3-controller/github-flow': ('crates/tl3-controller/tests/github.rs', 'custom'),
                'tl3-controller/github-notices': ('crates/tl3-controller/tests/github.rs', 'custom'),
                'tl3-controller/nested': ('crates/tl3-controller/tests/nested/main.rs', 'libtest'),
                'tl3-controller/renamed': ('crates/tl3-controller/scenarios/renamed.rs', 'custom'),
            })

    def test_bazel_query_yields_rust_binaries_and_python_modules(self):
        rust, python = bazel_parity.bazel_tests([
            rule('//crates/controller:test_test_nested', bazel_parity.RUST_RUNNER,
                 ['$(rootpath :b)', 'tl3-controller/nested', 'libtest', '-']),
            rule('//workers/tests:test_maker', bazel_parity.PYTHON_RUNNER, ['workers/tests/test_maker.py']),
            rule('//:gazelle_python_manifest.test', None, []),
        ])
        self.assertEqual(rust, {'tl3-controller/nested': 'libtest'})
        self.assertEqual(python, ['workers/tests/test_maker.py'])

    def test_manual_runner_targets_fail_because_the_gate_never_selects_them(self):
        with self.assertRaisesRegex(RuntimeError, r'manual[\s\S]*workers/tests/test_maker.py'):
            bazel_parity.bazel_tests([
                rule('//workers/tests:test_maker', bazel_parity.PYTHON_RUNNER,
                     ['workers/tests/test_maker.py'], tags=['manual']),
            ])

    def test_pytest_suffix_files_are_refused_unless_pytest_skips_them(self):
        # Light checks' pytest also collects *_test.py; Gazelle and parity only know test_*.py.
        files = ['harness/pyproject.toml', 'harness/tests/test_harness.py',
                 'harness/tests/chunk_test.py', 'harness/tests/fixtures/chunked_test.py']
        with self.assertRaisesRegex(RuntimeError, r'test_\*\.py[\s\S]*harness/tests/chunk_test\.py') as failure:
            bazel_parity.python_modules(files)
        self.assertNotIn('fixtures', str(failure.exception))
        self.assertEqual(bazel_parity.python_modules(files[:2] + files[3:]),
                         ['harness/tests/test_harness.py'])

    def test_missing_rust_file_is_named_with_the_fix(self):
        with tempfile.TemporaryDirectory() as root:
            cargo = self.cargo(Path(root))
            bazel = {binary: harness for binary, (_, harness) in cargo.items()}
            del bazel['tl3-controller/renamed']
            with self.assertRaises(RuntimeError) as failure:
                bazel_parity.compare_static(cargo, bazel, ['a/test_a.py'], ['a/test_a.py'])
            self.assertIn('crates/tl3-controller/scenarios/renamed.rs', str(failure.exception))
            self.assertIn('tools/bazel/generate.sh', str(failure.exception))

    def test_harness_mismatch_fails(self):
        with tempfile.TemporaryDirectory() as root:
            cargo = self.cargo(Path(root))
            bazel = {binary: 'libtest' for binary in cargo}
            with self.assertRaisesRegex(RuntimeError, 'harness'):
                bazel_parity.compare_static(cargo, bazel, ['a/test_a.py'], ['a/test_a.py'])

    def test_missing_and_extra_python_modules_fail(self):
        cargo = {'k/k': ('crates/k/src/lib.rs', 'libtest')}
        with self.assertRaisesRegex(RuntimeError, 'workers/tests/test_new.py'):
            bazel_parity.compare_static(cargo, {'k/k': 'libtest'}, ['workers/tests/test_new.py'], [])
        with self.assertRaisesRegex(RuntimeError, 'stale.py'):
            bazel_parity.compare_static(cargo, {'k/k': 'libtest'}, [], ['stale.py'])

    def test_empty_discovery_fails(self):
        with self.assertRaisesRegex(RuntimeError, 'empty'):
            bazel_parity.compare_static({}, {}, [], [])


if __name__ == '__main__':
    unittest.main()
