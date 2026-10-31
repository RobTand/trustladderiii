import json
from pathlib import Path
import tempfile
import sys
import unittest
from unittest.mock import patch

import check
import hosted
import rust_gate
import shards


class HostedTests(unittest.TestCase):
    def test_partition_is_complete_disjoint_stable_and_weighted(self):
        names = ['worker', 'github', 'short-a', 'short-b', 'new']
        weights = {'worker': 90, 'github': 70, 'short-a': 10, 'short-b': 10}
        groups = shards.partition(names, 3, weights)
        self.assertEqual(sorted(n for group in groups for n in group), sorted(names))
        self.assertEqual(groups, shards.partition(list(reversed(names)), 3, weights))
        self.assertNotEqual(next(i for i, group in enumerate(groups) if 'worker' in group),
                            next(i for i, group in enumerate(groups) if 'github' in group))
        for invalid in [0, -1]:
            with self.assertRaises(ValueError):
                shards.partition(names, invalid, weights)
        with self.assertRaisesRegex(ValueError, 'duplicate'):
            shards.partition(['a', 'a'], 2, {})

    def test_shard_coordinates_cannot_drop_the_suite(self):
        self.assertEqual(shards.parse_shard('0/8'), (0, 8))
        self.assertEqual(shards.parse_shard('7/8'), (7, 8))
        for value in ['8/8', '-1/8', '0/0', '1', 'a/b']:
            with self.subTest(value=value), self.assertRaises(ValueError):
                shards.parse_shard(value)

    def test_plan_uses_both_architectures_and_respects_one_total_setting(self):
        for total in [8, 10, 12]:
            result = hosted.plan(total)
            self.assertEqual(sum(len(matrix['include']) for matrix in result.values()), total)
            for matrix in result.values():
                entries = matrix['include']
                self.assertEqual({row['runner'] for row in entries},
                                 {'ubuntu-24.04', 'ubuntu-24.04-arm'})
                self.assertEqual({row['index'] for row in entries}, set(range(len(entries))))
                self.assertTrue(all(row['total'] == len(entries) for row in entries))
        for total in [0, 3, 19]:
            with self.assertRaises(ValueError):
                hosted.plan(total)

    def test_aggregate_rejects_failed_missing_cancelled_and_skipped_dependencies(self):
        hosted.aggregate({'shards': {'result': 'success'}, 'lint': {'result': 'success'}})
        for result in ['failure', 'cancelled', 'skipped', None]:
            with self.subTest(result=result), self.assertRaises(RuntimeError):
                hosted.aggregate({'shards': {'result': result}})
        with self.assertRaises(RuntimeError):
            hosted.aggregate({})

    def test_sharded_rust_still_enforces_budget_and_rejects_skips(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            fixture = root / 'native.py'
            original = ('print(\'{"type":"test","event":"ok","name":"one","exec_time":2}\')\n'
                        'print(\'{"type":"suite","event":"ok","passed":1,"failed":0,"ignored":0}\')\n')
            fixture.write_text(original)
            binaries = [{'name': name, 'custom': False, 'command': [sys.executable, str(fixture)]}
                        for name in ['first', 'second']]
            policy = root / rust_gate.POLICY
            policy.parent.mkdir(parents=True)
            policy.write_text(json.dumps({'seconds': 1, 'slow': {}}))
            with patch.object(rust_gate, 'ROOT', root), \
                 patch.object(rust_gate, 'baseline_policy', return_value={'seconds': 1, 'slow': {}}), \
                 patch.object(rust_gate, 'collect_binaries', return_value=binaries):
                with self.assertRaisesRegex(RuntimeError, 'budget exceeded'):
                    rust_gate.test_workspace(Path('Cargo.toml'), shard=(0, 2))
                fixture.write_text(original.replace('"exec_time":2', '"exec_time":0.25'))
                reports = [rust_gate.test_workspace(Path('Cargo.toml'), shard=(index, 2))
                           for index in range(2)]
                self.assertEqual([set(report['tests']) for report in reports],
                                 [{'first::one'}, {'second::one'}])
                fixture.write_text(
                    'print(\'{"type":"test","event":"ignored","name":"one"}\')\n'
                    'print(\'{"type":"suite","event":"ok","passed":0,"ignored":1}\')\n')
                with self.assertRaisesRegex(RuntimeError, 'skipped'):
                    rust_gate.test_workspace(Path('Cargo.toml'), shard=(0, 2))

    def test_python_shards_keep_every_project_and_standalone_suite(self):
        files = [Path(n) for n in ['workers/pyproject.toml', 'workers/tests/test_worker.py',
                                   'harness/pyproject.toml', 'harness/tests/test_harness.py',
                                   'tools/test_tool.py', 'deploy/test_deploy.py']]
        projects = {Path('workers'): {'project': {}}, Path('harness'): {'project': {}}}
        with patch.object(check, 'python_projects', return_value=projects):
            expected = {'project:workers', 'project:harness', 'unittest:tools', 'unittest:deploy'}
            groups = hosted.python_partition(files, 2)
            self.assertEqual(set().union(*map(set, groups)), expected)
            self.assertEqual(sum(map(len, groups)), len(expected))
            calls = []
            with patch.object(check, 'run', side_effect=lambda *args, **kwargs: calls.append((args, kwargs))):
                for index in range(2):
                    check.python(files, selected=set(groups[index]), lint=index == 0)
            self.assertEqual(sum('ruff' in args for args, _ in calls), 1)
            self.assertEqual(sum('unittest' in args for args, _ in calls), 2)
            self.assertEqual(sum('pytest' in args for args, _ in calls), 2)


if __name__ == '__main__':
    unittest.main()
