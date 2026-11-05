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
        result = perform()
        trace.append(('after', call))
        position, mutation = mutation, mutation + 1
        if schedule['crash'] == position:
            raise Crash()
        return result

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
            trace.append(('selection-read',))
            return super().read(*args, **kwargs)

        def write(self, value):
            return change(('selection-write', value.hex()), lambda: super(Selection, self).write(value))

        def seek(self, offset, whence=0):
            trace.append(('selection-seek', offset, whence))
            return super().seek(offset, whence)

    outcomes = ['passed', 'failed', 'error', 'skipped']
    entries = [{'event': 'result', 'test': f't.py::t{index:02}',
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
    original_exists, original_is_file = Path.exists, Path.is_file
    existence = iter(schedule.get('selector_reads', [1]))
    output = io.StringIO()

    class Output:
        def write(self, text):
            return change(('stdout-write', text), lambda: output.write(text))

        def flush(self):
            return change(('stdout-flush',), lambda: None)

    def exists(path):
        if path.name in ('t.py', 'other.py'):
            observed = bool(next(existence, 1))
            trace.append(('selector-exists', path.name, observed))
            return observed
        if path == progress:
            trace.append(('progress-exists',))
        return original_exists(path)

    def is_file(path):
        if path == Path('proof.json'):
            observed = schedule.get('proof', True)
            trace.append(('proof-is-file', observed))
            return observed
        return original_is_file(path)
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
        if path == Path('proof.json'):
            value = json.dumps({'mode':'pytest-outcome', 'sample':schedule['initial']})
            trace.append(('proof-read', value))
            return value
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
        patch.setattr(execution.os, 'fsync', lambda fd: change(('fsync',), lambda: None))
        patch.setattr(Path, 'open', opened)
        patch.setattr(Path, 'read_text', read)
        patch.setattr(Path, 'replace', replace)
        patch.setattr(Path, 'exists', exists)
        patch.setattr(Path, 'is_file', is_file)
        selected = tuple(schedule.get('selectors', ('t.py', 'other.py')))
        repeat = schedule.get('repeat', 2)
        try:
            mode = schedule.get('mode', 'run_current')
            if mode == 'main':
                patch.chdir(directory)
                patch.setattr(sys, 'argv', ['test_tool', 'sha256:' + '1' * 64, json.dumps(selected), '--repeat', str(repeat)])
                patch.setattr(sys, 'stdout', Output())
                module.main()
                result = ('stdout', output.getvalue())
            elif mode == 'run':
                result = ('result', module.run(directory, None, selected, repeat=repeat))
            else:
                result = ('result', module.run_current(directory, None, selected))
        except Crash:
            result = ('crash',)
    state = {path.name: path.read_bytes().hex() for path in sorted(directory.glob('progress*'))}
    state['stdout'] = output.getvalue()
    return trace, result, state, mutation


def worker(module, schedule, facts=None):
    summaries = []
    for index in (0, 1):
        summaries.append({'status': 'completed', 'tests': [],
                          'totals': {'passed': schedule['initial'] + index, 'failed': 0, 'error': 0, 'skipped': 0},
                          'note': ''})
    variants = [(test_tool.PREFIX + json.dumps(value) + '\n').encode() for value in summaries]
    if facts is not None:
        variants = [(test_tool.PREFIX + json.dumps(facts) + '\n').encode()] * 2
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



def lifecycle(module, directory, schedule):
    """Run the real staging and submission/reattachment shell with recorded edges."""
    from tl3_workers.tools import Worktree

    source, staged = directory / 'source', directory / 'stage'
    source.mkdir(); staged.mkdir()
    (source / 't.py').write_text('def test_ok(): pass\n')
    trace, mutation = [], 0
    original_open, original_mkdir = Path.open, Path.mkdir
    original_read_bytes, original_read_text = Path.read_bytes, Path.read_text
    original_replace = Path.replace

    def name(path):
        path = Path(path)
        if path.is_relative_to(directory):
            return str(path.relative_to(directory))
        return path.name

    def change(call, perform):
        nonlocal mutation
        trace.append(call)
        position, mutation = mutation, mutation + 1
        if schedule['crash'] == position:
            raise Crash()
        result = perform()
        trace.append(('after', call))
        position, mutation = mutation, mutation + 1
        if schedule['crash'] == position:
            raise Crash()
        return result

    class File:
        def __init__(self, stream, path): self.stream, self.path = stream, name(path)
        def __enter__(self): self.stream.__enter__(); return self
        def __exit__(self, *args): return self.stream.__exit__(*args)
        def fileno(self): return self.stream.fileno()
        def read(self, *args):
            value = self.stream.read(*args)
            trace.append(('stage-read', self.path, value.hex() if isinstance(value, bytes) else value))
            return value
        def write(self, value):
            return change(('stage-write', self.path, value.hex() if isinstance(value, bytes) else value), lambda: self.stream.write(value))
        def flush(self): return change(('stage-flush', self.path), self.stream.flush)

    class Temporary:
        def __init__(self, **kwargs): trace.append(('stage-temporary', kwargs))
        def __enter__(self): return str(staged)
        def __exit__(self, *args): trace.append(('stage-close',))

    def opened(path, mode='r', *args, **kwargs):
        if path.is_relative_to(directory):
            trace.append(('stage-open', name(path), mode))
            return File(original_open(path, mode, *args, **kwargs), path)
        return original_open(path, mode, *args, **kwargs)

    def mkdir(path, *args, **kwargs):
        if path.is_relative_to(directory):
            return change(('mkdir', name(path)), lambda: original_mkdir(path, *args, **kwargs))
        return original_mkdir(path, *args, **kwargs)

    def read_bytes(path):
        if path.name in ('pb_tests.py', 'pb_tests.py.txt'):
            value = b'recorded trusted worker bytes'
            trace.append(('trusted-source-read', value.hex()))
            return value
        return original_read_bytes(path)

    def read_text(path, *args, **kwargs):
        if path.is_relative_to(directory): trace.append(('stage-read-text', name(path)))
        return original_read_text(path, *args, **kwargs)

    def copytree(origin, target, **kwargs):
        trace.append(('trusted-copy', name(origin), name(target)))
        Path(target).mkdir()
        (Path(target) / 'marker').write_bytes(b'trusted harness')

    def git(root, *args):
        return change(('stage-git', args), lambda: 'c' * 40 if args[0] == 'write-tree' else '')

    raw = (test_tool.PREFIX + json.dumps({'status':'completed', 'tests':[],
        'totals':dict.fromkeys(test_tool.OUTCOMES, 0), 'note':''}) + '\n').encode()
    key = 'a' * 64
    ending = {'action_key':key, 'status':'executed', 'exit_code':0, 'succeeded':True,
        'result':{'verified':True, 'receipt_sha256':'b' * 64,
        'stdout':{'path':str(staged / 'action.log'), 'bytes':len(raw), 'sha256':hashlib.sha256(raw).hexdigest()}}}
    waits = iter(schedule.get('waits', [False, True]))

    def command(argv):
        if 'pbrun.py' in argv[1]:
            # Paths and Python executable are boundary inputs; compare stable paths.
            args = [str(value).replace(str(directory), '<root>') for value in argv]
            trace.append(('submit', args))
            return 0, {'schema':'prismabuild.submission.v1', 'action_key':key}
        complete = bool(next(waits, True))
        trace.append(('upstream-wait', key, complete))
        value = deepcopy(ending)
        if not complete: value['status'] = 'waiting'
        return 0, {'schema':'prismabuild.endings.v1', 'complete':complete, 'endings':[value]}

    class Log(io.BytesIO):
        def read(self, size=-1):
            trace.append(('verified-log-read', size))
            return super().read(size)

    original_builtin = builtins.open
    def log_open(path, mode='r', *args, **kwargs):
        if path == str(staged / 'action.log'):
            trace.append(('verified-log-open', mode))
            return Log(raw)
        return original_builtin(path, mode, *args, **kwargs)

    with Worktree(source) as tree, pytest.MonkeyPatch.context() as patch:
        patch.setattr(Path, 'open', opened); patch.setattr(Path, 'mkdir', mkdir)
        patch.setattr(Path, 'read_bytes', read_bytes); patch.setattr(Path, 'read_text', read_text)
        patch.setattr(Path, 'replace', lambda path, target: change(('stage-rename', name(path), name(target)), lambda: original_replace(path, target)))
        patch.setattr(module.tempfile, 'TemporaryDirectory', Temporary)
        patch.setattr(module.shutil, 'copytree', copytree)
        patch.setattr(module, '_git', git); patch.setattr(module, '_json_command', command)
        patch.setattr(module, 'scratch_environment', lambda: {})
        patch.setattr(module.secrets, 'token_hex', lambda n: 'fresh')
        patch.setattr(builtins, 'open', log_open)
        if schedule.get('reattach'):
            # The production contract hash is observed at the checkpoint boundary.
            class Checkpoint(dict):
                def pop(self, base, default=None):
                    trace.append(('reattach-read', base, tree.pb_identity))
                    return {'identity':tree.pb_identity, 'tree':'c' * 40, 'key':key}
            tree.pb_reattach = Checkpoint()
        def emit(value): return change(('progress-emit', value), lambda: None)
        tree.pb_submitted = lambda *args: change(('checkpoint-write', args), lambda: None)
        settings = module.TestSettings('sha256:' + '1' * 64, Path('/recorded-tools'))
        try:
            result = ('result', module.make_test_tool(tree, settings, emit=emit)(['t.py'], repeat=2))
        except Crash:
            result = ('crash',)
    state = {str(path.relative_to(directory)):path.read_bytes().hex()
        for path in sorted(staged.rglob('*')) if path.is_file()}
    return trace, result, state, mutation


REFUSALS_CHECKED = False


def compare_crashes(function, before_module, after_module, schedule):
    """Interrupt both sides immediately before/after every mutation in the trace."""
    def run(module, crash):
        with tempfile.TemporaryDirectory(prefix='harness-extraction-') as name:
            return function(module, Path(name), {**schedule, 'crash':crash})
    old = run(before_module, -1)
    new = run(after_module, -1)
    assert old == new, ('complete schedule changed', function.__name__, schedule, old, new)
    for crash in range(old[3]):
        before, after = run(before_module, crash), run(after_module, crash)
        assert before == after, ('crash schedule changed', function.__name__, schedule, crash, before, after)
    return old


def decide(schedule):
    global REFUSALS_CHECKED
    # The fixed below-cap counterexample must be compared before the random paths.
    if not REFUSALS_CHECKED:
        example = {**schedule, 'reads':[1] * 21, 'selector_reads':[1, 1],
                   'selectors':['t.py'], 'repeat':2, 'mode':'run', 'crash':-1}
        with tempfile.TemporaryDirectory(prefix='harness-repeat-') as name:
            old = action(legacy_action, Path(name), example)
            new = action(test_tool, Path(name), example)
        assert old == new, ('legacy run-list cap changed', old, new)
        vector = json.loads((REFERENCE / 'repeated-run.json').read_text())
        assert old[1] == ('result', vector['action'])
        before = worker(legacy_worker, schedule, facts=old[1][1])
        after = worker(pb_tests, schedule, facts=new[1][1])
        assert before == after
        assert after[1] == vector['worker'], ('complete worker parity vector changed', after)
    for mode in ('run_current', 'run', 'main'):
        compare_crashes(action, legacy_action, test_tool, {**schedule, 'mode':mode})
    for reattach in (False, True):
        compare_crashes(lifecycle, legacy_worker, pb_tests, {**schedule, 'reattach':reattach})
    before = worker(legacy_worker, schedule)
    after = worker(pb_tests, schedule)
    assert before == after, ('worker log schedule changed', schedule, before, after)
    for size in ([] if REFUSALS_CHECKED else [14_000, 14_500, 20_000, 750_000, 800_000, 1_050_000]):
        enlarged = {**schedule, 'proof_bytes':size}
        old_trace, old_result, old_state = worker(legacy_worker, enlarged)
        new_trace, new_result, new_state = worker(pb_tests, enlarged)
        for result in [old_result, new_result]:
            for receipt in result.get('receipts', []): receipt.pop('log_sha256')
        assert (old_trace, old_result, old_state) == (new_trace, new_result, new_state), (size, old_result, new_result)
    REFUSALS_CHECKED = True
    return {'equal':True}


if __name__ == '__main__':
    for line in sys.stdin:
        print(json.dumps(decide(json.loads(line))), flush=True)
