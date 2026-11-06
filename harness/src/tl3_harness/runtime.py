"""Host-side runtime I/O for the pytest edge; Rust owns the launch contract."""
import json
import os
import tempfile
import uuid
from contextlib import contextmanager
from pathlib import Path

from .model import NATIVE_THREAD_ENVIRONMENT


def launch_context():
    # FX1 supplies the generated contract and validated consumer operation.
    # Refuse Flux until that interface is connected, never fall through to PB.
    if os.environ.get('FLUX_JOB_ID'):
        raise RuntimeError('Flux requires the Rust-validated launch context')
    return None


def podman_command(tree, image, scratch, threads, *, executable='podman', harness=None, interpreter='python3'):
    harness = Path(harness) if harness is not None else Path(__file__).resolve().parent
    return [
        str(executable), 'run', '--cgroups=disabled', '--network=none', '--read-only', '--read-only-tmpfs=false', '--init',
        '--userns=keep-id', '--interactive', '--rm', '--pull=never', '--cap-drop=ALL',
        '--security-opt=no-new-privileges', '--pids-limit=128', '--ipc=private', '--workdir=/checkout',
        '--mount', f'type=bind,src={tree},dst=/checkout,readonly',
        '--mount', f'type=bind,src={harness},dst=/harness,readonly',
        '--mount', f'type=bind,src={scratch},dst=/tmp',
        '--env', 'HOME=/tmp', '--env', 'TMPDIR=/tmp', '--env', 'PYTEST_DISABLE_PLUGIN_AUTOLOAD=1',
        *[arg for name in NATIVE_THREAD_ENVIRONMENT for arg in ('--env', f'{name}={threads}')],
        f'--entrypoint={interpreter}', image, '-I', '-B', '/harness/runtime_evidence.py', '-',
    ]


def evidence_sink(container):
    context = launch_context()
    if context is None:
        return None
    return Evidence(context['output_dir'], container)


class Evidence:
    """Keep runtime observations separate from the runner's stdout journal."""

    def __init__(self, output, container):
        self.path = Path(output) / f'{container}.runtime.jsonl'
        self.container_seen = False
        self.container = container
        self.record('host', self.host_snapshot())

    @staticmethod
    def host_snapshot():
        from .runtime_evidence import snapshot
        return snapshot()

    def record(self, event, facts):
        mode = 'x' if event == 'host' else 'a'
        with self.path.open(mode) as target:
            target.write(json.dumps({'event': event, **facts}, sort_keys=True) + '\n')
            target.flush()
            os.fsync(target.fileno())

    def observe(self, line):
        from .runtime_evidence import PREFIX

        if not line.startswith(PREFIX.encode()):
            return False
        if self.container_seen:
            raise ValueError('duplicate runtime evidence')
        facts = json.loads(line[len(PREFIX):])
        if (not isinstance(facts, dict) or not {'Cpus_allowed_list', 'cgroup'} <= set(facts)
                or not all(isinstance(facts[key], str) for key in ('Cpus_allowed_list', 'cgroup'))):
            raise ValueError('invalid runtime evidence')
        self.record('container', facts)
        self.container_seen = True
        return True


def validate_runtime_binding(evidence):
    raise RuntimeError('Flux runtime binding requires the generated Rust validation operation')


_uncertain_containers = set()


def cache_owner():
    context = launch_context()
    if context is not None:
        run = context['run']
        return run['backend'], run['instance'], run['job']
    return ('prismabuild', os.environ.get('PRISMABUILD_QUEUE_ROOT', ''),
            os.environ['PRISMABUILD_CONTAINER_OWNER'])


def temporary_root():
    context = launch_context()
    return Path(context['scratch']['leaf']) if context is not None else None


@contextmanager
def workspace(prefix):
    root = temporary_root()
    if root is None:
        with tempfile.TemporaryDirectory(prefix=prefix) as directory:
            yield directory
        return
    from .scratch import private_directory, release_directory
    directory = private_directory(root, prefix=prefix)
    try:
        yield directory
    finally:
        if not _uncertain_containers:
            release_directory(directory)


def journal_path(path):
    context = launch_context()
    if context is None:
        return path
    return Path(context['output_dir']) / f'{path.stem}-{uuid.uuid4().hex}.jsonl'


def command(tree, scratch):
    context = launch_context()
    configured = context['runtime']
    return podman_command(tree, configured['image'], scratch, context['native_threads'],
                          executable=configured['executable'], harness=configured['trusted_root'],
                          interpreter=configured['interpreter'])


def prepare_scratch():
    root = temporary_root()
    root.mkdir(mode=0o700, exist_ok=True)
    # private_directory checks current ownership/mode before making any working leaf.


def no_progress_seconds():
    context = launch_context()
    if context is not None:
        return context['progress']['no_progress_seconds']
    return json.loads(os.environ.get('PRISMABUILD_ACTION_PROGRESS_ALLOWANCES', '{}')).get('tests', 120)


def container_pending(name):
    _uncertain_containers.add(name)


def container_stopped(name, command):
    from .scratch import _owned, release_directory

    _uncertain_containers.discard(name)
    for index, argument in enumerate(command[:-1]):
        if argument == '--mount':
            mount = command[index + 1]
            if mount.startswith('type=bind,src=') and mount.endswith(',dst=/tmp'):
                path = Path(mount[len('type=bind,src='):-len(',dst=/tmp')])
                if path in _owned:
                    release_directory(path)
