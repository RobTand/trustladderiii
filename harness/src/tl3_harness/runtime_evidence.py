"""Record the two permitted runtime facts before importing any served code."""
import json
import os
from pathlib import Path
import resource
import sys


PREFIX = 'TL3_RUNTIME_EVIDENCE '
# Generator executables resolve only from the image's system directories, never the checkout.
SYSTEM_PATH = '/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin'


class TargetEnvironmentError(ValueError):
    def __init__(self, reason):
        self.reason = reason
        super().__init__(reason + '; value withheld')


def safe_option_path(value):
    import re

    text = str(value)
    if (not re.fullmatch(r'/[A-Za-z0-9_./-]+', text)
            or any(part in ('.', '..') for part in text.split('/'))):
        raise TargetEnvironmentError('invalid_mount_path')
    return text


def validate_flux_uri(value):
    if not isinstance(value, str) or not value.startswith('local:///'):
        raise TargetEnvironmentError('invalid_flux_uri')
    try:
        path = safe_option_path(value[len('local://'):])
        if path.endswith('/') or '//' in path or not Path(path).name:
            raise TargetEnvironmentError('invalid_flux_uri')
    except TargetEnvironmentError:
        raise TargetEnvironmentError('invalid_flux_uri') from None
    return value


def target_environment(environment, pass_flux_uri=False):
    result = {name: value for name, value in environment.items() if not name.startswith('FLUX_')}
    if pass_flux_uri:
        result['FLUX_URI'] = validate_flux_uri(environment.get('FLUX_URI'))
    return result


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
    pass_flux_uri = sys.argv[1:2] == ['--pass-flux-uri']
    if pass_flux_uri:
        del sys.argv[1]
    environment = target_environment(os.environ, pass_flux_uri)
    if sys.argv[1:2] == ['--exec']:
        # A trusted generator argv runs only after the same host qualification gate.
        import shutil
        executable = shutil.which(sys.argv[3], path=SYSTEM_PATH) if sys.argv[2:3] == ['--'] and sys.argv[3:] else None
        if executable is None:
            raise SystemExit(127)
        os.execve(executable, sys.argv[3:], environment)
    os.execve(sys.executable, [sys.executable, '-I', '-B', '/harness/runner.py', *sys.argv[1:]], environment)
