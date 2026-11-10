"""Record the two permitted runtime facts before importing any served code."""
import json
import os
from pathlib import Path
import resource
import sys


PREFIX = 'TL3_RUNTIME_EVIDENCE '
# Generator executables resolve only from the image's system directories, never the checkout.
SYSTEM_PATH = '/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin'


def snapshot():
    with Path('/proc/self/status').open() as status:
        cpus = next(line.split(':', 1)[1].strip() for line in status
                    if line.startswith('Cpus_allowed_list:'))
    return {'Cpus_allowed_list': cpus, 'cgroup': Path('/proc/self/cgroup').read_text(),
            'RLIMIT_AS': resource.getrlimit(resource.RLIMIT_AS)}


if __name__ == '__main__':
    print(PREFIX + json.dumps(snapshot(), sort_keys=True), file=sys.stderr, flush=True)
    # Host validation must finish before the runner can import repository code.
    if os.read(0, 1) != b'\x01':
        raise SystemExit(125)
    if sys.argv[1:2] == ['--exec']:
        # A trusted generator argv runs only after the same host qualification gate.
        import shutil
        executable = shutil.which(sys.argv[3], path=SYSTEM_PATH) if sys.argv[2:3] == ['--'] and sys.argv[3:] else None
        if executable is None:
            raise SystemExit(127)
        os.execv(executable, sys.argv[3:])
    os.execv(sys.executable, [sys.executable, '-I', '-B', '/harness/runner.py', *sys.argv[1:]])
