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


if __name__ == '__main__':
    unittest.main()
