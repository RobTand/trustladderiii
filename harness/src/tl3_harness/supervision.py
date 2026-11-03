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
from .node_ids import selects
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
        return (f'Test execution timeout in {self.phase}; no assertion verdict is available.'
                if self.outcome == ExecutionOutcome.TEST_TIMEOUT else
                f'Test execution {self.outcome} in {self.phase}: {self.termination_reason}. '
                'No test verdict is available; inspect this action, not fleet availability.')


class Supervised:
    """Only runner protocol events establish phase, never checkout stdout or elapsed PB time."""

    def __init__(self, command, selection, tests):
        quiet = json.loads(os.environ.get('PRISMABUILD_ACTION_PROGRESS_ALLOWANCES', '{}')).get('tests', 120)
        if type(quiet) not in (int, float) or not 0 < quiet <= 86400:
            raise ValueError('invalid test progress allowance')
        # Leave the remaining allowance for termination, diagnostics and publication.
        self.allowance = quiet * 0.7
        self.cleanup_seconds = min(2, quiet * 0.1)
        self.tests = tests
        self.started = False
        self.test = None
        self.finished = False
        self.exit_code = None
        self.container = None
        command = list(command) + ['--supervised']
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
        setup = not self.started and self.exit_code in (125, 126, 127)
        return ExecutionFailure(ExecutionOutcome.SETUP_FAILURE if setup else ExecutionOutcome.UNKNOWN_INCOMPLETE,
                                'container_start' if setup else 'unknown', 'incomplete_report',
                                self.test is not None, self.test)

    def records(self):
        deadline = time.monotonic() + self.allowance
        buffer = b''
        total = 0
        with selectors.DefaultSelector() as ready:
            ready.register(self.child.stdout, selectors.EVENT_READ)
            while True:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    owned = self.test is not None
                    phase = ('cleanup' if self.finished else 'test_run') if owned else (
                        'pre_test' if self.started else 'container_start')
                    raise ExecutionFailure(ExecutionOutcome.TEST_TIMEOUT if owned else ExecutionOutcome.SETUP_FAILURE,
                                           phase, 'no_progress', owned, self.test)
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
                    if not self.started:
                        if record != {'event': 'start', 'schema': 1}:
                            raise ValueError('missing report header')
                        self.started = True
                    if record.get('event') == 'test-start':
                        test = record.get('test')
                        if (set(record) != {'event', 'test'} or not isinstance(test, str)
                                or self.finished or not test or len(test) > MAX_REPORT_LINE_BYTES
                                or self.tests and not any(selects(selector, test) for selector in self.tests)):
                            raise ValueError('invalid test start')
                        self.test = test
                        # A test starting is evidence, not a PB progress unit. Only completed
                        # results advance PB's existing progress contract.
                        continue
                    if record.get('event') == 'finish':
                        self.finished = True
                    yield line + b'\n'
                    if record.get('event') == 'result' or (
                            record.get('event') == 'phase' and record.get('when') == 'teardown'):
                        deadline = time.monotonic() + self.allowance
                if len(buffer) > MAX_REPORT_LINE_BYTES:
                    raise ValueError('oversized report line')
            try:
                self.exit_code = self.child.wait(timeout=max(0.001, deadline - time.monotonic()))
            except subprocess.TimeoutExpired as error:
                owned = self.test is not None
                raise ExecutionFailure(ExecutionOutcome.TEST_TIMEOUT if owned else ExecutionOutcome.SETUP_FAILURE,
                                       'cleanup' if owned else 'pre_test', 'no_progress', owned, self.test) from error

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
