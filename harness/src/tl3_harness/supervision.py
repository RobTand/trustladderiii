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

    def as_json(self):
        result = {'outcome': self.outcome, 'phase': self.phase,
                  'termination_reason': self.termination_reason, 'test_started': self.test_started}
        if self.test is not None:
            result['test'] = self.test[:2048]
        return result

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
        command = list(command)
        if command[:2] == ['docker', 'run']:
            self.container = 'tl3-test-' + uuid.uuid4().hex
            command[2:2] = ['--name', self.container]
        self.command, self.selection = command, selection

    def __enter__(self):
        try:
            self.child = subprocess.Popen(self.command, stdin=self.selection, stdout=subprocess.PIPE,
                                          stderr=subprocess.DEVNULL, start_new_session=True)
        except OSError as error:
            raise ExecutionFailure(ExecutionOutcome.SETUP_FAILURE, 'container_start', 'runtime_unavailable') from error
        return self

    def incomplete(self):
        setup = not self.report_started and self.exit_code in (125, 126, 127)
        return ExecutionFailure(ExecutionOutcome.SETUP_FAILURE if setup else ExecutionOutcome.UNKNOWN_INCOMPLETE,
                                'container_start' if setup else 'unknown', 'incomplete_report')

    def records(self):
        deadline = time.monotonic() + self.allowance
        buffer = b''
        total = 0
        with selectors.DefaultSelector() as ready:
            ready.register(self.child.stdout, selectors.EVENT_READ)
            while True:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise ExecutionFailure(ExecutionOutcome.UNKNOWN_INCOMPLETE, 'unknown', 'no_progress')
                if not ready.select(remaining):
                    continue
                chunk = os.read(self.child.stdout.fileno(), 65536)
                if not chunk:
                    if buffer:
                        raise ValueError('incomplete report line')
                    break
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
                raise ExecutionFailure(ExecutionOutcome.UNKNOWN_INCOMPLETE, 'unknown', 'no_progress') from error

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
        if self.container is not None:
            try:
                subprocess.run(['docker', 'rm', '--force', self.container], stdin=subprocess.DEVNULL,
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                               timeout=self.cleanup_seconds, check=False)
            except (OSError, subprocess.TimeoutExpired):
                # PB still owns and verifies its container scope before pbwait --until cleaned.
                pass
