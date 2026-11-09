"""Host-side runtime I/O for the pytest edge; Rust owns the launch contract."""
import json
import os
import stat
import uuid
import atexit
import threading
from contextlib import contextmanager
from pathlib import Path

from .model import NATIVE_THREAD_ENVIRONMENT


def launch_context():
    # Flux is the only executor (PB-REMOVAL): a missing or other backend is refused.
    if os.environ.get('TL3_EXECUTION_BACKEND') != 'flux':
        raise RuntimeError('unknown execution backend; Flux is the only executor')
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
        prefix = [str(executable), 'run', '--cgroups=disabled', '--cgroupns=host', '--init']
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
        '--interactive', '--rm', '--pull=never', '--cap-drop=ALL',
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
    return Evidence(context['output_dir'], container, context=context)


class Evidence:
    """Keep runtime observations separate from the runner's stdout journal."""

    def __init__(self, output, container, *, context=None):
        self.path = Path(output) / f'{container}.runtime.jsonl'
        self.container_seen = False
        self.container = container
        self.launch_requested = False
        self.context = context
        self.host_facts = self.host_snapshot()
        self.container_facts = None
        self.record('host', self.host_facts)
        if context is not None:
            self.record('launch', {'context': context, 'container_name': container})

    @staticmethod
    def host_snapshot():
        from .runtime_evidence import snapshot
        return {**snapshot(), 'hostname': os.uname().nodename}

    def record(self, event, facts):
        mode = 'x' if event == 'host' else 'a'
        with self.path.open(mode) as target:
            target.write(json.dumps({'event': event, **facts}, sort_keys=True) + '\n')
            target.flush()
            os.fsync(target.fileno())
        if mode == 'x':
            descriptor = os.open(self.path.parent, os.O_RDONLY | os.O_DIRECTORY)
            try:
                os.fsync(descriptor)
            finally:
                os.close(descriptor)

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


BACKEND_OUTPUT_ROOT = Path('/mnt/shared/flux-out/tl3')
BACKEND_IDLE_SECONDS = 30
DIAGNOSTIC_CAPTURE_BYTES = 65536
DIAGNOSTIC_TEXT_BYTES = 16384


class BackendCliError(RuntimeError):
    def __init__(self, diagnostic):
        self.diagnostic = diagnostic
        super().__init__('backend CLI failure: ' + json.dumps(diagnostic, ensure_ascii=False, sort_keys=True))


class _BackendTransportError(RuntimeError):
    def __init__(self, kind, message):
        self.kind = kind
        super().__init__(message)


class _DiagnosticStream:
    def __init__(self):
        self.prefix = bytearray()
        self.received = 0

    def add(self, chunk):
        self.received += len(chunk)
        self.prefix.extend(chunk[:max(0, DIAGNOSTIC_CAPTURE_BYTES - len(self.prefix))])

    def sanitized(self):
        from .redaction import Clip, Redactor

        raw = bytes(self.prefix)
        omitted = self.received - len(raw)
        if omitted:
            # A partial final token or private key is never exposed at a capture cut.
            boundary = raw.rfind(b'\n') + 1
            omitted += len(raw) - boundary
            raw = raw[:boundary]
        text = raw.decode('utf-8', errors='replace')
        chars = DIAGNOSTIC_TEXT_BYTES // 4
        if len(text) > chars:
            head = chars // 4
            tail = len(text) - (chars - head)
            text = Clip([(text, 0, head), (text, tail, len(text))], gap=tail - head)
        text = Redactor(None).text(text, limit=chars)
        text = ''.join(c for c in text if c.isprintable() or c in '\n\t')
        if omitted:
            text += f' [omitted: {omitted} stream bytes beyond capture bound]'
        # The shared redactor sees context before clipping. Bound encoded UTF-8 independently.
        encoded = text.encode('utf-8')
        if len(encoded) > DIAGNOSTIC_TEXT_BYTES:
            text = encoded[:DIAGNOSTIC_TEXT_BYTES - 64].decode('utf-8', errors='ignore') + ' [omitted: diagnostic byte bound]'
        return {'text': text, 'received_bytes': self.received, 'capture_omitted_bytes': omitted}


def _retain_backend_diagnostic(diagnostic):
    # Rust's validated request installs these values, overriding caller environment.
    # This remains available before launch_context succeeds and carries no job binding.
    correlation = os.environ.get('TL3_RUN_CORRELATION', '')
    directory = os.environ.get('TL3_RUN_DIAGNOSTIC_DIR')
    if (not correlation or correlation in ('.', '..') or len(correlation) > 128
            or any(not (c.isascii() and (c.isalnum() or c in '-_.')) for c in correlation)
            or directory != os.environ.get('OUT') or directory != str(BACKEND_OUTPUT_ROOT / correlation)):
        raise RuntimeError('validated Rust bootstrap diagnostic output authority unavailable')
    descriptor = os.open('/', os.O_RDONLY | os.O_DIRECTORY)
    try:
        for part in Path(directory).parts[1:]:
            child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=descriptor)
            os.close(descriptor)
            descriptor = child
        name = f'backend-cli-{uuid.uuid4().hex}.json'
        temporary = '.' + name
        handle = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=descriptor)
        with os.fdopen(handle, 'w', encoding='utf-8') as target:
            # JSON escapes also retain undecodable path bytes represented by surrogates.
            json.dump(diagnostic, target, ensure_ascii=True, sort_keys=True)
            target.write('\n')
            target.flush()
            os.fsync(target.fileno())
        os.rename(temporary, name, src_dir_fd=descriptor, dst_dir_fd=descriptor)
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


_backend_session = None
_backend_session_key = None
_backend_request_id = 0
_backend_lock = threading.RLock()


def close_backend_session():
    global _backend_session, _backend_session_key, _backend_request_id
    process, _backend_session = _backend_session, None
    key = _backend_session_key
    _backend_session_key = None
    _backend_request_id = 0
    if process is not None:
        try:
            if key is not None and key[0] == os.getpid():
                with _adopt_local_children():
                    _stop_backend_cli(process, _DiagnosticStream(), _DiagnosticStream())
        finally:
            for pipe in (process.stdin, process.stdout, process.stderr):
                pipe.close()


atexit.register(close_backend_session)


def _after_fork():
    global _backend_lock
    # The child closes its copies, never signals or waits for the parent's CLI.
    close_backend_session()
    _backend_lock = threading.RLock()


os.register_at_fork(after_in_child=_after_fork)


def backend_request(command, *, bounds=None):
    # One action owns the session. Each Rust command still reloads its journal,
    # validates Options and checks any required native generation/facts.
    with _backend_lock, _adopt_local_children():
        return _backend_request(command, bounds=bounds)


def _backend_request(command, *, bounds=None):
    """Transport one command to FX1's Rust CLI; do not interpret its domain schema."""
    import selectors
    import subprocess
    import time

    global _backend_session, _backend_session_key, _backend_request_id

    cleanup = command['op'] in ('cleanup_runtime', 'cancel')
    limits = backend_request({'op': 'cleanup_limits'})['limits'] if cleanup else None
    binding = launch_context() if cleanup else None
    if cleanup:
        if (not isinstance(limits, dict) or any(type(value) is not int or value <= 0 for value in limits.values())
                or not limits['helper_budget_seconds'] < limits['native_timeout_seconds'] < limits['idle_timeout_seconds']
                or binding['owner'] != command['owner']
                or binding['correlation'] != command['correlation']):
            raise RuntimeError('invalid Rust cleanup transport limits or binding')
    bound = limits['max_response_bytes'] if cleanup else (
        bounds['cli_result_bytes'] if bounds is not None else 4 * 1024 * 1024)
    request = json.dumps(command).encode() + b'\n'
    if len(request) > bound:
        raise RuntimeError('backend command exceeds transport bound')
    process = None
    stdout_evidence, stderr_evidence = _DiagnosticStream(), _DiagnosticStream()
    output = bytearray()
    pending = memoryview(request)
    idle = limits['idle_timeout_seconds'] if cleanup else BACKEND_IDLE_SECONDS
    deadline = time.monotonic() + idle
    sequence, received, executions = 0, 0, set()
    terminal = None
    parse_error = None
    exited_at = None
    stdout_closed = False
    session = os.environ.get('TL3_RUN_BACKEND_SESSION') == '1'
    request_id = None

    def accept(raw):
        nonlocal sequence, deadline, terminal
        frame = json.loads(raw)
        if (frame['schema'] != 'tl3.run_backend.frame.v1' or type(frame['sequence']) is not int
                or frame['sequence'] != sequence or terminal is not None
                or (session and (type(frame.get('request_id')) is not int or frame['request_id'] != request_id))):
            raise _BackendTransportError('invalid_frame', 'backend CLI returned an invalid frame sequence')
        sequence += 1
        if frame['kind'] == 'cleanup_progress':
            if not cleanup or len(raw) > limits['max_frame_bytes'] or len(executions) >= limits['max_progress_frames']:
                raise _BackendTransportError('invalid_progress', 'backend CLI returned unexpected or excessive cleanup progress')
            progress = frame['progress']
            execution = progress['execution']
            if (progress['owner'] != {'owner': command['owner'], 'correlation': command['correlation']}
                    or progress['run'] != binding['run'] or progress['request_digest'] != binding['request_digest']
                    or not isinstance(execution, str) or not execution or execution in executions
                    or progress['helper_budget_seconds'] != limits['helper_budget_seconds']
                    or progress['idle_timeout_seconds'] != idle):
                raise _BackendTransportError('invalid_progress', 'backend CLI cleanup progress differs from retained binding')
            executions.add(execution)
            deadline = time.monotonic() + idle
        elif frame['kind'] in ('result', 'error'):
            if frame['kind'] == 'result' and frame['result']['op'] != command['op']:
                raise _BackendTransportError('invalid_frame', 'backend CLI returned the wrong result operation')
            if frame['kind'] == 'error' and (not isinstance(frame.get('code'), str) or not isinstance(frame.get('message'), str)):
                raise _BackendTransportError('invalid_frame', 'backend CLI returned an invalid error frame')
            terminal = frame
        else:
            raise _BackendTransportError('invalid_frame', 'backend CLI returned an unexpected frame')
    try:
        if session:
            import hashlib

            config = os.environ['TL3_RUN_BACKEND_CONFIG']
            with Path(config).open('rb') as source:
                raw_config = source.read(8 * 1024 * 1024 + 1)
            if len(raw_config) > 8 * 1024 * 1024:
                raise RuntimeError('backend session config exceeds byte bound')
            key = (os.getpid(), os.environ['TL3_RUN_BACKEND_CLI'], config,
                   hashlib.sha256(raw_config).digest(),
                   tuple(os.environ.get(name) for name in ('TL3_RUN_OWNER', 'TL3_RUN_CORRELATION',
                         'TL3_RUN_INSTANCE', 'TL3_RUN_REQUEST_DIGEST', 'FLUX_JOB_ID')),
                   command.get('owner', os.environ.get('TL3_RUN_OWNER')),
                   command.get('correlation', os.environ.get('TL3_RUN_CORRELATION')))
            if _backend_session_key != key or (_backend_session is not None and _backend_session.poll() is not None):
                close_backend_session()
            if _backend_session is None:
                _backend_session = subprocess.Popen(
                    [key[1], '--session', '--config', key[2]], bufsize=0,
                    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
                _backend_session_key = key
            process = _backend_session
            _backend_request_id += 1
            request_id = _backend_request_id
            request = json.dumps({'request_id': request_id, 'command': command}).encode() + b'\n'
            if len(request) > bound:
                raise RuntimeError('backend session command exceeds transport bound')
            pending = memoryview(request)
        else:
            process = subprocess.Popen(
                [os.environ['TL3_RUN_BACKEND_CLI'], *([] if command['op'] in ('output_bounds', 'cleanup_limits') else
                 ['--config', os.environ['TL3_RUN_BACKEND_CONFIG']])],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
        with selectors.DefaultSelector() as ready:
            os.set_blocking(process.stdin.fileno(), False)
            ready.register(process.stdin, selectors.EVENT_WRITE)
            ready.register(process.stdout, selectors.EVENT_READ)
            ready.register(process.stderr, selectors.EVENT_READ)
            while ready.get_map():
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise _BackendTransportError('timeout', 'backend CLI transport timed out')
                if process.poll() is not None:
                    if exited_at is None:
                        import signal

                        exited_at = time.monotonic()
                        try:
                            os.killpg(process.pid, signal.SIGKILL)
                        except ProcessLookupError:
                            pass
                    elif time.monotonic() - exited_at >= 1:
                        raise _BackendTransportError('unclosed_pipes', 'backend CLI exited with unclosed descendant pipes')
                for key, _ in ready.select(min(remaining, 0.05)):
                    if key.fileobj is process.stdin:
                        pending = pending[os.write(key.fd, pending[:65536]):]
                        if not pending:
                            ready.unregister(key.fileobj)
                            if not session:
                                process.stdin.close()
                    else:
                        chunk = os.read(key.fd, 65536)
                        if not chunk:
                            ready.unregister(key.fileobj)
                            if key.fileobj is process.stdout:
                                stdout_closed = True
                        stream = stdout_evidence if key.fileobj is process.stdout else stderr_evidence
                        stream.add(chunk)
                        if key.fileobj is process.stderr:
                            continue
                        received += len(chunk)
                        if received > bound:
                            raise _BackendTransportError('stdout_oversize', 'backend CLI response exceeds transport bound')
                        if parse_error is not None:
                            continue
                        output.extend(chunk)
                        while b'\n' in output:
                            raw, _, remainder = output.partition(b'\n')
                            output[:] = remainder
                            try:
                                accept(raw)
                            except (ValueError, KeyError, TypeError, RuntimeError) as error:
                                parse_error = error
                                output.clear()
                                break
                if session and stdout_closed and terminal is None and parse_error is None:
                    raise _BackendTransportError('child_eof', 'backend CLI stdout closed before a terminal result')
                if session and parse_error is not None:
                    raise parse_error
                if session and terminal is not None and terminal['kind'] == 'result':
                    break
            code = (0 if session and terminal is not None and terminal['kind'] == 'result' and parse_error is None
                    else process.wait(timeout=max(0.001, deadline - time.monotonic())))
        if parse_error is not None:
            raise parse_error
        if output:
            raise _BackendTransportError('truncated', 'backend CLI returned a truncated frame')
        if terminal is not None and terminal['kind'] == 'error':
            raise _BackendTransportError('structured_error', 'backend CLI returned a structured error')
        if code != 0:
            raise _BackendTransportError('nonzero_exit', 'backend CLI exited unsuccessfully')
        if terminal is None:
            raise _BackendTransportError('missing_result', 'backend CLI returned no terminal result')
        return terminal['result']
    except (ValueError, KeyError, TypeError, OSError, RuntimeError, subprocess.SubprocessError) as error:
        from .redaction import Redactor

        if process is not None:
            _stop_backend_cli(process, stdout_evidence, stderr_evidence)
        code = process.returncode if process is not None else None
        cause = {'kind': getattr(error, 'kind', type(error).__name__), 'message': str(error)}
        if terminal is not None and terminal['kind'] == 'error':
            cause = {'kind': 'structured_error', 'code': terminal['code'], 'message': terminal['message'],
                     'transport': str(error)}
        cause = {key: Redactor(None).text(str(value), limit=2048) for key, value in cause.items()}
        diagnostic = {'schema': 'tl3.backend_cli_diagnostic.v1', 'operation': command['op'], 'cause': cause,
                      'status': {'code': code if code is not None and code >= 0 else None,
                                 'signal': -code if code is not None and code < 0 else None},
                      'stdout': stdout_evidence.sanitized(), 'stderr': stderr_evidence.sanitized()}
        try:
            _retain_backend_diagnostic(diagnostic)
        except (OSError, RuntimeError, ValueError) as retention:
            diagnostic['retention_error'] = Redactor(None).text(str(retention), limit=1024)
        raise BackendCliError(diagnostic) from None
    finally:
        if session and (terminal is None or terminal['kind'] != 'result' or parse_error is not None):
            close_backend_session()
        elif process is not None and not session:
            _stop_backend_cli(process, stdout_evidence, stderr_evidence)
            process.stdin.close()
            process.stdout.close()
            process.stderr.close()


def _stop_backend_cli(process, stdout, stderr):
    import signal

    # Local observer plumbing only. Never cancel or inspect a native job here.
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.wait(timeout=2)
    _reap_owned_group(process.pid, 1)
    for pipe, evidence in ((process.stdout, stdout), (process.stderr, stderr)):
        os.set_blocking(pipe.fileno(), False)
        # An escaped, unowned writer cannot keep diagnostic shutdown draining forever.
        for _ in range(4):
            try:
                chunk = os.read(pipe.fileno(), 65536)
            except BlockingIOError:
                break
            if not chunk:
                break
            evidence.add(chunk)


def output_bounds(suite):
    return backend_request({'op': 'output_bounds', 'suite': suite})['bounds']


def collect_host_payload(command, *, timeout, operation='collect_runner'):
    with _adopt_local_children():
        try:
            return _collect_host_payload(command, timeout=timeout, operation=operation)
        finally:
            close_backend_session()


@contextmanager
def _adopt_local_children():
    import ctypes

    libc = ctypes.CDLL(None, use_errno=True)
    previous = ctypes.c_int()
    if libc.prctl(37, ctypes.byref(previous), 0, 0, 0) != 0 or libc.prctl(36, 1, 0, 0, 0) != 0:
        raise OSError(ctypes.get_errno(), 'cannot own local orphan cleanup')
    try:
        yield
    finally:
        if libc.prctl(36, previous.value, 0, 0, 0) != 0:
            raise OSError(ctypes.get_errno(), 'cannot restore local orphan cleanup')


def _reap_owned_group(group, timeout):
    import time

    deadline = time.monotonic() + timeout
    while True:
        try:
            pid, _ = os.waitpid(-group, os.WNOHANG)
        except ChildProcessError:
            return
        if not pid:
            if time.monotonic() >= deadline:
                raise RuntimeError('owned local descendants did not exit; cessation remains unknown')
            time.sleep(0.01)


def _collect_host_payload(command, *, timeout, operation):
    """Retain raw action bytes. Rust alone extracts and validates the check report."""
    import hashlib
    import selectors
    import signal
    import subprocess
    import time

    if not command or not 0 < timeout <= 86400:
        raise ValueError('bounded host payload command required')
    context = launch_context()
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
        exited_at = None
        try:
            with selectors.DefaultSelector() as ready:
                ready.register(process.stdout, selectors.EVENT_READ, stdout)
                ready.register(process.stderr, selectors.EVENT_READ, stderr)
                while ready.get_map():
                    if process.poll() is not None:
                        if exited_at is None:
                            exited_at = time.monotonic()
                            # A dead action cannot finish its pipe-holding local children.
                            try:
                                os.killpg(process.pid, signal.SIGKILL)
                            except ProcessLookupError:
                                pass
                        elif time.monotonic() - exited_at >= 1:
                            raise RuntimeError('host action exited with unclosed descendant pipes; ownership remains unknown')
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise RuntimeError('host payload timed out; container cleanup remains unknown')
                    for key, _ in ready.select(min(remaining, 0.05)):
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
                _reap_owned_group(process.pid, 1)
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


def prepare_allocation(evidence):
    context = evidence.context
    if context['runtime_identity']['cleanup'] is None:
        raise RuntimeError('canonical scratch requires a pinned cleanup helper; bundle misconfiguration')
    host = evidence.host_facts
    observed = {'run': context['run'], 'request_digest': context['request_digest'],
                'hostname': host['hostname'], 'allowed_cpus': host['Cpus_allowed_list'], 'cgroup': host['cgroup']}
    facts = backend_request({'op': 'observe_allocation', 'owner': context['owner'],
                             'correlation': context['correlation'], 'observed': observed})['facts']
    grant = backend_request({'op': 'validate_allocation', 'owner': context['owner'],
                             'correlation': context['correlation'], 'facts': facts})['grant']
    refreshed = launch_context()
    if (not grant['host'] or grant['host'] != host['hostname']
            or grant['run'] != context['run'] or grant['request_digest'] != context['request_digest']
            or refreshed['run'] != context['run'] or refreshed['request_digest'] != context['request_digest']
            or refreshed['placement'] != {'kind': 'observed', **{key: grant[key] for key in
                ('allocation', 'topology', 'host', 'rank', 'os_pus')}}):
        raise RuntimeError('launch context does not contain the exact validated allocation grant')
    evidence.context = refreshed
    evidence.record('allocation_grant', grant)


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
    # From this point Rust may persist intent/debt even if no reply reaches us.
    evidence.launch_requested = True
    container_pending(evidence.container)
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
    run = launch_context()['run']
    return run['backend'], run['uri'], run['instance'], run['job']


def _action_record(context, name, facts):
    with (Path(context['output_dir']) / name).open('x') as retained:
        json.dump(facts, retained, sort_keys=True)
        retained.flush()
        os.fsync(retained.fileno())


@contextmanager
def cancellation_unwind():
    """Give the generated action's workspace finally the existing TERM grace."""
    import signal
    import time

    context = launch_context()
    previous = signal.getsignal(signal.SIGTERM)

    def terminate(number, frame):
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        try:
            _action_record(context, 'action-sigterm.json',
                           {'signal': number, 'monotonic_ns': time.monotonic_ns()})
        finally:
            raise SystemExit(128 + number)

    signal.signal(signal.SIGTERM, terminate)
    try:
        yield
    finally:
        signal.signal(signal.SIGTERM, previous)


def temporary_root():
    return Path(launch_context()['scratch']['leaf'])


@contextmanager
def workspace(prefix):
    root = temporary_root()
    context = launch_context()
    from .scratch import private_directory, release_directory
    identity = root.lstat()
    directory = private_directory(root, prefix=prefix)
    try:
        yield directory
    finally:
        if not _uncertain_containers:
            release_directory(directory)
            current = root.lstat()
            if (not stat.S_ISDIR(current.st_mode) or current.st_uid != os.getuid()
                    or (current.st_dev, current.st_ino) != (identity.st_dev, identity.st_ino)):
                raise RuntimeError('job scratch identity changed before cleanup')
            # This outer workspace owns the job root; remove only an empty root.
            if not any(root.iterdir()):
                root.rmdir()
                import time
                _action_record(context, 'action-scratch-absence.json',
                               {'path': str(root), 'kind': 'absent', 'attribution': 'action workspace finally',
                                'monotonic_ns': time.monotonic_ns()})


def journal_path(path):
    context = launch_context()
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
    return launch_context()['progress']['no_progress_seconds']


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


def cleanup_observations(ownership, *, launch=None, podman, systemctl, cgroup_root='/sys/fs/cgroup', timeout):
    """Collect exact CleanupEvidence; Rust alone reconciles ownership and clears debt."""
    import signal
    import subprocess
    import time

    group = ownership['process_group']
    if group <= 1 or group == os.getpgrp() or ownership['launcher']['pid'] <= 1:
        raise RuntimeError('cleanup cannot target the helper group or an unsafe identity')

    if not 0 < timeout < float('inf'):
        raise ValueError('cleanup total budget must be positive and finite')
    started_ns = time.monotonic_ns()
    deadline = time.monotonic() + timeout

    def remaining():
        seconds = deadline - time.monotonic()
        if seconds <= 0:
            raise subprocess.TimeoutExpired('runtime cleanup budget', timeout)
        return seconds

    def stop_launcher():
        launcher = _process_observation(ownership['launcher'])
        if launcher['kind'] != 'present':
            return
        handle = os.pidfd_open(ownership['launcher']['pid'])
        try:
            if (_process_observation(ownership['launcher'])['kind'] != 'present'
                    or os.getpgid(ownership['launcher']['pid']) != group):
                raise RuntimeError('retained launcher/group identity changed')
            grace = min(remaining() / 4, 1)
            os.killpg(group, signal.SIGTERM)
            if not processes_stopped([handle], grace):
                os.killpg(group, signal.SIGKILL)
                if not processes_stopped([handle], min(remaining(), grace)):
                    raise subprocess.TimeoutExpired('owned launcher stop', timeout)
        finally:
            os.close(handle)

    def capture(arguments):
        result = subprocess.run(arguments, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                stderr=subprocess.DEVNULL, timeout=remaining(), check=True)
        if len(result.stdout) > 65536:
            raise RuntimeError('cleanup metadata exceeds byte bound')
        return result.stdout

    def status(arguments):
        return subprocess.run(arguments, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                              stderr=subprocess.DEVNULL, timeout=remaining(), check=False).returncode

    template = '{"Id":{{json .ID}},"Name":{{json .Name}},"Labels":{{json .Config.Labels}},"Pid":{{.State.Pid}}}'

    def inspect(identifier, name, labels):
        actual = json.loads(capture([podman, 'container', 'inspect', '--format', template, identifier]))
        import re
        if (not isinstance(actual, dict) or not isinstance(actual.get('Name'), str)
                or not isinstance(actual.get('Labels'), dict)
                or not isinstance(actual.get('Id'), str) or not re.fullmatch('[0-9a-f]{64}', actual['Id'])
                or actual['Name'].lstrip('/') != name
                or any(actual['Labels'].get(key) != value for key, value in labels.items())):
            raise RuntimeError('exact container ownership name/labels/ID changed')
        return actual

    observed = None
    pending = None
    try:
        stop_launcher()
        container = ownership['container']
        if container is None and launch is not None:
            if launch['launcher'] != ownership:
                raise RuntimeError('pending launch differs from exact retained launcher')
            named = status([podman, 'container', 'exists', launch['name']])
            if named == 1 and launch['mode'] == 'in_job':
                filters = [arg for key, value in sorted(launch['labels'].items())
                           for arg in ('--filter', f'label={key}={value}')]
                if not filters:
                    raise RuntimeError('pending cleanup requires retained ownership labels')
                remaining_ids = capture([podman, 'ps', '--all', '--no-trunc', *filters, '--format', '{{.ID}}']).decode().split()
                pending = {'intent': launch, 'name_exists_exit': named, 'sweep_exit': 0, 'remaining_ids': remaining_ids,
                           'started_monotonic_ns': started_ns, 'finished_monotonic_ns': time.monotonic_ns()}
            elif named == 0:
                actual = inspect(launch['name'], launch['name'], launch['labels'])
                scope = (scope_identity(actual['Pid'], systemctl=systemctl, timeout=remaining())
                         if launch['mode'] == 'own_scope' else None)
                container = {'id': actual['Id'], 'name': launch['name'], 'labels': launch['labels'],
                             'mode': launch['mode'], 'scope': scope}
                ownership = {**ownership, 'container': container}
            else:
                raise RuntimeError('unbound launch identity unavailable; cleanup remains unknown')
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
                _scope_observation(scope, systemctl=systemctl, cgroup_root=cgroup_root, timeout=remaining())
            removed = status([podman, 'rm', '--force', '--ignore', identifier])
            absent = status([podman, 'container', 'exists', identifier])
            name_absent = status([podman, 'container', 'exists', name])
            remaining_ids = capture(sweep_command).decode().split()
            observed = {'id': identifier, 'name': name, 'name_exists_exit': name_absent,
                        'remove_exit': removed, 'exists_exit': absent, 'sweep_ids': sweep,
                        'sweep_exit': 0, 'remaining_ids': remaining_ids,
                        'scope': _scope_observation(scope, systemctl=systemctl, cgroup_root=cgroup_root, timeout=remaining())
                        if scope is not None else None}
    except subprocess.TimeoutExpired:
        # Partial observations cannot certify container cleanup; retain launcher facts.
        observed = None
        pending = None
    finally:
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
            'process_group_probe_errno': group_errno, 'container': observed, 'pending_launch': pending}


def cleanup_main():
    import argparse
    import sys

    parser = argparse.ArgumentParser(description='Independent exact-ownership runtime cleanup observations')
    parser.add_argument('--podman', required=True)
    parser.add_argument('--systemctl', required=True)
    args = parser.parse_args()
    check_rootless_directory(os.environ)
    raw = sys.stdin.buffer.read(1024 * 1024 + 1)
    if len(raw) > 1024 * 1024:
        raise ValueError('cleanup input exceeds byte bound')
    request = json.loads(raw)
    if request['schema'] == 'tl3.job_unit_absence_input.v1':
        print(json.dumps(job_unit_absence(request, systemctl=args.systemctl), sort_keys=True))
        return
    if request['schema'] == 'tl3.scratch_cleanup_input.v1':
        print(json.dumps(scratch_cleanup(request, systemctl=args.systemctl), sort_keys=True))
        return
    if request['schema'] != 'tl3.cleanup_input.v1':
        raise ValueError('unsupported Rust cleanup input')
    budget = request['budget_seconds']
    if type(budget) is not int or budget <= 0:
        raise ValueError('positive Rust cleanup budget required')
    evidence = cleanup_observations(request['ownership'], launch=request['launch'], podman=args.podman, systemctl=args.systemctl,
                                    timeout=budget)
    print(json.dumps(evidence, sort_keys=True))


def post_exit_launch_absence(evidence, references, exited_ns):
    """Read only exact Rust-accepted cleanup references; never infer process cessation."""
    import hashlib

    for reference in references:
        path = Path(reference['path'])
        if path.parent != Path(evidence.context['output_dir']) or reference['bytes'] > 1024 * 1024:
            continue
        with path.open('rb') as source:
            raw = source.read(1024 * 1024 + 1)
        if len(raw) != reference['bytes'] or hashlib.sha256(raw).hexdigest() != reference['sha256']:
            continue
        cleanup = json.loads(raw)
        pending = cleanup.get('pending_launch')
        if (cleanup['ownership'] == evidence.launch['launcher'] and cleanup['container'] is None
                and pending is not None and pending['intent'] == evidence.launch
                and pending['name_exists_exit'] == 1 and pending['sweep_exit'] == 0
                and not pending['remaining_ids'] and cleanup['process_group_probe_errno'] == 3
                and cleanup['launcher']['kind'] in ('absent', 'reaped')
                and pending.get('started_monotonic_ns', 0) > exited_ns
                and pending.get('finished_monotonic_ns', 0) >= pending['started_monotonic_ns']):
            return reference
    return None


def job_unit_absence(request, *, systemctl):
    """External, read-only observation. A present unit never proves no survivors."""
    import re
    import stat
    import time

    if set(request) != {'schema', 'run', 'owner', 'request_digest', 'host', 'nonce', 'budget_seconds'}:
        raise ValueError('invalid exact-unit input fields')
    run = request['run']
    instance = run['instance'].split('/')
    if (len(instance) != 2 or not re.fullmatch(r'[A-Za-z0-9_-]+', instance[0])
            or not re.fullmatch(r'[0-9a-f]{32}', instance[1])
            or run['backend'] != 'flux' or not run['uri'].startswith('local:///')
            or not re.fullmatch(r'f[1-9A-HJ-NP-Za-km-z]+', run['job'])
            or not re.fullmatch(r'[0-9a-f]{64}', request['request_digest'])
            or not re.fullmatch(r'[0-9a-f]{64}', request['nonce'])
            or set(request['owner']) != {'owner', 'correlation'}
            or any(not re.fullmatch(r'[A-Za-z0-9_-]+', value) for value in request['owner'].values())
            or request['host'] != 'dl380g10'
            or type(request['budget_seconds']) is not int or not 0 < request['budget_seconds'] <= 10):
        raise ValueError('unqualified exact-unit identity')
    started = time.monotonic_ns()
    deadline = time.monotonic() + request['budget_seconds']


    path = '/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/flux.slice/flux-jobs.slice/' + f"shell-{run['job']}:sys.service"
    control_path = path.rsplit('/', 2)[0]
    result = {'schema': 'tl3.job_unit_absence.v1', 'run': run, 'owner': request['owner'],
              'request_digest': request['request_digest'], 'nonce': request['nonce'],
              'host': os.uname().nodename, 'path': path, 'started_monotonic_ns': started,
              'generation_before': _external_generation(systemctl, instance, deadline), 'observation': {'kind': 'unknown'},
              'control': {'path': control_path, 'observation': {'kind': 'unknown'}}}
    if (result['host'] == request['host'] and result['generation_before']['exit_code'] == 0
            and result['generation_before']['stdout'].strip() == instance[1]):
        try:
            control = os.stat(control_path, follow_symlinks=False)
        except FileNotFoundError as error:
            result['control']['observation'] = {'kind': 'absent', 'errno': error.errno}
        except OSError as error:
            result['control']['observation'] = {'kind': 'unknown', 'errno': error.errno}
        else:
            result['control']['observation'] = {'kind': 'directory' if stat.S_ISDIR(control.st_mode) else 'not_directory'}
    if result['control']['observation']['kind'] == 'directory':
        try:
            # The authorized hierarchy control precedes this exact job stat. No traversal.
            os.stat(path, follow_symlinks=False)
        except FileNotFoundError as error:
            result['observation'] = {'kind': 'absent', 'errno': error.errno}
        except OSError as error:
            result['observation'] = {'kind': 'unknown', 'errno': error.errno}
        else:
            result['observation'] = {'kind': 'present'}
    result['generation_after'] = _external_generation(systemctl, instance, deadline)
    result['finished_monotonic_ns'] = time.monotonic_ns()
    return result


def _external_generation(systemctl, instance, deadline):
    import selectors
    import subprocess
    import time
    from .redaction import Redactor

    process = subprocess.Popen([systemctl, '--user', 'show', f'flux-vanilla-{instance[0]}.service',
                                '-p', 'InvocationID', '--value'], stdin=subprocess.DEVNULL,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
    streams = {process.stdout: bytearray(), process.stderr: bytearray()}
    try:
        with selectors.DefaultSelector() as ready:
            for pipe in streams:
                ready.register(pipe, selectors.EVENT_READ)
            while ready.get_map():
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise RuntimeError('generation observation timed out')
                for key, _ in ready.select(remaining):
                    chunk = os.read(key.fd, 4096)
                    streams[key.fileobj].extend(chunk)
                    if len(streams[key.fileobj]) > 16384:
                        raise RuntimeError('generation observation exceeds byte bound')
                    if not chunk:
                        ready.unregister(key.fileobj)
        code = process.wait(timeout=max(0.001, deadline - time.monotonic()))
        return {'exit_code': code, 'stdout': streams[process.stdout].decode(errors='replace'),
                'stderr': Redactor(None).text(streams[process.stderr].decode(errors='replace'), limit=16384)}
    finally:
        import signal
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait(timeout=1)
        process.stdout.close()
        process.stderr.close()


def scratch_cleanup(request, *, systemctl):
    """Post-Clean exact-root check; only the Rust caller grants reclamation."""
    import hashlib
    import re

    if os.getuid() != 1000:
        raise ValueError('scratch cleanup requires executing uid1000')
    expected = {'schema', 'run', 'owner', 'request_digest', 'host', 'nonce', 'budget_seconds',
                'reclamation'}
    if set(request) != expected or request['schema'] != 'tl3.scratch_cleanup_input.v1':
        raise ValueError('invalid scratch input fields')
    run, owner = request['run'], request['owner']
    instance = run['instance'].split('/')
    if (len(instance) != 2 or not re.fullmatch(r'[A-Za-z0-9_-]+', instance[0])
            or not re.fullmatch(r'[0-9a-f]{32}', instance[1])
            or run['backend'] != 'flux' or not run['uri'].startswith('local:///')
            or not re.fullmatch(r'f[1-9A-HJ-NP-Za-km-z]+', run['job'])
            or not re.fullmatch(r'[0-9a-f]{64}', request['request_digest'])
            or not re.fullmatch(r'[0-9a-f]{64}', request['nonce'])
            or set(owner) != {'owner', 'correlation'}
            or any(not re.fullmatch(r'[A-Za-z0-9_-]+', value) for value in owner.values())
            or not re.fullmatch(r'[A-Za-z0-9_.-]+', request['host'])
            or type(request['budget_seconds']) is not int or not 0 < request['budget_seconds'] <= 10
            or request['reclamation'] not in (None, 'external scratch reclamation',
                                             'external scratch reclamation after validated launch failure')):
        raise ValueError('unqualified exact scratch identity')
    output = Path('/mnt/shared/flux-out/tl3') / owner['correlation']
    output_fd = os.open(output, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    def retained(name):
        fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW, dir_fd=output_fd)
        with os.fdopen(fd, 'rb') as source:
            info = os.fstat(source.fileno())
            if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_size > 1024 * 1024:
                raise ValueError('unsafe retained scratch authority')
            return source.read(1024 * 1024 + 1)
    try:
        if os.fstat(output_fd).st_uid != os.getuid():
            raise ValueError('foreign retained scratch output')
        raw = retained('request.json')
        intent = json.loads(retained('intent.json'))
        original = json.loads(retained('scratch-absence-' + request['nonce'] + '.input.json'))
    finally:
        os.close(output_fd)
    if (hashlib.sha256(raw).hexdigest() != request['request_digest'] or original != request
            or intent['request_digest'] != request['request_digest']
            or intent['owner'] != owner['owner'] or intent['correlation'] != owner['correlation']
            or intent['instance'] != run['instance'] or intent['uri'] != run['uri']
            or intent['backend'] != run['backend'] or intent['output_dir'] != str(output)):
        raise ValueError('scratch input differs from retained Rust authority')
    approved_text = json.loads(raw)['scratch_root']
    approved = Path(approved_text)
    if (not approved.is_absolute() or str(approved) != approved_text or '..' in approved.parts
            or len(approved.parts) < 3 or approved == Path('/mnt/shared') or Path('/mnt/shared') in approved.parents):
        raise ValueError('unqualified retained scratch root')
    return _scratch_cleanup_bound(request, instance, approved, systemctl=systemctl)


def _scratch_cleanup_bound(request, instance, approved, *, systemctl):
    import time

    if os.getuid() != 1000:
        raise ValueError('scratch cleanup requires executing uid1000')
    run, owner = request['run'], request['owner']
    started = time.monotonic_ns()
    deadline = time.monotonic() + request['budget_seconds']
    root = approved / run['job']
    result = {key: request[key] for key in ('run', 'owner', 'request_digest', 'host', 'nonce')}
    result.update(schema='tl3.scratch_cleanup.v1', path=str(root), uid=os.getuid(),
                  started_monotonic_ns=started, observation={'kind': 'unknown'},
                  attribution=None, manifest=None)
    result['host'] = os.uname().nodename
    result['generation_before'] = _external_generation(systemctl, instance, deadline)
    descriptors = []
    output = Path('/mnt/shared/flux-out/tl3') / owner['correlation']
    manifest_path = output / 'scratch-reclamation.manifest.json'
    completion_path = output / 'scratch-reclamation.complete.json'

    def bounded():
        if time.monotonic() >= deadline:
            raise RuntimeError('scratch observation budget exhausted')

    def identity(info):
        return {'dev': info.st_dev, 'ino': info.st_ino, 'uid': info.st_uid,
                'mode': info.st_mode, 'size': info.st_size, 'mtime_ns': info.st_mtime_ns,
                'ctime_ns': info.st_ctime_ns}

    def directory(name, parent=None):
        bounded()
        fd = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=parent)
        descriptors.append(fd)
        return fd

    def own(info, device, *, private=False):
        if (info.st_uid != os.getuid() or info.st_dev != device
                or not (stat.S_ISDIR(info.st_mode) or stat.S_ISREG(info.st_mode))
                or (private and info.st_mode & 0o022)):
            raise RuntimeError('scratch uid, device, type or writable access is unsafe')

    def persist(path, value):
        fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            parent_info = os.fstat(fd)
            if parent_info.st_uid != os.getuid():
                raise RuntimeError('retained scratch manifest directory has another owner')
            target = os.open(path.name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=fd)
            with os.fdopen(target, 'w') as retained:
                json.dump(value, retained, sort_keys=True)
                retained.flush()
                os.fsync(retained.fileno())
            os.fsync(fd)
        finally:
            os.close(fd)

    try:
        if (result['host'] != request['host'] or result['generation_before']['exit_code'] != 0
                or result['generation_before']['stdout'].strip() != instance[1]):
            raise RuntimeError('scratch host or generation changed')
        if manifest_path.exists() and not completion_path.exists():
            raise RuntimeError('prior scratch reclamation incomplete; no retry')
        parent = directory('/')
        for part in approved.parts[1:]:
            parent = directory(part, parent)
        parent_info = os.fstat(parent)
        own(parent_info, parent_info.st_dev, private=True)
        try:
            info = os.stat(run['job'], dir_fd=parent, follow_symlinks=False)
        except FileNotFoundError:
            result['observation'] = {'kind': 'absent', 'errno': 2}
        else:
            own(info, parent_info.st_dev, private=True)
            if not stat.S_ISDIR(info.st_mode):
                raise RuntimeError('scratch root is not an owned directory')
            result['observation'] = {'kind': 'present'}
            if request['reclamation'] is not None:
                root_fd = directory(run['job'], parent)
                if identity(os.fstat(root_fd)) != identity(info):
                    raise RuntimeError('scratch root changed during open')
                entries, total = [], 0

                def inventory(fd, prefix):
                    nonlocal total
                    bounded()
                    if len(prefix) > 32:
                        raise RuntimeError('scratch inventory depth bound exceeded')
                    # Bound enumeration itself, including a huge single directory.
                    with os.scandir(fd) as children:
                        names = []
                        for child in children:
                            bounded()
                            if len(entries) + len(names) >= 4096:
                                raise RuntimeError('scratch inventory entry bound exceeded')
                            names.append(child.name)
                    for name in sorted(names):
                        bounded()
                        current = os.stat(name, dir_fd=fd, follow_symlinks=False)
                        own(current, info.st_dev)
                        total += current.st_size if stat.S_ISREG(current.st_mode) else 0
                        if total > 2 * 1024 ** 3 or len(entries) >= 4096:
                            raise RuntimeError('scratch inventory byte or entry bound exceeded')
                        path = prefix + [name]
                        entries.append({'parts': path, **identity(current)})
                        if stat.S_ISDIR(current.st_mode):
                            child_fd = directory(name, fd)
                            if identity(os.fstat(child_fd)) != identity(current):
                                raise RuntimeError('scratch child changed during open')
                            inventory(child_fd, path)
                            os.close(descriptors.pop())

                inventory(root_fd, [])
                manifest = {key: result[key] for key in ('run', 'owner', 'request_digest', 'path', 'uid')}
                manifest.update(schema='tl3.scratch_manifest.v1', root=identity(info), entries=entries,
                                total_bytes=total, attribution=request['reclamation'])
                if len(json.dumps(manifest).encode()) > 1024 * 1024:
                    raise RuntimeError('scratch manifest byte bound exceeded')
                # An existing attempt is never retried, even after a partial failed removal.
                persist(manifest_path, manifest)
                result['manifest'] = str(manifest_path)
                # A fresh generation guard must also precede the first mutation.
                result['generation_pre_removal'] = _external_generation(systemctl, instance, deadline)
                if (result['generation_pre_removal']['exit_code'] != 0
                        or result['generation_pre_removal']['stdout'].strip() != instance[1]):
                    raise RuntimeError('generation changed before scratch removal')
                by_path = {tuple(entry['parts']): entry for entry in entries}

                def remove(fd, prefix):
                    bounded()
                    expected_names = {parts[-1] for parts in by_path if list(parts[:-1]) == prefix}
                    current_names = set()
                    with os.scandir(fd) as children:
                        for child in children:
                            bounded()
                            if child.name not in expected_names or child.name in current_names:
                                raise RuntimeError('scratch entries changed before removal')
                            current_names.add(child.name)
                    if current_names != expected_names:
                        raise RuntimeError('scratch entries changed before removal')
                    for name in sorted(expected_names):
                        bounded()
                        parts = prefix + [name]
                        prior = by_path[tuple(parts)]
                        current = os.stat(name, dir_fd=fd, follow_symlinks=False)
                        own(current, info.st_dev)
                        if identity(current) != {key: prior[key] for key in identity(current)}:
                            raise RuntimeError('scratch identity changed before removal')
                        if stat.S_ISDIR(current.st_mode):
                            child_fd = directory(name, fd)
                            if identity(os.fstat(child_fd)) != identity(current):
                                raise RuntimeError('scratch child changed before removal')
                            remove(child_fd, parts)
                            os.close(descriptors.pop())
                            again = os.stat(name, dir_fd=fd, follow_symlinks=False)
                            if (again.st_dev, again.st_ino, again.st_uid) != (current.st_dev, current.st_ino, current.st_uid):
                                raise RuntimeError('scratch directory replaced before unlink')
                            os.rmdir(name, dir_fd=fd)
                        else:
                            os.unlink(name, dir_fd=fd)
                # Recheck root path and parent identity before traversing retained fds.
                if identity(os.stat(run['job'], dir_fd=parent, follow_symlinks=False)) != identity(info):
                    raise RuntimeError('scratch root replaced before removal')
                remove(root_fd, [])
                current = os.stat(run['job'], dir_fd=parent, follow_symlinks=False)
                own(current, info.st_dev)
                if (current.st_dev, current.st_ino) != (info.st_dev, info.st_ino):
                    raise RuntimeError('scratch root replaced before rmdir')
                os.rmdir(run['job'], dir_fd=parent)
                os.fsync(parent)
                try:
                    os.stat(run['job'], dir_fd=parent, follow_symlinks=False)
                except FileNotFoundError:
                    result['observation'] = {'kind': 'absent', 'errno': 2}
                    result['attribution'] = request['reclamation']
                else:
                    raise RuntimeError('scratch root remains after removal')
    except (OSError, RuntimeError) as error:
        result['observation'] = {'kind': 'unknown', 'reason': str(error)[:256]}
    finally:
        for fd in reversed(descriptors):
            os.close(fd)
    result['generation_after'] = _external_generation(systemctl, instance, deadline)
    result['finished_monotonic_ns'] = time.monotonic_ns()
    if result['attribution'] is not None and result['observation']['kind'] == 'absent':
        if (result['generation_after']['exit_code'] == 0
                and result['generation_after']['stdout'].strip() == instance[1]):
            persist(completion_path, result)
    return result
