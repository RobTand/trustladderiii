"""Generation failures must name what is stale and the single repair command."""
from pathlib import Path
import sys
import tempfile
import unittest

import bazel_generated


class GeneratedInputsTests(unittest.TestCase):
    def test_stale_output_names_file_and_single_repair_command(self):
        with tempfile.TemporaryDirectory() as temporary:
            fake = Path(temporary, 'generator.py')
            fake.write_text('print("workers/tests/test_new.py missing from BUILD.bazel")\nraise SystemExit(1)\n')
            with self.assertRaises(RuntimeError) as failure:
                bazel_generated.check([[sys.executable, str(fake)]], Path(temporary))
            self.assertIn('workers/tests/test_new.py', str(failure.exception))
            self.assertIn('tools/bazel/generate.sh', str(failure.exception))

    def test_every_generator_is_checked_even_after_a_failure(self):
        failing = [sys.executable, '-c', 'print("first stale"); raise SystemExit(1)']
        also = [sys.executable, '-c', 'print("second stale"); raise SystemExit(1)']
        with self.assertRaises(RuntimeError) as failure:
            bazel_generated.check([failing, also], Path.cwd())
        self.assertIn('first stale', str(failure.exception))
        self.assertIn('second stale', str(failure.exception))

    def test_requirements_crates_and_gazelle_are_all_checked(self):
        commands = [' '.join(map(str, command)) for command in bazel_generated.CHECKS]
        self.assertTrue(any('requirements.py --check' in command for command in commands))
        self.assertTrue(any('rust_builds.py --check' in command for command in commands))
        self.assertTrue(any('//:gazelle' in command and '-mode=diff' in command for command in commands))

    def test_success_is_accepted(self):
        bazel_generated.check([[sys.executable, '-c', 'pass']], Path.cwd())


if __name__ == '__main__':
    unittest.main()
