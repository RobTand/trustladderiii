"""#823: persistent old/new I/O oracle driven by controller-side proptest."""
import builtins
from copy import deepcopy
import hashlib
import io
import json
from pathlib import Path
import sys
import tempfile
import types

import pytest

from tl3_harness import execution, test_tool
from tl3_harness.supervision import ExecutionFailure, ExecutionOutcome
from tl3_workers import pb_tests


REFERENCE = Path(__file__).with_name('fixtures') / 'harness-extraction'
legacy_action = types.ModuleType('tl3_harness.legacy_test_tool')
legacy_action.__package__ = 'tl3_harness'
sys.modules[legacy_action.__name__] = legacy_action
exec(compile((REFERENCE / 'test_tool.py.txt').read_text(), 'pre-extraction/test_tool.py', 'exec'), legacy_action.__dict__)
legacy_worker = types.ModuleType('tl3_workers.legacy_pb_tests')
legacy_worker.__package__ = 'tl3_workers'
sys.modules[legacy_worker.__name__] = legacy_worker
# This reference retains all six original bounding call sites verbatim.
exec(compile((REFERENCE / 'pb_tests.py.txt').read_text(), 'pre-extraction/pb_tests.py', 'exec'), legacy_worker.__dict__)


class Crash(BaseException):
    pass


def action(module, directory, schedule):
    for path in directory.glob('progress*'):
        path.unlink()
    progress = directory / 'progress.json'
    progress.write_text(json.dumps({'token': 'fixture', 'units_completed': schedule['initial']}))
    trace, mutation = [], 0

    def change(call, perform):
        nonlocal mutation
        trace.append(call)
        position, mutation = mutation, mutation + 1
        if schedule['crash'] == position:
            raise Crash()
        return perform()

    class File:
        def __init__(self, stream):
            self.stream = stream

        def __enter__(self):
            self.stream.__enter__()
            return self

        def __exit__(self, *args):
            return self.stream.__exit__(*args)

        def read(self, *args, **kwargs):
            return self.stream.read(*args, **kwargs)

        def write(self, value):
            return change(('write', value), lambda: self.stream.write(value))

        def flush(self):
            return change(('flush',), self.stream.flush)

        def fileno(self):
            return self.stream.fileno()

    class Selection(io.BytesIO):
        def read(self, *args, **kwargs):
            return self.stream.read(*args, **kwargs)

        def write(self, value):
            return change(('selection-write', value.hex()), lambda: super(Selection, self).write(value))

        def seek(self, offset, whence=0):
            trace.append(('selection-seek', offset, whence))
            return super().seek(offset, whence)

    outcomes = ['passed', 'failed', 'error', 'skipped']
    entries = [{'event': 'result', 'test': f'test_schedule.py::test_{index}',
                'outcome': outcomes[code % 4], 'message': f'message-{code}', 'truncated': False}
               for index, code in enumerate(schedule['reads'])]
    exit_code = int(any(entry['outcome'] in ('failed', 'error') for entry in entries))
    records = [{'event': 'start', 'schema': 1}, *entries, {'event': 'finish', 'exit_code': exit_code}]

    class Process:
        report_started = True

        def __init__(self, command, selection, tests):
            trace.append(('launch', command, selection.getvalue().hex(), tests))

        def __enter__(self):
            return self

        def __exit__(self, *args):
            trace.append(('process-close',))

        def records(self):
            for index, record in enumerate(records):
                trace.append(('report-read', index, deepcopy(record)))
                yield json.dumps(record)

        @property
        def exit_code(self):
            trace.append(('exit-read', exit_code))
            return exit_code

        def incomplete(self):
            trace.append(('incomplete',))
            return ExecutionFailure(ExecutionOutcome.UNKNOWN_INCOMPLETE, 'unknown', 'incomplete_report')

    original_open, original_read, original_replace = Path.open, Path.read_text, Path.replace
    original_sync = execution.os.fsync
    times = iter([code / 1000 for code in schedule['times']])

    def clock():
        value = next(times, 123.5)
        trace.append(('clock', value))
        return value

    def opened(path, mode='r', *args, **kwargs):
        if path.parent == directory and path.name.startswith('progress'):
            trace.append(('open', path.name, mode))
            return File(original_open(path, mode, *args, **kwargs))
        return original_open(path, mode, *args, **kwargs)

    def read(path, *args, **kwargs):
        if path == progress:
            trace.append(('progress-read',))
        return original_read(path, *args, **kwargs)

    def replace(path, target):
        return change(('rename', path.name, Path(target).name), lambda: original_replace(path, target))

    with pytest.MonkeyPatch.context() as patch:
        patch.setenv('PRISMABUILD_ACTION_PROGRESS_PATH', str(progress))
        patch.setenv('PRISMABUILD_ACTION_PROGRESS_TOKEN', 'fixture')
        patch.setattr(module, 'require_admission', lambda: trace.append(('admission',)))
        patch.setattr(module, 'container_command', lambda *args: ['fake-container'])
        patch.setattr(module, 'Supervised', Process)
        patch.setattr(module.tempfile, 'TemporaryFile', Selection)
        patch.setattr(execution.time, 'time', clock)
        patch.setattr(execution.os, 'getpid', lambda: 823)
        patch.setattr(execution.os, 'fsync', lambda fd: change(('fsync',), lambda: original_sync(fd)))
        patch.setattr(Path, 'open', opened)
        patch.setattr(Path, 'read_text', read)
        patch.setattr(Path, 'replace', replace)
        try:
            result = ('result', module.run_current(directory, None, ()))
        except Crash:
            result = ('crash',)
    state = {path.name: path.read_bytes().hex() for path in sorted(directory.glob('progress*'))}
    return trace, result, state


def worker(module, schedule):
    summaries = []
    for index in (0, 1):
        summaries.append({'status': 'completed', 'tests': [],
                          'totals': {'passed': schedule['initial'] + index, 'failed': 0, 'error': 0, 'skipped': 0},
                          'note': ''})
    variants = [(test_tool.PREFIX + json.dumps(value) + '\n').encode() for value in summaries]
    if schedule.get('proof_bytes') is not None:
        facts = {**summaries[0], 'proof': {'mode': 'head-observer', 'payload': 'p' * schedule['proof_bytes']},
                 'origin': 'test_run'}
        # The reference is the complete former producer/consumer path. The new
        # path sends raw facts to Rust before decorating the verified receipt.
        producer = legacy_action.bounded if module is legacy_worker else test_tool.action_summary
        variants = [(test_tool.PREFIX + json.dumps(producer(facts)) + '\n').encode()] * 2
    raw = variants[0]
    key = 'a' * 64
    ending = {'action_key': key, 'status': 'executed', 'exit_code': 0, 'succeeded': True,
              'result': {'verified': True, 'receipt_sha256': 'b' * 64,
                         'stdout': {'path': 'scheduled-action.log', 'bytes': len(raw),
                                    'sha256': hashlib.sha256(raw).hexdigest()}}}
    trace, calls = [], 0
    original = builtins.open

    class Log(io.BytesIO):
        def read(self, size=-1):
            nonlocal calls
            trace.append(('log-read', size))
            # A second read observes different bytes. Re-reading is detectable.
            value = variants[min(calls, 1)]
            calls += 1
            return value[:size] if size >= 0 else value

        def __exit__(self, *args):
            trace.append(('log-close',))
            return super().__exit__(*args)

    def opened(path, mode='r', *args, **kwargs):
        if path == 'scheduled-action.log':
            trace.append(('log-open', path, mode))
            return Log()
        return original(path, mode, *args, **kwargs)

    with pytest.MonkeyPatch.context() as patch:
        patch.setattr(builtins, 'open', opened)
        result = module._summary(deepcopy(ending), key)
    return trace, result, {}


def decide(schedule):
    with tempfile.TemporaryDirectory(prefix='harness-extraction-') as name:
        directory = Path(name)
        before = action(legacy_action, directory, schedule)
        after = action(test_tool, directory, schedule)
        assert before == after, ('action schedule changed', schedule, before, after)
    before = worker(legacy_worker, schedule)
    after = worker(pb_tests, schedule)
    assert before == after, ('worker log schedule changed', schedule, before, after)
    # Receipt log hashes identify different producer bytes after retention.
    # Every other worker-visible field must agree, including the refusal.
    for size in [14_000, 14_500, 20_000, 750_000, 800_000, 1_050_000]:
        enlarged = {**schedule, 'proof_bytes': size}
        old_trace, old_result, old_state = worker(legacy_worker, enlarged)
        new_trace, new_result, new_state = worker(pb_tests, enlarged)
        for result in [old_result, new_result]:
            for receipt in result.get('receipts', []):
                receipt.pop('log_sha256')
        assert (old_trace, old_result, old_state) == (new_trace, new_result, new_state), (size, old_result, new_result)
    return {'equal': True}


if __name__ == '__main__':
    for line in sys.stdin:
        print(json.dumps(decide(json.loads(line))), flush=True)
