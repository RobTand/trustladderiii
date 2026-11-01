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


if __name__ == '__main__':
    unittest.main()
