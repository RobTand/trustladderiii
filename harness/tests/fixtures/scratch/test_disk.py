import ctypes
import os
import struct


def test_disk_backed_scratch(tmp_path):
    buffer = ctypes.create_string_buffer(256)
    libc = ctypes.CDLL(None, use_errno=True)
    assert libc.statfs(os.fsencode(tmp_path), buffer) == 0
    filesystem = struct.unpack_from('l', buffer.raw)[0]
    assert filesystem not in (0x01021994, 0x858458F6), f'scratch must be disk-backed, got {filesystem:#x}'
    assert os.getuid() == 65534


def test_restrictive_session_directory(tmp_path_factory):
    directory = tmp_path_factory.mktemp('retained')
    directory.chmod(0o700)
    (directory / 'payload').write_text('UID 65534 session data survives until action cleanup')
