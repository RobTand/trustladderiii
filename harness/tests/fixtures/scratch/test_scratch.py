import os
from pathlib import Path
import subprocess
import sys
import tempfile

import pytest


@pytest.fixture(scope='module')
def shared(tmp_path_factory):
    path = tmp_path_factory.mktemp('module') / 'shared'
    path.write_text('module fixture stays alive')
    return path


@pytest.mark.parametrize('value', range(4))
def test_scratch(tmp_path, shared, value):
    assert shared.read_text() == 'module fixture stays alive'
    (tmp_path / 'payload').write_bytes(b'x' * (80 * 1024 * 1024))
    with tempfile.NamedTemporaryFile(prefix='payload-', delete=False) as stream:
        stream.write(b'x' * (40 * 1024 * 1024))
    subprocess.run([sys.executable, '-c',
                    'import os; from pathlib import Path; Path(os.environ["TMPDIR"], "payload-child").write_text("child")'], check=True)
    assert Path(os.environ['TMPDIR']).is_dir()
    assert value >= 0
