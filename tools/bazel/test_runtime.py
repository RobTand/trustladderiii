"""Subprocess Python must keep declared dependencies after callers strip its environment."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent

# Runs in a child: optionally prepare(), then start an isolated grandchild with a
# bare environment, as worker code does, and report whether it sees the module.
CHILD = '''
import os, subprocess, sys
sys.path.insert(0, {declared!r})
sys.path.insert(0, {here!r})
if {prepare}:
    import runtime
    runtime.prepare()
result = subprocess.run([sys.executable, '-I', '-c', 'import declared_probe'],
                        env={{'PATH': os.environ['PATH']}}, capture_output=True, text=True)
sys.exit(result.returncode)
'''


class PythonRuntimeTests(unittest.TestCase):
    def run_child(self, prepare):
        with tempfile.TemporaryDirectory() as temporary:
            declared = Path(temporary, 'declared')
            declared.mkdir()
            (declared / 'declared_probe.py').write_text('VALUE = 7\n')
            scratch = Path(temporary, 'scratch')
            scratch.mkdir()
            code = CHILD.format(declared=str(declared), here=str(HERE), prepare=prepare)
            env = {'PATH': os.environ['PATH'], 'TEST_TMPDIR': str(scratch)}
            return subprocess.run([sys.executable, '-c', code], env=env, cwd=temporary).returncode

    def test_isolated_child_keeps_declared_packages(self):
        self.assertEqual(self.run_child(prepare=True), 0)

    def test_without_prepare_the_isolated_child_loses_them(self):
        self.assertNotEqual(self.run_child(prepare=False), 0)


class PythonRunnerTests(unittest.TestCase):
    def run_module(self, source):
        sys.path.insert(0, str(HERE))
        import python_runner
        with tempfile.TemporaryDirectory() as temporary:
            Path(temporary, 'test_probe_module.py').write_text(source)
            sys.path.insert(0, temporary)
            try:
                return python_runner.run_unittest('test_probe_module')
            finally:
                sys.path.remove(temporary)
                sys.modules.pop('test_probe_module', None)

    def test_a_module_without_tests_fails_like_pytest_exit_5(self):
        self.assertEqual(self.run_module('VALUE = 1\n'), 5)

    def test_a_passing_module_succeeds(self):
        source = 'import unittest\nclass T(unittest.TestCase):\n    def test_ok(self):\n        pass\n'
        self.assertEqual(self.run_module(source), 0)


PROBE = '''
import os
import pytest

@pytest.mark.parametrize("case", range(7))
def test_case(case):
    with open(os.environ["PROBE_LOG"], "a") as log:
        log.write(f"case{case}\\n")

def test_last():
    with open(os.environ["PROBE_LOG"], "a") as log:
        log.write("last\\n")
'''


class PytestShardTests(unittest.TestCase):
    """Bazel shards of one pytest module run each collected test exactly once."""

    def run_shards(self, total):
        sys.path.insert(0, str(HERE))
        import pytest
        import python_runner
        ran = []
        with tempfile.TemporaryDirectory() as temporary:
            module = Path(temporary, 'test_probe_shards.py')
            module.write_text(PROBE)
            for index in range(total):
                log = Path(temporary, f'shard{index}.log')
                log.touch()
                os.environ['PROBE_LOG'] = str(log)
                try:
                    status = pytest.main([str(module), '-q', '-p', 'no:cacheprovider'],
                                         plugins=[python_runner.Shard(index, total)])
                finally:
                    del os.environ['PROBE_LOG']
                    sys.modules.pop('test_probe_shards', None)
                self.assertEqual(status, 0)
                ran.append(log.read_text().split())
        return ran

    def test_shards_partition_the_module(self):
        for total in (2, 3, 8):
            ran = self.run_shards(total)
            names = [name for shard in ran for name in shard]
            self.assertEqual(sorted(names), sorted([f'case{case}' for case in range(7)] + ['last']))
            self.assertTrue(all(ran), f'every one of {total} shards runs a test')
            self.assertLessEqual(max(map(len, ran)) - min(map(len, ran)), 1, 'shards stay even')

    def test_partition_ignores_collection_order(self):
        """Each shard is its own process, so its collection order may differ (set or
        hash-seeded parametrize). A test must still land in exactly one shard."""
        sys.path.insert(0, str(HERE))
        import python_runner

        class Item:
            def __init__(self, nodeid):
                self.nodeid = nodeid

        class Hook:
            def pytest_deselected(self, items):
                pass

        class Config:
            hook = Hook()

        ids = [f'test_probe.py::test_case[{case}]' for case in range(8)]
        for total in (2, 3, 5):
            kept = []
            for index in range(total):
                order = ids if index % 2 == 0 else list(reversed(ids))
                items = [Item(nodeid) for nodeid in order]
                python_runner.Shard(index, total).pytest_collection_modifyitems(Config(), items)
                kept.extend(item.nodeid for item in items)
            self.assertEqual(sorted(kept), sorted(ids), f'{total} shards over a reordered collection')

    def test_runner_accepts_bazel_sharding(self):
        sys.path.insert(0, str(HERE))
        import python_runner
        with tempfile.TemporaryDirectory() as temporary:
            status = Path(temporary, 'shard-status')
            shard_env = {'TEST_TOTAL_SHARDS': '3', 'TEST_SHARD_INDEX': '2',
                         'TEST_SHARD_STATUS_FILE': str(status)}
            saved = {key: os.environ.get(key) for key in shard_env}
            os.environ.update(shard_env)
            try:
                self.assertEqual(python_runner.bazel_shard(), (2, 3))
            finally:
                for key, value in saved.items():
                    if value is None:
                        os.environ.pop(key, None)
                    else:
                        os.environ[key] = value
            self.assertTrue(status.exists(), 'Bazel needs the status file to trust the shard')


if __name__ == '__main__':
    unittest.main()
