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


def podman_command(tree, image, scratch, threads, *, mode, cpus=None, memory=None,
                   executable='podman', harness=None, interpreter='python3', oci_runtime=None, init_path=None):
    harness = Path(harness) if harness is not None else Path(__file__).resolve().parent
    if mode == 'in_job':
        if cpus is not None or memory is not None:
            raise ValueError('in_job cannot set independent CPU or memory limits')
        prefix = [str(executable), 'run', '--cgroups=disabled', '--init']
        if oci_runtime is not None:
            prefix.append(f'--runtime={oci_runtime}')
        if init_path is not None:
            prefix.append(f'--init-path={init_path}')
    elif mode == 'own_scope':
        if not cpus or type(memory) is not int or memory <= 0:
            raise ValueError('own_scope requires Rust-mapped CPUs and a positive memory limit')
        prefix = [str(executable), '--cgroup-manager=systemd', 'run', '--cgroups=enabled',
                  '--cpuset-cpus', cpus, '--memory', str(memory), '--memory-swap', str(memory)]
    else:
        raise ValueError('explicit container mode required')
    return [
        *prefix, '--network=none', '--http-proxy=false', '--read-only', '--read-only-tmpfs=false',
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


def backend_request(command, *, bounds=None):
    """Transport one command to FX1's Rust CLI; do not interpret its domain schema."""
    import selectors
    import signal
    import subprocess
    import time

    bound = bounds['cli_result_bytes'] if bounds is not None else 4 * 1024 * 1024
    request = json.dumps(command).encode() + b'\n'
    if len(request) > bound:
        raise RuntimeError('backend command exceeds transport bound')
    process = subprocess.Popen(
        [os.environ['TL3_RUN_BACKEND_CLI'], *([] if command['op'] == 'output_bounds' else
         ['--config', os.environ['TL3_RUN_BACKEND_CONFIG']])],
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


def output_bounds(suite):
    return backend_request({'op': 'output_bounds', 'suite': suite})['bounds']


def collect_host_payload(command, *, timeout, operation='collect_runner'):
    """Retain raw action bytes. Rust alone extracts and validates the check report."""
    import hashlib
    import selectors
    import signal
    import subprocess
    import time

    if not command or not 0 < timeout <= 86400:
        raise ValueError('bounded host payload command required')
    context = launch_context()
    if context is None:
        raise RuntimeError('host collector requires explicit Flux bootstrap')
    if operation not in ('collect_runner', 'collect_worker'):
        raise ValueError('unsupported Rust collector operation')
    bounds = context['output_bounds']
    root = Path(context['output_dir'])
    stdout_path, stderr_path = root / 'host.stdout', root / 'host.stderr'
    with stdout_path.open('xb') as stdout, stderr_path.open('xb') as stderr:
        process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, start_new_session=True)
        counts = {stdout: 0, stderr: 0}
        limits = {stdout: bounds["stdout_bytes"], stderr: bounds["stderr_bytes"]}
        deadline = time.monotonic() + timeout
        try:
            with selectors.DefaultSelector() as ready:
                ready.register(process.stdout, selectors.EVENT_READ, stdout)
                ready.register(process.stderr, selectors.EVENT_READ, stderr)
                while ready.get_map():
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise RuntimeError('host payload timed out; container cleanup remains unknown')
                    for key, _ in ready.select(remaining):
                        chunk = os.read(key.fd, 65536)
                        if not chunk:
                            ready.unregister(key.fileobj)
                        counts[key.data] += len(chunk)
                        if counts[key.data] > limits[key.data]:
                            stream = 'stdout' if key.data is stdout else 'stderr'
                            raise RuntimeError(f'host payload {stream} exceeds Rust byte bound')
                        key.data.write(chunk)
                code = process.wait(timeout=max(0.001, deadline - time.monotonic()))
        finally:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            try:
                process.wait(timeout=5)
            finally:
                try:
                    process.stdout.close()
                    process.stderr.close()
                    for stream in (stdout, stderr):
                        stream.flush()
                        os.fsync(stream.fileno())
                finally:
                    # Even capture/overflow failures attempt independent cleanup. Rust retains debt.
                    try:
                        cleanup = backend_request({'op': 'cleanup_runtime', 'owner': context['owner'],
                                                   'correlation': context['correlation']})
                    except Exception as error:
                        cleanup = {'error': type(error).__name__}
                    with (root / 'host.cleanup.json').open('x') as retained:
                        json.dump(cleanup, retained, sort_keys=True)
                        retained.flush()
                        os.fsync(retained.fileno())

    def reference(path):
        digest = hashlib.sha256()
        size = 0
        with path.open('rb') as source:
            while chunk := source.read(65536):
                size += len(chunk)
                digest.update(chunk)
        return {'path': str(path), 'bytes': size, 'sha256': digest.hexdigest()}

    result = backend_request({
        'op': operation, 'owner': context['owner'], 'correlation': context['correlation'],
        'facts': {'run': context['run'], 'stdout': reference(stdout_path), 'stderr': reference(stderr_path),
                  'runtime_binding': None, 'runtime_bindings': binding_observations(root), 'cleanup': None},
    })
    return code, result


def binding_observations(root):
    # Exact runtime-owned journal, never a report/evidence filename search.
    path = Path(root) / 'runtime-bindings.jsonl'
    if not path.exists():
        return []
    with path.open() as source:
        return [json.loads(line) for line in source]


def collector_main():
    import argparse

    parser = argparse.ArgumentParser(description='Collect raw host action output through the shared Rust backend')
    parser.add_argument('--timeout', required=True, type=float)
    parser.add_argument('--collector-operation', choices=('collect_runner', 'collect_worker'), default='collect_runner')
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    code, _ = collect_host_payload(command, timeout=args.timeout, operation=args.collector_operation)
    raise SystemExit(code if code >= 0 else 128 - code)


def stage_host_runtime(destination, *, interpreter, producer_commit, podman, systemctl):
    """Stage only raw inputs to Rust prepared::prepare_bundle; never seal/publish."""
    import hashlib
    import re

    if (any(not Path(value).is_absolute() or any(char.isspace() for char in value)
            for value in (interpreter, podman, systemctl))
            or not re.fullmatch(r'[0-9a-f]{40}', producer_commit)):
        raise ValueError('explicit executable paths and producer commit required')
    root = Path(destination).resolve()
    root.mkdir(mode=0o700)
    package = root / 'lib' / 'tl3_harness'
    package.mkdir(parents=True, mode=0o700)
    package.parent.chmod(0o700)

    def write(path, data):
        with path.open('xb') as target:
            path.chmod(0o600)
            target.write(data)
            target.flush()
            os.fsync(target.fileno())

    files = []
    for source in sorted(Path(__file__).resolve().parent.glob('*.py')):
        data = source.read_bytes()
        target = package / source.name
        write(target, data)
        files.append({'path': str(target.relative_to(root)), 'sha256': hashlib.sha256(data).hexdigest(),
                      'bytes': len(data)})
    tool_data = json.dumps({'podman': podman, 'systemctl': systemctl}, sort_keys=True).encode()
    tools_path = package / 'runtime-tools.json'
    write(tools_path, tool_data)
    files.append({'path': str(tools_path.relative_to(root)), 'sha256': hashlib.sha256(tool_data).hexdigest(),
                  'bytes': len(tool_data)})
    manifest = root / 'collector-manifest.json'
    # CodeInventory/CodeFile are the published Rust source shape. No Python types/schema.
    raw = (json.dumps({'interpreter': interpreter, 'files': files}, sort_keys=True) + '\n').encode()
    write(manifest, raw)
    manifest_hash = hashlib.sha256(raw).hexdigest()
    verifier = (
        f'#!{interpreter} -I\n'
        'from pathlib import Path\nimport sys, hashlib, json\n'
        'sys.dont_write_bytecode = True\n'
        'root = Path(__file__).resolve().parent\n'
        'raw = (root / "collector-manifest.json").read_bytes()\n'
        f'if hashlib.sha256(raw).hexdigest() != {manifest_hash!r}:\n'
        '    raise SystemExit("trusted runtime manifest changed")\n'
        'for entry in json.loads(raw)["files"]:\n'
        '    path = (root / entry["path"]).resolve()\n'
        '    if not path.is_relative_to(root):\n'
        '        raise SystemExit("trusted runtime path changed")\n'
        '    data = path.read_bytes()\n'
        '    if len(data) != entry["bytes"] or hashlib.sha256(data).hexdigest() != entry["sha256"]:\n'
        '        raise SystemExit("trusted runtime imported bytes changed")\n'
        'sys.path.insert(0, str(root / "lib"))\n')
    executable, cleanup = root / 'host-collector', root / 'host-cleanup'
    write(executable, (verifier + 'from tl3_harness.runtime import collector_main\ncollector_main()\n').encode())
    cleanup_args = ['--podman', podman, '--systemctl', systemctl]
    write(cleanup, (verifier + f'sys.argv[1:1] = {cleanup_args!r}\n'
                    'from tl3_harness.runtime import cleanup_main\ncleanup_main()\n').encode())
    return executable, manifest, package


def begin_container_launch(child, evidence):
    context = evidence.context
    placement = context['placement']
    if placement['kind'] != 'observed':
        raise RuntimeError(placement['reason'])
    observation = _process_observation({'pid': child.pid, 'start_ticks': 0})
    launcher = {'execution': evidence.container, 'run': context['run'],
                'owner': {'owner': context['owner'], 'correlation': context['correlation']},
                'request_digest': context['request_digest'], 'host': placement['host'], 'rank': placement['rank'],
                'launcher': {'pid': child.pid, 'start_ticks': observation['observed_start_ticks']},
                'process_group': os.getpgid(child.pid), 'container': None}
    result = backend_request({'op': 'begin_container_launch', 'owner': context['owner'],
                              'correlation': context['correlation'], 'launcher': launcher})
    evidence.launch = result['launch']
    evidence.record('launch_intent', evidence.launch)
    return evidence.launch


def runtime_tools():
    # Explicit paths baked into, inventoried and verified with the prepared bundle.
    return json.loads(Path(__file__).with_name('runtime-tools.json').read_text())


def scope_identity(pid, *, systemctl, timeout=10):
    import subprocess

    raw = Path(f'/proc/{pid}/cgroup').read_text()
    rows = raw.splitlines()
    if len(rows) != 1 or not rows[0].startswith('0::/'):
        raise RuntimeError('owned container has no unified cgroup observation')
    cgroup = rows[0][3:]
    path = Path(cgroup)
    unit = path.name
    if not unit.endswith('.scope') or '..' in path.parts:
        raise RuntimeError('actual owned container scope identity unavailable')
    result = subprocess.run([systemctl, '--user', 'show', '--property=Id', '--property=InvocationID',
                             '--property=ControlGroup', '--', unit], stdin=subprocess.DEVNULL,
                            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=timeout, check=True)
    properties = dict(line.split('=', 1) for line in result.stdout.decode().splitlines())
    if properties['Id'] != unit or properties['ControlGroup'] != cgroup:
        raise RuntimeError('actual owned scope differs from systemd observation')
    identity = {'unit': properties['Id'], 'invocation_id': properties['InvocationID'], 'cgroup': cgroup}
    return identity


def scope_facts(pid, identifier, *, systemctl, cgroup_root='/sys/fs/cgroup', timeout=10):
    identity = scope_identity(pid, systemctl=systemctl, timeout=timeout)
    root = Path(cgroup_root).resolve(strict=True)
    directory = (root / Path(identity['cgroup']).relative_to('/')).resolve(strict=True)
    if not directory.is_relative_to(root) or directory == root:
        raise RuntimeError('unsafe owned cgroup observation path')

    def read(name):
        with (directory / name).open() as source:
            value = source.read(65537)
        if len(value) > 65536:
            raise RuntimeError('owned cgroup observation exceeds bound')
        return value.strip()

    memory, swap = read('memory.max'), read('memory.swap.max')
    facts = {**identity, 'cpuset_effective': read('cpuset.cpus.effective'),
             'memory_max': None if memory == 'max' else int(memory),
             'memory_swap_max': None if swap == 'max' else int(swap), 'container_id': identifier}
    return identity, facts


def bind_container_launch(evidence, metadata, *, systemctl=None, cgroup_root='/sys/fs/cgroup', timeout=10):
    launch = evidence.launch
    # Store only the Rust-issued ownership labels; image labels may add unrelated keys.
    if (metadata['Name'].lstrip('/') != launch['name']
            or any(metadata['Labels'].get(key) != value for key, value in launch['labels'].items())):
        raise RuntimeError('observed container differs from durable name/labels')
    scope = None
    evidence.container_scope = None
    if launch['mode'] == 'own_scope':
        scope, evidence.container_scope = scope_facts(
            metadata['Pid'], metadata['Id'], systemctl=systemctl or runtime_tools()['systemctl'],
            cgroup_root=cgroup_root, timeout=timeout)
    owned = {**launch['launcher'], 'container': {'mode': launch['mode'], 'id': metadata['Id'],
             'name': launch['name'], 'labels': launch['labels'], 'scope': scope}}
    context = evidence.context
    binding = backend_request({'op': 'bind_container_launch', 'owner': context['owner'],
                               'correlation': context['correlation'], 'observed': owned})['binding']
    evidence.record('launch_binding', binding)
    evidence.ownership = owned
    if not binding['payload_allowed']:
        raise RuntimeError('container bound for cleanup only; payload gate held')


def validate_runtime_binding(evidence):
    context = evidence.context
    placement = context['placement']
    if placement['kind'] == 'unqualified':
        raise RuntimeError(placement['reason'])

    def process(facts):
        return {'rank': placement['rank'], 'host': placement['host'],
                'allowed_cpus': facts['Cpus_allowed_list'], 'cgroup': facts['cgroup']}

    facts = {'execution': evidence.ownership['execution'], 'container_id': evidence.ownership['container']['id'],
             'run': context['run'], 'request_digest': context['request_digest'],
             'allocation': placement['allocation'], 'topology': placement['topology'],
             'host': process(evidence.host_facts), 'container': process(evidence.container_facts),
             'container_scope': evidence.container_scope}
    result = backend_request({'op': 'validate_runtime_binding', 'owner': context['owner'],
                              'correlation': context['correlation'], 'facts': facts})
    qualification = result['qualification']
    evidence.record('qualification', qualification)
    if not qualification['qualified']:
        raise RuntimeError(qualification['reason'])
    # Different action subprocesses append independent base/head/aggregate bindings.
    import fcntl
    with (Path(context['output_dir']) / 'runtime-bindings.jsonl').open('a') as journal:
        fcntl.flock(journal, fcntl.LOCK_EX)
        journal.write(json.dumps(facts, sort_keys=True) + '\n')
        journal.flush()
        os.fsync(journal.fileno())


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
    import shutil

    check_rootless_directory(os.environ)
    context = launch_context()
    configured = context['runtime']
    if configured['mode'] == 'own_scope':
        raise RuntimeError('OwnScope activation held pending profile/binding/cleanup qualification')
    oci_runtime, init_path = shutil.which('crun'), shutil.which('catatonit')
    if not oci_runtime or not init_path:
        raise RuntimeError('InJob requires provisioned compatible crun and catatonit')
    return podman_command(tree, configured['image'], scratch, context['native_threads'],
                          mode=configured['mode'],
                          executable=configured['executable'], harness=configured['trusted_root'],
                          interpreter=configured['interpreter'], oci_runtime=oci_runtime, init_path=init_path)


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

    template = '{"Id":{{json .ID}},"Name":{{json .Name}},"Labels":{{json .Config.Labels}},"Pid":{{.State.Pid}},"ConmonPid":{{.State.ConmonPid}}}'
    inspected = subprocess.run([executable, 'container', 'inspect', '--format', template, container],
                               stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                               timeout=timeout, check=True)
    metadata = json.loads(inspected.stdout)
    handles = []
    try:
        for name in ('Pid', 'ConmonPid'):
            handles.append(os.pidfd_open(metadata[name]))
        evidence.record('container_processes', metadata)
        if evidence.context is not None:
            bind_container_launch(evidence, metadata, timeout=timeout)
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


def _process_observation(identity):
    # One retained PID only. Never enumerate host processes or /proc.
    try:
        raw = Path(f"/proc/{identity['pid']}/stat").read_text()
    except FileNotFoundError:
        return {'kind': 'absent'}
    ticks = int(raw.rsplit(')', 1)[1].split()[19])
    return {'kind': 'present' if ticks == identity['start_ticks'] else 'reused', 'observed_start_ticks': ticks}


def _scope_observation(identity, *, systemctl, cgroup_root, timeout):
    import re
    import subprocess

    unit = identity['unit']
    if not re.fullmatch(r'[A-Za-z0-9_.:@-]+\.scope', unit):
        raise RuntimeError('unsafe retained scope unit')
    reply = subprocess.run([systemctl, '--user', 'show', '--property=LoadState', '--property=ActiveState',
                            '--property=InvocationID', '--property=ControlGroup', '--', unit],
                           stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                           timeout=timeout, check=False)
    if len(reply.stdout) > 16384:
        raise RuntimeError('scope properties exceed byte bound')
    properties = dict(line.split('=', 1) for line in reply.stdout.decode().splitlines())
    if reply.returncode != 0 and not (reply.returncode == 4 and properties.get('LoadState') == 'not-found'):
        raise RuntimeError('owned scope query failed')
    invocation = properties['InvocationID'] or None
    if invocation is not None and invocation != identity['invocation_id']:
        raise RuntimeError('owned systemd scope generation changed')
    if properties['ControlGroup'] not in ('', identity['cgroup']):
        raise RuntimeError('owned systemd cgroup identity changed')
    relative = Path(identity['cgroup']).relative_to('/')
    root = Path(cgroup_root).resolve(strict=True)
    path = (root / relative).resolve()
    if not relative.parts or '..' in relative.parts or not path.is_relative_to(root) or path == root:
        raise RuntimeError('unsafe retained scope cgroup')
    try:
        info = path.stat()
    except FileNotFoundError:
        events, procs = None, ''
    else:
        if not stat.S_ISDIR(info.st_mode):
            raise RuntimeError('owned cgroup is not a directory')
        events = (path / 'cgroup.events').read_text()
        procs = (path / 'cgroup.procs').read_text()
        if len(events) > 4096 or len(procs) > 65536:
            raise RuntimeError('owned cgroup observations exceed byte bound')
    active = 'not-found' if properties['LoadState'] == 'not-found' else properties['ActiveState']
    return {'identity': identity, 'observed_invocation_id': invocation, 'cgroup_events': events,
            'cgroup_procs': procs, 'active_state': active}


def cleanup_observations(ownership, *, launch=None, podman, systemctl, cgroup_root='/sys/fs/cgroup', timeout=10):
    """Collect exact CleanupEvidence; Rust alone reconciles ownership and clears debt."""
    import signal
    import subprocess
    import time

    group = ownership['process_group']
    if group <= 1 or group == os.getpgrp() or ownership['launcher']['pid'] <= 1:
        raise RuntimeError('cleanup cannot target the helper group or an unsafe identity')

    def capture(arguments):
        result = subprocess.run(arguments, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                stderr=subprocess.DEVNULL, timeout=timeout, check=True)
        if len(result.stdout) > 65536:
            raise RuntimeError('cleanup metadata exceeds byte bound')
        return result.stdout

    def status(arguments):
        return subprocess.run(arguments, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                              stderr=subprocess.DEVNULL, timeout=timeout, check=False).returncode

    template = '{"Id":{{json .ID}},"Name":{{json .Name}},"Labels":{{json .Config.Labels}},"Pid":{{.State.Pid}}}'

    def inspect(identifier, name, labels):
        actual = json.loads(capture([podman, 'container', 'inspect', '--format', template, identifier]))
        if (actual['Name'].lstrip('/') != name
                or any(actual['Labels'].get(key) != value for key, value in labels.items())):
            raise RuntimeError('exact container ownership name/labels changed')
        return actual

    observed = None
    try:
        container = ownership['container']
        if container is None and launch is not None:
            if launch['launcher'] != ownership:
                raise RuntimeError('pending launch differs from exact retained launcher')
            # An absent name after an uncertain launch cannot prove no escaped scope.
            if status([podman, 'container', 'exists', launch['name']]) != 0:
                raise RuntimeError('unbound launch identity unavailable; cleanup remains unknown')
            actual = inspect(launch['name'], launch['name'], launch['labels'])
            scope = (scope_identity(actual['Pid'], systemctl=systemctl, timeout=timeout)
                     if launch['mode'] == 'own_scope' else None)
            container = {'id': actual['Id'], 'name': launch['name'], 'labels': launch['labels'],
                         'mode': launch['mode'], 'scope': scope}
            ownership = {**ownership, 'container': container}
        if container is not None:
            identifier, name, labels = container['id'], container['name'], container['labels']
            filters = [arg for key, value in sorted(labels.items()) for arg in ('--filter', f'label={key}={value}')]
            if not filters:
                raise RuntimeError('cleanup requires retained ownership labels')
            sweep_command = [podman, 'ps', '--all', '--no-trunc', *filters, '--format', '{{.ID}}']
            sweep = capture(sweep_command).decode().split()
            if len(sweep) > 1 or any(item != identifier for item in sweep):
                raise RuntimeError('label-filtered sweep differs from retained container')
            present = status([podman, 'container', 'exists', identifier])
            named = status([podman, 'container', 'exists', name])
            if present == 0:
                actual = inspect(identifier, name, labels)
                if actual['Id'] != identifier:
                    raise RuntimeError('exact container identity changed')
            elif present != 1:
                raise RuntimeError('container existence is unknown')
            if named == 0:
                if inspect(name, name, labels)['Id'] != identifier:
                    raise RuntimeError('retained container name was reused')
            elif named != 1:
                raise RuntimeError('container name existence is unknown')
            scope = container['scope']
            if scope is not None:
                _scope_observation(scope, systemctl=systemctl, cgroup_root=cgroup_root, timeout=timeout)
            removed = status([podman, 'rm', '--force', '--ignore', identifier])
            absent = status([podman, 'container', 'exists', identifier])
            name_absent = status([podman, 'container', 'exists', name])
            remaining = capture(sweep_command).decode().split()
            observed = {'id': identifier, 'name': name, 'name_exists_exit': name_absent,
                        'remove_exit': removed, 'exists_exit': absent, 'sweep_ids': sweep,
                        'sweep_exit': 0, 'remaining_ids': remaining,
                        'scope': _scope_observation(scope, systemctl=systemctl, cgroup_root=cgroup_root, timeout=timeout)
                        if scope is not None else None}
    finally:
        # Query/removal timeouts never skip stopping the exact owned launcher.
        launcher = _process_observation(ownership['launcher'])
        if launcher['kind'] == 'present':
            handle = os.pidfd_open(ownership['launcher']['pid'])
            try:
                if (_process_observation(ownership['launcher'])['kind'] != 'present'
                        or os.getpgid(ownership['launcher']['pid']) != group):
                    raise RuntimeError('retained launcher/group identity changed')
                os.killpg(group, signal.SIGTERM)
                if not processes_stopped([handle], timeout):
                    os.killpg(group, signal.SIGKILL)
                    processes_stopped([handle], timeout)
            finally:
                os.close(handle)
        deadline = time.monotonic() + timeout
        while True:
            try:
                os.killpg(group, 0)
                group_errno = None
            except OSError as error:
                group_errno = error.errno
            if group_errno is not None or time.monotonic() >= deadline:
                break
            time.sleep(min(0.05, max(0, deadline - time.monotonic())))
        launcher = _process_observation(ownership['launcher'])
    return {'schema': 'tl3.cleanup_evidence.v1', 'ownership': ownership, 'launcher': launcher,
            'process_group_probe_errno': group_errno, 'container': observed}


def cleanup_main():
    import argparse
    import sys

    parser = argparse.ArgumentParser(description='Independent exact-ownership runtime cleanup observations')
    parser.add_argument('--podman', required=True)
    parser.add_argument('--systemctl', required=True)
    parser.add_argument('--timeout', type=float, default=10)
    args = parser.parse_args()
    check_rootless_directory(os.environ)
    if not 0 < args.timeout <= 60:
        raise ValueError('cleanup timeout must be positive and at most 60 seconds')
    raw = sys.stdin.buffer.read(1024 * 1024 + 1)
    if len(raw) > 1024 * 1024:
        raise ValueError('cleanup input exceeds byte bound')
    request = json.loads(raw)
    if request['schema'] != 'tl3.cleanup_input.v1':
        raise ValueError('unsupported Rust cleanup input')
    evidence = cleanup_observations(request['ownership'], launch=request['launch'], podman=args.podman, systemctl=args.systemctl,
                                    timeout=args.timeout)
    print(json.dumps(evidence, sort_keys=True))
