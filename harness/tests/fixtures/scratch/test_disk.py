import ctypes
import os
import struct

import pytest


def test_disk_backed_scratch(tmp_path):
    buffer = ctypes.create_string_buffer(256)
    libc = ctypes.CDLL(None, use_errno=True)
    assert libc.statfs(os.fsencode(tmp_path), buffer) == 0
    filesystem = struct.unpack_from('l', buffer.raw)[0]
    assert filesystem not in (0x01021994, 0x858458F6), f'scratch must be disk-backed, got {filesystem:#x}'
    assert os.getuid() == os.stat('/tmp').st_uid


def test_restrictive_session_directory(tmp_path_factory):
    directory = tmp_path_factory.mktemp('retained')
    directory.chmod(0o700)
    (directory / 'payload').write_text('leaf-owner session data survives until action cleanup')


def test_setup_failure(retained_setup_failure):
    pass


def test_cancellation(tmp_path_factory):
    import time
    from pathlib import Path

    directory = tmp_path_factory.mktemp('cancel-retained')
    directory.chmod(0o700)
    (directory / 'payload').write_text('retained until PB cancellation cleanup')
    Path('/tmp/cancel-ready').write_text(str(directory))
    while True:
        time.sleep(1)


@pytest.fixture
def retained_setup_failure(tmp_path_factory):
    directory = tmp_path_factory.mktemp('setup-retained')
    directory.chmod(0o700)
    (directory / 'payload').write_text('retained after setup failure')
    raise RuntimeError('intentional scratch setup failure')
