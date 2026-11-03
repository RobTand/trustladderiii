"""The admitted action owns disk scratch and its cleanup, including launcher loss."""
import json
import os
from pathlib import Path
import stat
import sys
import tempfile


SCRATCH_BYTES = 512 * 1024 * 1024


def environment():
    return {
        'TL3_SCRATCH_ROOT': '/home/rob/tmp',
        'TL3_SCRATCH_MAX_BYTES': str(SCRATCH_BYTES),
        'PRISMABUILD_LOCAL_SCRATCH_PAIRS': 'TL3_SCRATCH_ROOT:TL3_SCRATCH_MAX_BYTES',
        'PRISMABUILD_EPHEMERAL_SCRATCH_DECLARATIONS': json.dumps({
            'schema': 'prismabuild.scratch_lifetime_selection.v1',
            'entries': [{'root_env': 'TL3_SCRATCH_ROOT', 'name': 'pytest', 'lifetime': 'ephemeral'}],
        }, sort_keys=True, separators=(',', ':')),
    }


def registered_leaf():
    # The admitted progress helper belongs to the action's pinned PB generation.
    runtime = Path(os.environ['PRISMABUILD_ACTION_PROGRESS_HELPER']).resolve().parents[1]
    sys.path.insert(0, str(runtime / 'src'))
    # gazelle:ignore prismabuild
    # gazelle:ignore prismabuild.client
    from prismabuild import client

    queue = client.PoolQueue(os.environ['PRISMABUILD_QUEUE_ROOT'])
    record = client.read_claimed_record(queue, os.environ['PRISMABUILD_ACTION_KEY'])
    registration = record[client.SCRATCH_LIFETIME_FIELD]
    declaration = client.bind_ephemeral_scratch(queue, root_env='TL3_SCRATCH_ROOT', name='pytest',
                                               claim_snapshot=record)
    entries = [entry for entry in registration['entries'] if entry['declaration'] == declaration]
    if not registration['registration_complete'] or len(entries) != 1 or entries[0]['cleaned']:
        raise RuntimeError('pytest scratch requires the admitted action\'s registered ephemeral leaf')
    leaf = client.ephemeral_scratch_path(declaration)
    info = leaf.lstat()
    identity = entries[0]['identity'][-1]
    if (not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid()
            or info.st_mode & 0o777 != 0o700 or (info.st_dev, info.st_ino) != (identity['dev'], identity['ino'])):
        raise RuntimeError('pytest scratch ownership changed after PB registration')
    return leaf


def directory():
    leaf = registered_leaf()
    # Keep the registered leaf private. Only this fresh child enters the container.
    return Path(tempfile.mkdtemp(prefix='pytest-', dir=leaf))
