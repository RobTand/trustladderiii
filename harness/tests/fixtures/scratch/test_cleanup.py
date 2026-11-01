import os
from pathlib import Path
import subprocess
import sys

import pytest


@pytest.mark.parametrize('fails', [False, True])
def test_writer_recreates_a_file_during_cleanup(fails):
    scratch = Path(os.environ['TMPDIR'])
    (scratch / 'payload').write_text('original')
    writer = subprocess.Popen(
        [sys.executable, '-c',
         'import sys; from pathlib import Path; sys.stdin.read(); '
         'Path(sys.argv[1]).write_text("late payload"); print("created")',
         str(scratch / 'late-payload')],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True,
    )
    original = os.rmdir

    def recreate_before_removal(path, *args, **kwargs):
        if str(path) == str(scratch):
            os.rmdir = original
            # The real writer fills the directory after rmtree emptied it,
            # so the real rmdir refuses it without a scheduling race.
            output, _ = writer.communicate('', timeout=10)
            assert writer.returncode == 0 and output.strip() == 'created'
        return original(path, *args, **kwargs)

    os.rmdir = recreate_before_removal
    assert not fails, 'the test failed independently of cleanup'


def test_tmp_path_keeps_pytest_name_and_number(tmp_path, tmp_path_factory):
    assert tmp_path.parent == tmp_path_factory.getbasetemp()
    assert tmp_path.name.startswith('test_tmp_path_keeps_pytest_')
    assert tmp_path.name[-1].isdigit()
