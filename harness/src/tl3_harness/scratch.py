"""The admitted action owns disk scratch and its cleanup, including launcher loss."""
import os
from pathlib import Path
import stat
import tempfile


SCRATCH_BYTES = 512 * 1024 * 1024


def environment():
    return {
        'TL3_SCRATCH_ROOT': '/home/rob/tmp',
        'TL3_SCRATCH_MAX_BYTES': str(SCRATCH_BYTES),
    }


def directory():
    from .runtime import temporary_root

    # Flux is the only executor (PB-REMOVAL): the launch context owns the job scratch leaf.
    return private_directory(temporary_root())


_owned = {}


def private_directory(root, *, prefix='pytest-'):
    """Create a leaf in the Rust-validated job root, retaining its filesystem identity."""
    root = Path(root)
    info = root.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o777 != 0o700:
        raise RuntimeError('job scratch must remain an owned private directory')
    child = Path(tempfile.mkdtemp(prefix=prefix, dir=root))
    info = child.lstat()
    _owned[child] = (info.st_dev, info.st_ino)
    return child


def release_directory(child):
    """Only supervision with confirmed execution stop may release a container leaf."""
    import shutil

    child = Path(child)
    identity = _owned.get(child)
    if identity is None:
        raise RuntimeError('refusing to remove an unowned scratch directory')
    info = child.lstat()
    if not stat.S_ISDIR(info.st_mode) or (info.st_dev, info.st_ino) != identity:
        raise RuntimeError('scratch identity changed before cleanup')
    shutil.rmtree(child)
    del _owned[child]
