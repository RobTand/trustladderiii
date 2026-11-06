"""Host-side runtime I/O for the pytest edge; Rust owns the launch contract."""
import json
import os
import stat
import tempfile
import uuid
from contextlib import contextmanager
from pathlib import Path

from .model import NATIVE_THREAD_ENVIRONMENT


def launch_context():
    selected = os.environ.get('TL3_EXECUTION_BACKEND', 'prismabuild')
    if selected == 'prismabuild':
        return None
    if selected != 'flux':
        raise RuntimeError('unknown execution backend')
    return backend_request({
        'op': 'launch_context', 'owner': os.environ['TL3_RUN_OWNER'],
        'correlation': os.environ['TL3_RUN_CORRELATION'], 'job': os.environ['FLUX_JOB_ID'],
    })['context']


def podman_command(tree, image, scratch, threads, *, executable='podman', harness=None, interpreter='python3'):
    harness = Path(harness) if harness is not None else Path(__file__).resolve().parent
    return [
        str(executable), 'run', '--cgroups=disabled', '--network=none', '--read-only', '--read-only-tmpfs=false', '--init',
        '--userns=keep-id', '--interactive', '--rm', '--pull=never', '--cap-drop=ALL',
        '--security-opt=no-new-privileges', '--pids-limit=128', '--ipc=private', '--pid=private', '--workdir=/checkout',
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
    return Evidence(context['output_dir'], container, context=context)


class Evidence:
    """Keep runtime observations separate from the runner's stdout journal."""

    def __init__(self, output, container, *, context=None):
        self.path = Path(output) / f'{container}.runtime.jsonl'
        self.container_seen = False
        self.container = container
        self.context = context
        self.host_facts = self.host_snapshot()
        self.container_facts = None
        self.record('host', self.host_facts)
        if context is not None:
            self.record('launch', {'context': context, 'container_name': container})

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
        self.container_facts = facts
        self.container_seen = True
        return True


def backend_request(command):
    """Transport one command to FX1's Rust CLI; do not interpret its domain schema."""
    import selectors
    import signal
    import subprocess
    import time

    bound = 4 * 1024 * 1024
    request = json.dumps(command).encode() + b'\n'
    if len(request) > bound:
        raise RuntimeError('backend command exceeds transport bound')
    process = subprocess.Popen(
        [os.environ['TL3_RUN_BACKEND_CLI'], '--config', os.environ['TL3_RUN_BACKEND_CONFIG']],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, start_new_session=True)
    output = bytearray()
    pending = memoryview(request)
    deadline = time.monotonic() + 30
    try:
        with selectors.DefaultSelector() as ready:
            os.set_blocking(process.stdin.fileno(), False)
            ready.register(process.stdin, selectors.EVENT_WRITE)
            ready.register(process.stdout, selectors.EVENT_READ)
            while ready.get_map():
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise RuntimeError('backend CLI transport timed out')
                for key, _ in ready.select(remaining):
                    if key.fileobj is process.stdin:
                        pending = pending[os.write(key.fd, pending[:65536]):]
                        if not pending:
                            ready.unregister(key.fileobj)
                            process.stdin.close()
                    else:
                        chunk = os.read(key.fd, 65536)
                        if not chunk:
                            ready.unregister(key.fileobj)
                        output.extend(chunk)
                        if len(output) > bound:
                            raise RuntimeError('backend CLI response exceeds transport bound')
            code = process.wait(timeout=max(0.001, deadline - time.monotonic()))
        frames = output.splitlines()
        if code != 0 or len(frames) != 1:
            raise RuntimeError('backend CLI did not return one successful result')
        frame = json.loads(frames[0])
        if (frame['schema'] != 'tl3.run_backend.frame.v1' or frame['sequence'] != 0
                or frame['kind'] != 'result' or frame['result']['op'] != command['op']):
            raise RuntimeError('backend CLI returned an invalid result frame')
        return frame['result']
    except (ValueError, KeyError, TypeError, OSError, subprocess.SubprocessError) as error:
        raise RuntimeError('backend CLI transport failed') from error
    finally:
        # The observer process group is local plumbing, never a native cancel.
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait()
        process.stdin.close()
        process.stdout.close()


def validate_runtime_binding(evidence):
    context = evidence.context
    placement = context['placement']
    if placement['kind'] == 'unqualified':
        raise RuntimeError(placement['reason'])

    def process(facts):
        return {'rank': placement['rank'], 'host': placement['host'],
                'allowed_cpus': facts['Cpus_allowed_list'], 'cgroup': facts['cgroup']}

    result = backend_request({
        'op': 'validate_runtime_binding', 'owner': context['owner'], 'correlation': context['correlation'],
        'facts': {'run': context['run'], 'request_digest': context['request_digest'],
                  'allocation': placement['allocation'], 'topology': placement['topology'],
                  'host': process(evidence.host_facts), 'container': process(evidence.container_facts)},
    })
    qualification = result['qualification']
    evidence.record('qualification', qualification)
    if not qualification['qualified']:
        raise RuntimeError(qualification['reason'])


_uncertain_containers = set()


def cache_owner():
    context = launch_context()
    if context is not None:
        run = context['run']
        return run['backend'], run['uri'], run['instance'], run['job']
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
    check_rootless_directory(os.environ)
    context = launch_context()
    configured = context['runtime']
    return podman_command(tree, configured['image'], scratch, context['native_threads'],
                          executable=configured['executable'], harness=configured['trusted_root'],
                          interpreter=configured['interpreter'])


def prepare_scratch():
    check_rootless_directory(os.environ)
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


def capture_processes(executable, container, evidence, timeout):
    """Hold kernel process identities while the trusted container wrapper is gated."""
    import subprocess

    template = '{"Id":{{json .Id}},"Pid":{{.State.Pid}},"ConmonPid":{{.State.ConmonPid}}}'
    inspected = subprocess.run([executable, 'container', 'inspect', '--format', template, container],
                               stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                               timeout=timeout, check=True)
    metadata = json.loads(inspected.stdout)
    handles = []
    try:
        for name in ('Pid', 'ConmonPid'):
            handles.append(os.pidfd_open(metadata[name]))
        evidence.record('container_processes', metadata)
        return handles
    except BaseException:
        for handle in handles:
            os.close(handle)
        raise


def processes_stopped(handles, timeout):
    import selectors
    import time

    if not handles:
        return False
    deadline = time.monotonic() + timeout
    with selectors.DefaultSelector() as ready:
        for handle in handles:
            ready.register(handle, selectors.EVENT_READ)
        while ready.get_map():
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return False
            for key, _ in ready.select(remaining):
                ready.unregister(key.fd)
    return True


def check_rootless_directory(environment):
    if os.geteuid() == 0:
        raise RuntimeError('Flux test containers require rootless Podman')
    if not environment.get('HOME') or not environment.get('XDG_RUNTIME_DIR'):
        raise RuntimeError('rootless Podman requires explicitly provisioned HOME and XDG_RUNTIME_DIR')
    info = Path(environment['XDG_RUNTIME_DIR']).lstat()
    if (not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid()
            or info.st_mode & 0o777 != 0o700):
        raise RuntimeError('rootless runtime directory must be owned by the job UID with mode 0700')
