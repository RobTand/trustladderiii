"""The packaged verifier must find its specimen without a neighboring checkout."""
import os
from pathlib import Path
import runpy
import shutil
import tempfile
import unittest
from unittest.mock import patch


class VerifierRunfilesTests(unittest.TestCase):
    def test_declared_runfiles_supply_the_disk_specimen(self):
        for strategy in ('directory', 'manifest'):
            with self.subTest(strategy=strategy):
                self.verify_packaged_specimen(strategy)

    def verify_packaged_specimen(self, strategy):
        runfiles = Path(os.environ['TEST_SRCDIR'])
        with tempfile.TemporaryDirectory() as temporary:
            isolated = Path(temporary)
            shutil.copytree(runfiles / '_main/harness', isolated / '_main/harness', symlinks=False)
            namespace = runpy.run_path(str(isolated / '_main/harness/tools/verify_disk_scratch.py'))
            verify = namespace['verify']

            class SpecimenCopied(Exception):
                pass

            def first_git_command(repo, *args):
                self.assertIn('libc.statfs', (repo / 'test_disk.py').read_text())
                raise SpecimenCopied

            environment = dict(os.environ, RUNFILES_DIR=str(isolated))
            environment.pop('RUNFILES_MANIFEST_FILE', None)
            if strategy == 'manifest':
                fixture = isolated / '_main/harness/tests/fixtures/scratch/test_disk.py'
                relocated = isolated / 'specimen.py'
                fixture.rename(relocated)
                manifest = isolated / 'MANIFEST'
                manifest.write_text(f'_main/harness/tests/fixtures/scratch/test_disk.py {relocated}\n')
                environment.pop('RUNFILES_DIR')
                environment['RUNFILES_MANIFEST_FILE'] = str(manifest)
            with patch.dict(os.environ, environment, clear=True), patch.dict(verify.__globals__, {
                'require_admission': lambda: None,
                'git': first_git_command,
            }):
                with self.assertRaises(SpecimenCopied):
                    verify('sha256:' + '1' * 64)


if __name__ == '__main__':
    unittest.main()
