"""Subprocess Python must work after callers intentionally strip its environment."""
import os
import subprocess
import sys
import unittest


class PythonRuntimeTests(unittest.TestCase):
    def test_isolated_child_keeps_declared_third_party_packages(self):
        result = subprocess.run([sys.executable, '-I', '-c', 'import pytest'],
                                env={'PATH': os.environ['PATH']}, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
