"""State-table Command row: required Bazel coverage of checked command IDs."""
from pathlib import Path
import runpy
import unittest


class CommandIdPolicyTests(unittest.TestCase):
    def test_controller_source_obeys_command_policy(self):
        runpy.run_path(str(Path(__file__).resolve().parents[2] / 'tools/check_command_ids.py'),
                       run_name='__main__')
