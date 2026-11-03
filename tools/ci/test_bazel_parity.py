"""The Bazel migration must preserve the gate's discovered tests."""
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

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



class ShardedReceiptTests(unittest.TestCase):
    def receipts(self, root, parts):
        for index, names in enumerate(parts):
            outputs = root / f'shard_{index}/test.outputs'
            outputs.mkdir(parents=True)
            (outputs / 'inventory.json').write_text(json.dumps({
                'rust': {'controller/workers': names},
                'shard': {'index': index, 'total': 2}}))

    def test_disjoint_shards_merge_into_one_binary_inventory(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.receipts(root, [['first'], ['second']])
            try:
                found = bazel_parity.bazel_receipts(root)['rust']
            except RuntimeError as error:
                self.fail(f'disjoint complete shards must be accepted: {error}')
            self.assertEqual(found, {'controller/workers': ['first', 'second']})

    def test_missing_shard_fails(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.receipts(root, [['first']])
            with self.assertRaisesRegex(RuntimeError, 'missing.*shard'):
                bazel_parity.bazel_receipts(root)

    def test_duplicate_scenario_across_shards_fails(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.receipts(root, [['same'], ['same']])
            with self.assertRaisesRegex(RuntimeError, 'duplicate.*scenario'):
                bazel_parity.bazel_receipts(root)


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


class RustRunnerReceiptTests(unittest.TestCase):
    def run_sharded(self, binary):
        sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'bazel'))
        import rust_runner
        with tempfile.TemporaryDirectory() as temporary:
            environment = {'TEST_TOTAL_SHARDS': '2', 'TEST_SHARD_INDEX': '1',
                           'TEST_SHARD_STATUS_FILE': str(Path(temporary, 'status')),
                           'TEST_UNDECLARED_OUTPUTS_DIR': temporary, 'TL3_BUDGETS': 'record'}
            with patch.dict(os.environ, environment), \
                 patch.object(sys, 'argv', ['runner', 'fixture', binary, 'custom', '-']), \
                 patch.object(rust_runner, 'prepare'), \
                 patch.object(rust_runner, 'link_workspace'), \
                 patch.object(rust_runner, 'check_workspace'), \
                 patch.object(rust_runner.rust_gate, 'execute_binary',
                              return_value={binary + '::one': 0.25}) as execute:
                rust_runner.main()
            self.assertTrue(Path(temporary, 'status').exists())
            receipt = json.loads(Path(temporary, 'inventory.json').read_text())
            self.assertEqual(receipt['shard'], {'index': 1, 'total': 2})
            self.assertEqual(receipt['rust'], {binary: ['one']})
            execute.assert_called_once()

    def test_long_controller_scenarios_accept_sharding(self):
        for name in ['mergify', 'publishing', 'github-flow', 'github-notices', 'simulation']:
            with self.subTest(binary=name):
                self.run_sharded('tl3-controller/' + name)

    def test_non_allowlisted_binary_refuses_sharding(self):
        with self.assertRaisesRegex(RuntimeError, 'shard-aware custom harness'):
            self.run_sharded('tl3-controller/unlisted')

    def test_shard_receipt_does_not_claim_scalar_action_resource_metrics(self):
        sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'bazel'))
        import rust_runner
        with tempfile.TemporaryDirectory() as temporary:
            environment = {'TEST_TOTAL_SHARDS': '2', 'TEST_SHARD_INDEX': '1',
                           'TEST_SHARD_STATUS_FILE': str(Path(temporary, 'status')),
                           'TEST_UNDECLARED_OUTPUTS_DIR': temporary, 'TL3_BUDGETS': 'record'}
            with patch.dict(os.environ, environment), \
                 patch.object(sys, 'argv', ['runner', 'fixture', 'tl3-controller/workers', 'custom', '-']), \
                 patch.object(rust_runner, 'prepare'), \
                 patch.object(rust_runner, 'link_workspace'), \
                 patch.object(rust_runner, 'check_workspace'), \
                 patch.object(rust_runner.rust_gate, 'execute_binary',
                              return_value={'tl3-controller/workers::one': 0.25}):
                rust_runner.main()
            receipt = json.loads(Path(temporary, 'inventory.json').read_text())
            self.assertEqual(receipt['shard'], {'index': 1, 'total': 2})
            self.assertEqual(receipt['rust'], {'tl3-controller/workers': ['one']})
            self.assertIsInstance(receipt.get('cpu_seconds', {}), dict)
            self.assertNotIn('wall_seconds', receipt)


class NestedSuiteSizingTests(unittest.TestCase):
    def test_detects_runners_in_any_package_and_embedded_scripts(self):
        sources = [
            'subprocess.run([sys.executable, "-m", "pytest", "tests"])',
            'subprocess.Popen(["python", "-m", "unittest"])',
            'RUNNER = Path("src/tl3_harness/runner.py")\nsubprocess.run([sys.executable, str(RUNNER)])',
            'execution.run_tree(root, image, tests, report, progress)',
            'run_checks(root, base, head, checks)',
            'script.write_text(script.read_text().replace("marker", '
            + repr('import subprocess\nsubprocess.run([sys.executable, str(root / "trusted/tl3_harness/runner.py")])\nprint(json.dumps({"schema":') + '))',
        ]
        for source in sources:
            with self.subTest(source=source):
                self.assertTrue(bazel_parity.has_nested_suite(source))
        for source in ['import pytest\npytest.raises(ValueError)',
                       'subprocess.run(["git", "status"])',
                       'SCRIPT = "from unittest.mock import patch"\nsubprocess.run(["python", "-c", SCRIPT])',
                       'description = "subprocess.run([python, pytest])"']:
            self.assertFalse(bazel_parity.has_nested_suite(source))

    def test_parity_requires_all_positive_properties_outside_harness(self):
        source = 'subprocess.run(["python", "-m", "pytest"])'
        properties = {'EstimatedComputeUnits': '1', 'EstimatedCPU': '2', 'EstimatedMemory': '0.5GB'}
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            module = root / 'future/tests/test_nested.py'
            module.parent.mkdir(parents=True)
            module.write_text(source)

            def query(values):
                item = json.loads(rule('//future/tests:test_nested', bazel_parity.PYTHON_RUNNER,
                                       ['future/tests/test_nested.py']))
                item['rule']['attribute'].append({
                    'name': 'exec_properties',
                    'stringDictValue': [{'key': key, 'value': value} for key, value in values.items()]})
                return [json.dumps(item)]

            for key in properties:
                for value in [None, '0']:
                    values = dict(properties)
                    if value is None:
                        del values[key]
                    else:
                        values[key] = value
                    with self.subTest(key=key, value=value):
                        with self.assertRaisesRegex(RuntimeError, 'future/tests:test_nested.*' + key):
                            bazel_parity.check_python_sizing(query(values), root)
            bazel_parity.check_python_sizing(query(properties), root)
            module.write_text('def test_plain(): assert True')
            bazel_parity.check_python_sizing(query({}), root)


if __name__ == '__main__':
    unittest.main()
