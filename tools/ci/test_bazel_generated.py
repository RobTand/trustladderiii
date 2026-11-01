"""Generation failures must explain which test was omitted and how to repair it."""
from pathlib import Path
import sys
import tempfile
import unittest

import bazel_generated


class GazelleFreshnessTests(unittest.TestCase):
    def test_stale_output_names_file_and_single_repair_command(self):
        with tempfile.TemporaryDirectory() as temporary:
            fake = Path(temporary, 'generator.py')
            fake.write_text('print("workers/tests/test_new.py missing from BUILD.bazel")\nraise SystemExit(1)\n')
            with self.assertRaises(RuntimeError) as failure:
                bazel_generated.check_gazelle([sys.executable, str(fake)], Path(temporary))
            self.assertIn('workers/tests/test_new.py', str(failure.exception))
            self.assertIn('bazel run //:gazelle', str(failure.exception))

    def test_success_is_accepted(self):
        bazel_generated.check_gazelle([sys.executable, '-c', 'pass'], Path.cwd())


if __name__ == '__main__':
    unittest.main()
