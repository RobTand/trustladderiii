"""Bound report reads and process cleanup outside the served repository."""
from dataclasses import dataclass
from enum import StrEnum
import json
import os
from pathlib import Path
import selectors
import signal
import sys
import subprocess
import time
import uuid

from .limits import MAX_REPORT_BYTES, MAX_REPORT_LINE_BYTES
from .report import decode
from .redaction import Redactor
from . import runtime


MAX_STDERR_BYTES = 16_384
STDERR_WITHHELD = 'stderr withheld: redaction context lost'
MAX_STDERR_CHARS = 1024
MAX_STDERR_LINE_CHARS = 512
FAILURE_PREFIX = 'TL3_EXECUTION_FAILURE '
# Share the existing result envelope's headroom across all failed trees in one action.
MAX_STDERR_LOG_BYTES = 16_384
_stderr_log_bytes = 0
_redactor = Redactor(None)


def safe_stderr(text):
    # Match on all retained context, before sanitizing or removing any characters.
    text = _redactor.window(text, 0, len(text))
    text = ''.join(c if c == '\n' or c.isprintable() else '?' for c in text)
    return '\n'.join(line[-MAX_STDERR_LINE_CHARS:] for line in text.split('\n'))[-MAX_STDERR_CHARS:]


def model_result(value):
    """Operator diagnostics never cross the model tool boundary, including nested runs."""
    if isinstance(value, dict):
        return {key: model_result(item) for key, item in value.items() if key != 'stderr_tail'}
    if isinstance(value, list):
        return [model_result(item) for item in value]
    return value


class ExecutionOutcome(StrEnum):
    POST_BEGIN_LAUNCH_FAILURE_CANDIDATE = 'post_begin_launch_failure_candidate'
    TEST_TIMEOUT = 'test_timeout'
    SETUP_FAILURE = 'setup_failure'
    TRANSPORT_FAILURE = 'transport_failure'
    UNKNOWN_INCOMPLETE = 'unknown_incomplete'


@dataclass
class ExecutionFailure(Exception):
    outcome: ExecutionOutcome
    phase: str
    termination_reason: str
    test_started: bool = False
    test: str | None = None
    stderr_tail: str = ''

    def as_json(self):
        result = {'outcome': self.outcome, 'phase': self.phase,
                  'termination_reason': self.termination_reason, 'test_started': self.test_started}
        if self.test is not None:
            result['test'] = self.test[:2048]
        return result

    def operator_json(self):
        return {**self.as_json(), 'stderr_tail': safe_stderr(self.stderr_tail)}

    def log(self):
        global _stderr_log_bytes
        if self.stderr_tail:
            line = FAILURE_PREFIX + json.dumps(self.operator_json()) + '\n'
            size = len(line.encode())
            if _stderr_log_bytes + size <= MAX_STDERR_LOG_BYTES:
                _stderr_log_bytes += size
                print(line, end='', flush=True)

    def message(self):
        if self.outcome == ExecutionOutcome.TEST_TIMEOUT:
            return f'Test execution timeout in {self.phase}; no assertion verdict is available.'
        if self.outcome == ExecutionOutcome.UNKNOWN_INCOMPLETE and self.termination_reason == 'no_progress':
            return ('Test execution no-progress timeout; test start and runner exit are unproven. '
                    'Ownership is unknown. No test verdict is available; inspect this action, not fleet availability.')
        return (f'Test execution {self.outcome} in {self.phase}: {self.termination_reason}. '
                'No test verdict is available; inspect this action, not fleet availability.')


class Supervised:
    """Bound the launcher without treating in-process reports as lifecycle evidence.

    Repo hooks can reach the reporter and its descriptor. Until a protected
    external start/exit boundary exists, every no-progress timeout is unknown.
    """

    def __init__(self, command, selection, tests, *, deadline_seconds=None):
        quiet = runtime.no_progress_seconds()
        if quiet is not None and (type(quiet) not in (int, float) or not 0 < quiet <= 86400):
            raise ValueError('invalid test progress allowance')
        # Raw mode (generators) has one fixed deadline and yields stdout bytes, not report records.
        self.raw = deadline_seconds is not None
        if self.raw and (type(deadline_seconds) not in (int, float) or not 0 < deadline_seconds <= 86400):
            raise ValueError('invalid execution deadline')
        # Leave the remaining allowance for termination, diagnostics and publication.
        self.allowance = deadline_seconds if self.raw else quiet * 0.7 if quiet is not None else None
        self.cleanup_seconds = min(2, quiet * 0.1) if quiet is not None else 2
        self.gate_opened = False
        self.report_started = False
        self.exit_code = None
        self.exit_recorded = False
        self.exit_observed_ns = None
        self.command_released = False
        self.container = None
        self.runtime = None
        self.cleanup_confirmed = False
        self.evidence = None
        self.process_handles = []
        self.evidence_buffer = b''
        self.pending_input = None
        self.stderr_tail = b''
        self.stderr_diagnostic = ''
        self.stderr_withheld = False
        command = list(command)
        run_index = 2 if command[1:2] == ['--cgroup-manager=systemd'] else 1
        if len(command) > run_index and Path(command[0]).name == 'podman' and command[run_index] == 'run':
            self.runtime = command[0]
            self.container = 'tl3-test-' + uuid.uuid4().hex
            command[run_index + 1:run_index + 1] = ['--name', self.container]
        self.command, self.selection = command, selection

    def __enter__(self):
        phase = 'host_evidence'
        # Flux is the only executor (PB-REMOVAL): every container launch is durable.
        if self.runtime:
            try:
                self.evidence = runtime.evidence_sink(self.container)
                phase = 'allocation'
                runtime.prepare_allocation(self.evidence)
            except (OSError, RuntimeError, ValueError, StopIteration, KeyError, TypeError) as error:
                # No command bytes have reached any runtime client.
                raise self.prelaunch_failure(error, phase, 'prelaunch_refused') from error
        durable = self.evidence is not None
        launch_command = ([sys.executable, '-I', '-B', str(Path(runtime.__file__).with_name('runtime_launcher.py'))]
                          if durable else self.command)
        try:
            self.child = subprocess.Popen(launch_command, stdin=subprocess.PIPE if self.evidence else self.selection,
                                          stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
        except OSError as error:
            raise self.prelaunch_failure(error, 'container_start', 'runtime_unavailable') from error
        if durable:
            try:
                intent = runtime.begin_container_launch(self.child, self.evidence)
                self.container = intent['name']
                run_index = self.command.index('run')
                name_index = self.command.index('--name')
                self.command[name_index + 1] = self.container
                self.command[run_index + 1:run_index + 1] = [
                    arg for key, value in intent['labels'].items() for arg in ('--label', key + '=' + value)]
                runtime.container_pending(self.container)
                # Begin returned only after Rust fsynced the intent and debt. No retry on failure.
                self.child.stdin.write(json.dumps(self.command).encode() + b'\n')
                self.child.stdin.flush()
                self.command_released = True
            except BaseException as error:
                # __exit__ is not called when __enter__ fails. Stop only this owned gated launcher.
                try:
                    os.killpg(self.child.pid, signal.SIGKILL)
                    self.child.wait(timeout=self.cleanup_seconds)
                except ProcessLookupError:
                    self.child.wait(timeout=self.cleanup_seconds)
                finally:
                    for pipe in (self.child.stdin, self.child.stdout, self.child.stderr):
                        pipe.close()
                self.capture_diagnostic(str(error).encode(errors='replace'))
                self.retain_exit()
                if not self.evidence.launch_requested:
                    if isinstance(error, Exception):
                        raise self.prelaunch_failure(error, 'container_start', 'launch_refused',
                                                     launcher_started=True) from error
                else:
                    self.capture_diagnostic(str(error).encode(errors='replace'))
                    try:
                        self.evidence.record('launch_uncertain', {'launcher_reaped': self.child.returncode is not None,
                                                                  'cause': safe_stderr(str(error))})
                    except (OSError, RuntimeError, ValueError, KeyError, TypeError) as secondary:
                        self.capture_diagnostic(('\nlaunch evidence: ' + str(secondary)).encode(errors='replace'))
                    if isinstance(error, Exception):
                        raise self.failure(ExecutionOutcome.UNKNOWN_INCOMPLETE, 'container_start', 'launch_refused') from error
                raise

        return self

    def prelaunch_failure(self, error, phase, reason, *, launcher_started=False):
        self.capture_diagnostic(str(error).encode(errors='replace'))
        try:
            runtime.container_stopped(self.container, self.command)
            self.cleanup_confirmed = True
        except (OSError, RuntimeError, ValueError, KeyError, TypeError) as secondary:
            runtime.container_pending(self.container)
            self.capture_diagnostic(('\nrefusal scratch cleanup: ' + str(secondary)).encode(errors='replace'))
        if self.evidence is not None:
            try:
                self.evidence.record('prelaunch_refusal', {'command_released': False,
                    'launcher_started': launcher_started, 'cause': safe_stderr(str(error))})
            except (OSError, RuntimeError, ValueError, KeyError, TypeError) as secondary:
                self.capture_diagnostic(('\nrefusal evidence: ' + str(secondary)).encode(errors='replace'))
        return self.failure(ExecutionOutcome.SETUP_FAILURE, phase, reason)

    def incomplete(self):
        setup = not self.report_started and self.exit_code in (125, 126, 127)
        return self.failure(ExecutionOutcome.SETUP_FAILURE if setup else ExecutionOutcome.UNKNOWN_INCOMPLETE,
                            'container_start' if setup else 'unknown', 'incomplete_report')

    def retain_exit(self):
        if self.evidence is None or self.exit_recorded:
            return
        self.exit_observed_ns = time.monotonic_ns()
        # wait()/poll() supply the actual launcher status, never an inferred stderr code.
        self.evidence.record('launcher_exit', {
            'execution': self.evidence.container, 'exit_code': self.child.returncode,
            'command_released': self.command_released,
            'natural_exit': self.exit_code is not None and self.exit_code == self.child.returncode,
            'monotonic_ns': self.exit_observed_ns,
            'stderr_tail': self.stderr_diagnostic,
        })
        self.exit_recorded = True

    def failure(self, outcome, phase, reason):
        return ExecutionFailure(outcome, phase, reason, stderr_tail=self.stderr_diagnostic)

    def capture_stderr(self, chunk):
        if self.evidence is not None and not self.evidence.container_seen:
            self.evidence_buffer += chunk
            if len(self.evidence_buffer) > MAX_STDERR_BYTES:
                raise self.failure(ExecutionOutcome.UNKNOWN_INCOMPLETE, 'runtime', 'missing_runtime_evidence')
            while b'\n' in self.evidence_buffer:
                line, self.evidence_buffer = self.evidence_buffer.split(b'\n', 1)
                try:
                    observed = self.evidence.observe(line)
                except (ValueError, OSError) as error:
                    raise self.failure(ExecutionOutcome.UNKNOWN_INCOMPLETE, 'runtime', 'invalid_runtime_evidence') from error
                if not observed:
                    self.capture_diagnostic(line + b'\n')
                if observed:
                    try:
                        self.process_handles = runtime.capture_processes(self.runtime, self.container, self.evidence,
                                                                         self.allowance or 30)
                        runtime.validate_runtime_binding(self.evidence)
                    except (RuntimeError, OSError, ValueError, KeyError, TypeError, AttributeError,
                            subprocess.SubprocessError) as error:
                        self.capture_diagnostic(str(error).encode(errors='replace'))
                        raise self.failure(ExecutionOutcome.UNKNOWN_INCOMPLETE, 'runtime',
                                           'runtime_binding_unqualified') from error
                    self.pending_input = memoryview(b'\x01' + self.selection.read())
                    self.gate_opened = True
                    self.capture_diagnostic(self.evidence_buffer)
                    self.evidence_buffer = b''
                    break
            return
        self.capture_diagnostic(chunk)

    def capture_diagnostic(self, chunk):
        if self.stderr_withheld:
            return
        # #826: never forget matching context while publishing subsequent bytes.
        # Withhold on every cut rather than duplicate the shared policy's scopes.
        if len(self.stderr_tail) + len(chunk) > MAX_STDERR_BYTES:
            self.stderr_tail = b''
            self.stderr_withheld = True
            self.stderr_diagnostic = STDERR_WITHHELD
            return
        self.stderr_tail += chunk
        self.stderr_diagnostic = safe_stderr(self.stderr_tail.decode('utf-8', errors='replace'))

    def records(self):
        deadline = time.monotonic() + self.allowance if self.allowance is not None else None
        buffer = b''
        total = 0
        with selectors.DefaultSelector() as ready:
            ready.register(self.child.stdout, selectors.EVENT_READ)
            ready.register(self.child.stderr, selectors.EVENT_READ)
            while ready.get_map():
                remaining = deadline - time.monotonic() if deadline is not None else None
                if remaining is not None and remaining <= 0:
                    raise self.failure(ExecutionOutcome.UNKNOWN_INCOMPLETE, 'unknown', 'no_progress')
                events = ready.select(remaining)
                if not events:
                    continue
                # One bounded read per ready stream keeps stderr floods from starving stdout.
                for key, _ in events:
                    if key.fileobj is self.child.stderr:
                        chunk = os.read(key.fd, MAX_STDERR_BYTES)
                        if chunk:
                            self.capture_stderr(chunk)
                        else:
                            ready.unregister(key.fileobj)
                if self.pending_input is not None and self.child.stdin not in ready.get_map():
                    os.set_blocking(self.child.stdin.fileno(), False)
                    ready.register(self.child.stdin, selectors.EVENT_WRITE)
                for key, _ in events:
                    if key.fileobj is self.child.stdin:
                        try:
                            count = os.write(key.fd, self.pending_input[:65536])
                        except (BrokenPipeError, OSError) as error:
                            raise self.failure(ExecutionOutcome.UNKNOWN_INCOMPLETE, 'runtime', 'input_gate_lost') from error
                        self.pending_input = self.pending_input[count:]
                        if not self.pending_input:
                            ready.unregister(self.child.stdin)
                            self.child.stdin.close()
                            self.pending_input = None
                if not any(key.fileobj is self.child.stdout for key, _ in events):
                    continue
                chunk = os.read(self.child.stdout.fileno(), 65536)
                if not chunk:
                    ready.unregister(self.child.stdout)
                    if buffer:
                        raise ValueError('incomplete report line')
                    continue
                if self.raw:
                    # Before the gate only the runtime client runs; served code cannot speak yet.
                    if self.evidence is not None and not self.gate_opened:
                        raise self.failure(ExecutionOutcome.UNKNOWN_INCOMPLETE, 'runtime', 'output_before_gate')
                    yield chunk
                    continue
                buffer += chunk
                total += len(chunk)
                if total > MAX_REPORT_BYTES:
                    raise ValueError('oversized report')
                while b'\n' in buffer:
                    line, buffer = buffer.split(b'\n', 1)
                    if len(line) > MAX_REPORT_LINE_BYTES:
                        raise ValueError('oversized report line')
                    record = decode(line)
                    if not self.report_started:
                        if record != {'event': 'start', 'schema': 1}:
                            raise ValueError('missing report header')
                        self.report_started = True
                    if record.get('event') == 'test-start':
                        # Legacy or forged events are only untrusted process output.
                        # They prove neither test start nor runner liveness/exit.
                        continue
                    yield line + b'\n'
                    if record.get('event') == 'result' or (
                            record.get('event') == 'phase' and record.get('when') == 'teardown'):
                        deadline = time.monotonic() + self.allowance if self.allowance is not None else None
                if len(buffer) > MAX_REPORT_LINE_BYTES:
                    raise ValueError('oversized report line')
            try:
                self.exit_code = self.child.wait(timeout=max(0.001, deadline - time.monotonic()) if deadline is not None else None)
                self.capture_diagnostic(self.evidence_buffer)
                self.evidence_buffer = b''
                self.retain_exit()
                if self.evidence is not None and not self.evidence.container_seen:
                    raise self.failure(ExecutionOutcome.UNKNOWN_INCOMPLETE, 'runtime', 'missing_runtime_evidence')
            except subprocess.TimeoutExpired as error:
                # EOF and a report's finish record do not prove the runner exited:
                # the direct child may still be Docker's attach/wait transport.
                raise self.failure(ExecutionOutcome.UNKNOWN_INCOMPLETE, 'unknown', 'no_progress') from error

    def __exit__(self, *args):
        # Reap the whole group even if its launcher exited while a descendant retained pipes.
        try:
            os.killpg(self.child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        try:
            self.child.wait(timeout=self.cleanup_seconds)
        except subprocess.TimeoutExpired:
            pass
        self.retain_exit()
        if self.child.stdin is not None:
            self.child.stdin.close()
        self.child.stdout.close()
        self.child.stderr.close()
        if self.evidence is not None:
            context = self.evidence.context
            runtime_stopped = False
            try:
                result = runtime.backend_request({'op': 'cleanup_runtime', 'owner': context['owner'],
                                                  'correlation': context['correlation']})['cleanup']
                self.evidence.record('cleanup_result', result)
                runtime_stopped = result['stopped']
            except (RuntimeError, OSError, ValueError, KeyError, subprocess.SubprocessError):
                pass
            started = time.monotonic_ns()
            try:
                # Observe the captured identities even when Rust cleanup is unconfirmed.
                # A pending launch may have failed before any container was observed.
                processes_stopped = runtime.processes_stopped(self.process_handles, self.cleanup_seconds)
                self.evidence.record('container_processes_stopped', {
                    'confirmed': processes_stopped, 'captured': len(self.process_handles),
                    'started_monotonic_ns': started, 'finished_monotonic_ns': time.monotonic_ns(),
                })
                self.cleanup_confirmed = runtime_stopped and processes_stopped
                if (not self.evidence.container_seen and not self.process_handles
                        and self.command_released and self.exit_recorded
                        and self.exit_code is not None
                        and self.child.returncode is not None and self.child.returncode != 0):
                    references = result.get('evidence', []) if runtime_stopped else []
                    cleanup = runtime.post_exit_launch_absence(self.evidence, references, self.exit_observed_ns)
                    if cleanup is not None:
                        self.evidence.record('no_container_observed', {
                            'execution': self.evidence.container, 'cleanup': cleanup,
                            'pidfd_cessation': False,
                        })
                        # The job still contains this action. Keep scratch pending until
                        # external whole-unit evidence; this is only a local candidate.
                        raise self.failure(ExecutionOutcome.POST_BEGIN_LAUNCH_FAILURE_CANDIDATE,
                                           'container_start', 'nonzero_launcher_no_container_observed')
            except (RuntimeError, OSError, ValueError, KeyError):
                self.cleanup_confirmed = False
            finally:
                for handle in self.process_handles:
                    os.close(handle)
            if self.cleanup_confirmed:
                runtime.container_stopped(self.container, self.command)
                # The pre-begin execution token also held scratch pending lookup.
                runtime._uncertain_containers.discard(self.evidence.container)
            if not self.cleanup_confirmed:
                raise self.failure(ExecutionOutcome.UNKNOWN_INCOMPLETE, 'cleanup', 'container_cleanup_unconfirmed')
            return
