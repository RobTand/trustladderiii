"""Bound report reads and process cleanup outside the served repository."""
from dataclasses import dataclass
from enum import StrEnum
import json
import os
import selectors
import signal
import subprocess
import time
import uuid

from .limits import MAX_REPORT_BYTES, MAX_REPORT_LINE_BYTES
from .report import decode
from .redaction import Redactor


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
    TEST_TIMEOUT = 'test_timeout'
    SETUP_FAILURE = 'setup_failure'
    TRANSPORT_FAILURE = 'transport_failure'
    UNKNOWN_INCOMPLETE = 'unknown_incomplete'


@dataclass(frozen=True)
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

    def __init__(self, command, selection, tests):
        quiet = json.loads(os.environ.get('PRISMABUILD_ACTION_PROGRESS_ALLOWANCES', '{}')).get('tests', 120)
        if type(quiet) not in (int, float) or not 0 < quiet <= 86400:
            raise ValueError('invalid test progress allowance')
        # Leave the remaining allowance for termination, diagnostics and publication.
        self.allowance = quiet * 0.7
        self.cleanup_seconds = min(2, quiet * 0.1)
        self.report_started = False
        self.exit_code = None
        self.container = None
        self.stderr_tail = b''
        self.stderr_diagnostic = ''
        self.stderr_withheld = False
        command = list(command)
        if command[:2] == ['docker', 'run']:
            self.container = 'tl3-test-' + uuid.uuid4().hex
            command[2:2] = ['--name', self.container]
        self.command, self.selection = command, selection

    def __enter__(self):
        try:
            self.child = subprocess.Popen(self.command, stdin=self.selection, stdout=subprocess.PIPE,
                                          stderr=subprocess.PIPE, start_new_session=True)
        except OSError as error:
            raise ExecutionFailure(ExecutionOutcome.SETUP_FAILURE, 'container_start', 'runtime_unavailable') from error
        return self

    def incomplete(self):
        setup = not self.report_started and self.exit_code in (125, 126, 127)
        return self.failure(ExecutionOutcome.SETUP_FAILURE if setup else ExecutionOutcome.UNKNOWN_INCOMPLETE,
                            'container_start' if setup else 'unknown', 'incomplete_report')

    def failure(self, outcome, phase, reason):
        return ExecutionFailure(outcome, phase, reason, stderr_tail=self.stderr_diagnostic)

    def capture_stderr(self, chunk):
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
        deadline = time.monotonic() + self.allowance
        buffer = b''
        total = 0
        with selectors.DefaultSelector() as ready:
            ready.register(self.child.stdout, selectors.EVENT_READ)
            ready.register(self.child.stderr, selectors.EVENT_READ)
            while ready.get_map():
                remaining = deadline - time.monotonic()
                if remaining <= 0:
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
                if not any(key.fileobj is self.child.stdout for key, _ in events):
                    continue
                chunk = os.read(self.child.stdout.fileno(), 65536)
                if not chunk:
                    ready.unregister(self.child.stdout)
                    if buffer:
                        raise ValueError('incomplete report line')
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
                        deadline = time.monotonic() + self.allowance
                if len(buffer) > MAX_REPORT_LINE_BYTES:
                    raise ValueError('oversized report line')
            try:
                self.exit_code = self.child.wait(timeout=max(0.001, deadline - time.monotonic()))
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
        self.child.stdout.close()
        self.child.stderr.close()
        if self.container is not None:
            try:
                subprocess.run(['docker', 'rm', '--force', self.container], stdin=subprocess.DEVNULL,
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                               timeout=self.cleanup_seconds, check=False)
            except (OSError, subprocess.TimeoutExpired):
                # PB still owns and verifies its container scope before pbwait --until cleaned.
                pass
