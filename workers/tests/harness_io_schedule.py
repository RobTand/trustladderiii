"""#823: persistent old/new I/O oracle driven by controller-side proptest."""
import builtins
from contextlib import contextmanager
from copy import deepcopy
import hashlib
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import types
from unittest.mock import patch

import pytest

from tl3_harness import execution, runtime, selector_identity as identity, test_tool
from tl3_harness.selector_identity import selector_identity
from tl3_harness.supervision import ExecutionFailure, ExecutionOutcome
from tl3_workers import tests_tool


# Frozen reference dependency, copied verbatim from 2e8cbefd9fe914d33308d8872838b34bbdb80219
# harness/src/tl3_harness/selector_identity.py:15. Only the reference imports this copy.
def selection_matches(value, selectors):
    """Compare a full list or digest to the trusted full selection, never its prefix."""
    if isinstance(value, list):
        return value == list(selectors)
    return (isinstance(value, dict) and type(value.get('count')) is int
            and value == selector_identity(selectors))


REFERENCE = Path(__file__).with_name('fixtures') / 'harness-extraction'
legacy_action = types.ModuleType('tl3_harness.legacy_test_tool')
legacy_action.__package__ = 'tl3_harness'
sys.modules[legacy_action.__name__] = legacy_action
exec(compile((REFERENCE / 'test_tool.py.txt').read_text(), 'pre-extraction/test_tool.py', 'exec'), legacy_action.__dict__)
legacy_worker = types.ModuleType('tl3_workers.legacy_pb_tests')
legacy_worker.__package__ = 'tl3_workers'
legacy_worker.__file__ = str(REFERENCE / 'pb_tests.py.txt')
sys.modules[legacy_worker.__name__] = legacy_worker
# This reference retains all six original bounding call sites verbatim.
with patch.object(identity, 'selection_matches', selection_matches, create=True):
    exec(compile((REFERENCE / 'pb_tests.py.txt').read_text(), 'pre-extraction/pb_tests.py', 'exec'), legacy_worker.__dict__)


class Crash(BaseException):
    pass



class RecordedCalls(list):
    stopped = False

    def append(self, call):
        if not self.stopped:
            super().append(call)


class Recording:
    """Freeze the visible filesystem and trace at the cut, before stack unwind."""
    def __init__(self, crash, snapshot):
        self.trace = RecordedCalls()
        self.mutation = 0
        self.crash = crash
        self.snapshot = snapshot
        self.cut = None

    def boundary(self):
        position = self.mutation
        self.mutation += 1
        if self.crash == position:
            self.trace.stopped = True
            self.cut = (list(self.trace), self.snapshot(), self.mutation)
            raise Crash()

    def change(self, call, perform):
        if self.trace.stopped:
            # Context managers may clean up real handles after the frozen cut.
            return perform()
        self.trace.append(call)
        self.boundary()
        result = perform()
        self.trace.append(('after', call))
        self.boundary()
        return result

    def finish(self, result):
        if self.cut is not None:
            trace, state, mutation = self.cut
            return trace, ('crash',), state, mutation
        return self.trace, result, self.snapshot(), self.mutation


def action(module, directory, schedule):
    for path in directory.glob('progress*'):
        path.unlink()
    # The Flux job's progress journal: one line per completed unit (execution.Progress).
    progress = directory / 'progress.jsonl'
    progress.write_text(''.join(json.dumps(f'prior-{index}') + '\n' for index in range(schedule['initial'])))
    (directory / 'scratch').mkdir(mode=0o700, exist_ok=True)
    context = {'run': {'backend': 'flux', 'uri': 'local:///fixture/flux', 'instance': 'fixture', 'job': 'f823'},
               'scratch': {'leaf': str(directory / 'scratch')},
               'progress': {'journal': str(progress), 'no_progress_seconds': 120},
               'output_dir': str(directory)}
    recording = Recording(schedule['crash'], lambda: snapshot())
    trace, change = recording.trace, recording.change

    class File:
        def __init__(self, stream):
            self.stream = stream

        def __enter__(self):
            self.stream.__enter__()
            return self

        def __exit__(self, *args):
            try:
                if self.stream.writable():
                    self.flush()  # Record the close's implicit buffered flush.
                return change(('close',), lambda: self.stream.__exit__(*args))
            finally:
                self.stream.close()

        def read(self, *args, **kwargs):
            return self.stream.read(*args, **kwargs)

        def __iter__(self):
            # execution.Progress counts the journal's completed units line by line.
            return iter(self.stream)

        def write(self, value):
            return change(('write', value), lambda: self.stream.write(value))

        def flush(self):
            return change(('flush',), self.stream.flush)

        def fileno(self):
            return self.stream.fileno()

    class Selection(io.BytesIO):
        def __exit__(self, *args):
            try:
                self.flush()
                return change(('selection-close',), lambda: super(Selection, self).__exit__(*args))
            finally:
                super().close()

        def flush(self):
            return change(('selection-flush',), lambda: super(Selection, self).flush())

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
                'outcome': outcomes[code % 4], 'message': '' if schedule.get('empty_messages') else f'message-{code}', 'truncated': False}
               for index, code in enumerate(schedule['reads'])]
    launches = 0

    class Process:
        report_started = True

        def __init__(self, command, selection, tests):
            nonlocal launches
            trace.append(('launch', command, selection.getvalue().hex(), tests))
            observed = deepcopy(entries)
            codes = schedule['reads']
            for index, item in enumerate(observed):
                item['outcome'] = outcomes[codes[(index + launches) % len(codes)] % 4]
            launches += 1
            self.code = int(any(item['outcome'] in ('failed', 'error') for item in observed))
            self.observed = [{'event':'start', 'schema':1}, *observed, {'event':'finish', 'exit_code':self.code}]

        def __enter__(self):
            return self

        def __exit__(self, *args):
            trace.append(('process-close',))

        def records(self):
            for index, record in enumerate(self.observed):
                trace.append(('report-read', index, deepcopy(record)))
                yield json.dumps(record)

        @property
        def exit_code(self):
            trace.append(('exit-read', self.code))
            return self.code

        def incomplete(self):
            trace.append(('incomplete',))
            return ExecutionFailure(ExecutionOutcome.UNKNOWN_INCOMPLETE, 'unknown', 'incomplete_report')

    original_open, original_read, original_replace = Path.open, Path.read_text, Path.replace
    original_exists, original_is_file = Path.exists, Path.is_file
    existence = iter(schedule.get('selector_reads', [1]))
    output = io.StringIO()
    published = ['']
    proof_reads = 0

    def snapshot():
        state = {}
        for path in sorted(directory.glob('progress*')):
            with original_open(path, 'rb') as stream:
                state[path.name] = stream.read().hex()
        state['stdout'] = published[0]
        return state

    class Output:
        def write(self, text):
            return change(('stdout-write', text), lambda: output.write(text))

        def flush(self):
            return change(('stdout-flush',), lambda: published.__setitem__(0, output.getvalue()))

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
    def opened(path, mode='r', *args, **kwargs):
        if path.parent == directory and path.name.startswith('progress'):
            call = ('open', path.name, mode)
            if any(flag in mode for flag in 'wax+'):
                return change(call, lambda: File(original_open(path, mode, *args, **kwargs)))
            trace.append(call)
            return File(original_open(path, mode, *args, **kwargs))
        return original_open(path, mode, *args, **kwargs)

    def read(path, *args, **kwargs):
        nonlocal proof_reads
        if path == progress:
            trace.append(('progress-read',))
        if path == Path('proof.json'):
            value = json.dumps({'mode':'pytest-outcome', 'sample':schedule['initial'] + proof_reads})
            proof_reads += 1
            trace.append(('proof-read', value))
            return value
        return original_read(path, *args, **kwargs)

    def replace(path, target):
        return change(('rename', path.name, Path(target).name), lambda: original_replace(path, target))

    with pytest.MonkeyPatch.context() as patch:
        patch.setenv('TL3_EXECUTION_BACKEND', 'flux')
        patch.setattr(runtime, 'launch_context', lambda: context)
        patch.setattr(runtime, 'journal_path', lambda path: path)
        patch.setattr(module, 'require_admission', lambda: trace.append(('admission',)))
        patch.setattr(module, 'container_command', lambda *args: ['fake-container'])
        patch.setattr(module, 'Supervised', Process)
        patch.setattr(module.tempfile, 'TemporaryFile', lambda: change(('selection-open',), Selection))
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
    return recording.finish(result)


NATIVE_KEY = 'worker:' + 'a' * 64
SEMANTIC_FIELDS = ('status', 'execution', 'totals', 'proof', 'selectors', 'origin', 'message')


def summary_facts(schedule, facts=None):
    """The action-side facts a summary consumer reads, as worker() fed them before Flux."""
    if facts is not None:
        return facts, facts
    summary = {'status': 'completed', 'tests': [],
               'totals': {'passed': schedule['initial'], 'failed': 0, 'error': 0, 'skipped': 0}, 'note': ''}
    if schedule.get('proof_bytes') is None:
        return summary, summary
    raw = {**summary, 'proof': {'mode': 'head-observer', 'payload': 'p' * schedule['proof_bytes']},
           'origin': 'test_run'}
    return legacy_action.bounded(raw), test_tool.action_summary(raw)


def legacy_summary(facts):
    """The pre-extraction PB consumer: it read the verified action log once."""
    raw = (test_tool.PREFIX + json.dumps(facts) + '\n').encode()
    key = 'a' * 64
    ending = {'action_key': key, 'status': 'executed', 'exit_code': 0, 'succeeded': True,
              'result': {'verified': True, 'receipt_sha256': 'b' * 64,
                         'stdout': {'path': 'scheduled-action.log', 'bytes': len(raw),
                                    'sha256': hashlib.sha256(raw).hexdigest()}}}
    original = builtins.open

    def opened(path, mode='r', *args, **kwargs):
        if path == 'scheduled-action.log':
            return io.BytesIO(raw)
        return original(path, mode, *args, **kwargs)

    with pytest.MonkeyPatch.context() as patch:
        patch.setattr(builtins, 'open', opened)
        return legacy_worker._summary(deepcopy(ending), key)


def native_receipt():
    from pb_fixtures import run_binding
    return {'backend': 'flux', 'wait_reference': NATIVE_KEY, 'run_binding': run_binding(NATIVE_KEY),
            'receipt_sha256': 'b' * 64, 'ending': 'passed'}


def native_summary(facts):
    """The real Flux consumer layer of run_tests for a verified payload carrying these facts.

    This is the tool's inner run(): the layer the PB _summary reader and finish()
    occupied. The outer run_tests wrapper's failure baseline is a separate step.
    """
    from tl3_workers import flux_execution
    from tl3_workers.tools import Worktree
    with tempfile.TemporaryDirectory(prefix='harness-native-summary-') as name, \
            pytest.MonkeyPatch.context() as patch, Worktree(name) as tree:
        patch.setattr(flux_execution, 'activation', lambda settings: None)
        patch.setattr(flux_execution, 'allocate', lambda settings: ('a' * 64, Path(name)))
        patch.setattr(flux_execution, 'prepare', lambda *args, **kwargs: {'key': NATIVE_KEY})
        patch.setattr(flux_execution, 'execute', lambda plan, **kwargs: {
            'kind': 'tests', 'summary': deepcopy(facts), 'symptom_runs': [], 'symptom_receipts': [],
            'receipts': [native_receipt()]})
        patch.setattr(tests_tool, '_stage', lambda *args, **kwargs: 'c' * 40)
        patch.setattr(tests_tool, '_git', lambda *args: 'c' * 40)
        settings = tests_tool.TestSettings('sha256:' + '1' * 64,
                                           execution={'backend': 'flux', 'profile': '/recorded/execution.json'})
        run_tests = tests_tool.make_test_tool(tree, settings)
        [run] = [cell.cell_contents for name_, cell in zip(run_tests.__code__.co_freevars, run_tests.__closure__)
                 if name_ == 'run']
        return run(['t.py'])


def compare_consumers(legacy_facts, native_facts, context, reference=None):
    """The recorded consumer's verdict, then the canonical native envelope.

    #823 moved the summary bound before receipt decoration, so the two pre-bound
    payloads differ by design; their verdict fields must not. The native envelope
    must be exactly the canonical bound of its verified payload plus the native
    receipt, and must equal the reference's entries and note whenever both fit.
    """
    from tl3_harness.supervision import model_result
    legacy = legacy_summary(legacy_facts) if reference is None else reference
    semantic = tests_tool._verified_test_summary(deepcopy(native_facts), [])
    native = native_summary(native_facts)
    assert native['wait_reference'] == NATIVE_KEY, (context, native)
    decorated = model_result({**deepcopy(semantic), 'wait_reference': NATIVE_KEY, 'receipts': [native_receipt()]})
    expected = tests_tool.bounded(decorated, limit=test_tool.SUMMARY_BYTES - 512)
    expected = tests_tool.bounded({**expected, 'tree': 'c' * 40}, limit=test_tool.SUMMARY_BYTES)
    assert native == expected, (context, 'native envelope is not the canonical bound of the same payload')
    assert len(json.dumps(native).encode()) <= test_tool.SUMMARY_BYTES, context
    for name_ in SEMANTIC_FIELDS:
        assert native.get(name_) == legacy.get(name_), (context, name_, legacy.get(name_), native.get(name_))
    fits = len(json.dumps({**decorated, 'tree': 'c' * 40}).encode()) <= test_tool.SUMMARY_BYTES - 512
    if fits:
        assert native.get('tests') == semantic.get('tests') and native.get('note') == semantic.get('note'), \
            (context, 'a native envelope that fits keeps every verified entry and note')
        if 'Truncated' not in str(legacy.get('note', '')):
            assert native.get('tests') == legacy.get('tests') and native.get('note') == legacy.get('note'), \
                (context, 'entries or note changed although both envelopes fit')
    return native


def lifecycle(module, directory, schedule):
    """Run the real staging shell with recorded edges.

    The PB submission, wait and reattachment transport this once compared was
    deleted with PrismaBuild; Flux transport is covered by the native worker tests.
    """
    from tl3_workers.tools import Worktree

    source, staged = directory / 'source', directory / 'stage'
    source.mkdir(); staged.mkdir()
    (source / 't.py').write_text('def test_ok(): pass\n')
    recording = Recording(schedule['crash'], lambda: snapshot())
    trace, change = recording.trace, recording.change
    original_open, original_mkdir = Path.open, Path.mkdir
    original_read_bytes, original_read_text = Path.read_bytes, Path.read_text
    original_replace = Path.replace
    original_chmod = os.fchmod
    original_rmtree = module.shutil.rmtree
    source_reads = 0

    def name(path):
        path = Path(path)
        if path.is_relative_to(directory):
            return str(path.relative_to(directory))
        return path.name

    class File:
        def __init__(self, stream, path):
            self.stream, self.path = stream, name(path) if isinstance(path, Path) else path
        def __enter__(self): self.stream.__enter__(); return self
        def __exit__(self, *args):
            try:
                if self.stream.writable(): self.flush()
                return change(('stage-file-close', self.path), lambda: self.stream.__exit__(*args))
            finally:
                self.stream.close()
        def fileno(self): return self.stream.fileno()
        @property
        def buffer(self): return File(self.stream.buffer, self.path)
        def read(self, *args):
            value = self.stream.read(*args)
            trace.append(('stage-read', self.path, value.hex() if isinstance(value, bytes) else value))
            return value
        def write(self, value):
            return change(('stage-write', self.path, value.hex() if isinstance(value, bytes) else value), lambda: self.stream.write(value))
        def flush(self): return change(('stage-flush', self.path), self.stream.flush)

    def opened(path, mode='r', *args, **kwargs):
        if path.is_relative_to(directory):
            call = ('stage-open', name(path), mode)
            if any(flag in mode for flag in 'wax+'):
                return change(call, lambda: File(original_open(path, mode, *args, **kwargs), path))
            trace.append(call)
            return File(original_open(path, mode, *args, **kwargs), path)
        return original_open(path, mode, *args, **kwargs)

    def mkdir(path, *args, **kwargs):
        if path.is_relative_to(directory):
            return change(('mkdir', name(path)), lambda: original_mkdir(path, *args, **kwargs))
        return original_mkdir(path, *args, **kwargs)

    def read_bytes(path):
        nonlocal source_reads
        if path.name in ('tests_tool.py', 'pb_tests.py.txt'):
            value = f'recorded trusted worker bytes {schedule["initial"] + source_reads}'.encode()
            source_reads += 1
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
        observed = tuple(str(arg).replace(str(directory), '<root>') for arg in args)
        return change(('stage-git', observed), lambda: 'c' * 40 if args[0] == 'write-tree' else '')

    proof = {'mode':'pytest-outcome', 'sample':schedule['initial']}
    def snapshot():
        state = {}
        for path in sorted(staged.rglob('*')):
            if path.is_file():
                with original_open(path, 'rb') as stream:
                    state[str(path.relative_to(directory))] = (stream.read().hex(), path.stat().st_mode & 0o777)
        return state

    with Worktree(source) as tree, pytest.MonkeyPatch.context() as patch:
        patch.setattr(Path, 'open', opened); patch.setattr(Path, 'mkdir', mkdir)
        patch.setattr(Path, 'read_bytes', read_bytes); patch.setattr(Path, 'read_text', read_text)
        patch.setattr(Path, 'replace', lambda path, target: change(('stage-rename', name(path), name(target)), lambda: original_replace(path, target)))
        original_directory, original_walk, original_file = tree._directory, tree._walk, tree._file
        @contextmanager
        def source_directory(parts, **kwargs):
            trace.append(('source-directory', parts, kwargs))
            with original_directory(parts, **kwargs) as descriptor: yield descriptor
        def source_walk(descriptor, prefix=""):
            values = list(original_walk(descriptor, prefix))
            trace.append(('source-walk', values))
            return iter(values)
        @contextmanager
        def source_file(path):
            trace.append(('source-file', path))
            with original_file(path) as stream:
                with File(stream, source / path) as recorded:
                    yield recorded
        patch.setattr(tree, '_directory', source_directory)
        patch.setattr(tree, '_walk', source_walk)
        patch.setattr(tree, '_file', source_file)
        patch.setattr(os, 'fchmod', lambda fd, mode: change(('stage-chmod', mode), lambda: original_chmod(fd, mode)))
        patch.setattr(module.shutil, 'copytree', copytree)
        patch.setattr(module.shutil, 'rmtree', lambda path, *args, **kwargs: change(('stage-remove', name(path)), lambda: original_rmtree(path, *args, **kwargs)))
        if schedule.get('base'):
            tree.test_checkout = source
            tree.check_base_sha = 'd' * 40
            def export(checkout, sha, target):
                trace.append(('base-export', name(checkout), sha, name(target)))
                target.mkdir()
                (target / 't.py').write_text((source / 't.py').read_text())
            def assemble(base, head, checkout, check, configuration, *, role='maker', check_index=0, footprint=(), protected=(), generated=()):
                trace.append(('assemble-proof', name(base), name(head), name(checkout)))
                checkout.mkdir()
                (checkout / 't.py').write_text((base / 't.py').read_text())
                return deepcopy(proof)
            patch.setattr(module, 'export', export)
            patch.setattr(module, 'assemble_proof', assemble)
        patch.setattr(module, '_git', git)
        patch.setattr(module, 'scratch_environment', lambda: {})
        base_sha = 'd' * 40 if schedule.get('base') else None
        try:
            # The run shell's staging sequence: stage, read a base proof, hash the trusted tree.
            tree_id = module._stage(tree, staged, base_sha, checks=(), selected=('t.py',))
            observed = json.loads((staged / 'proof.json').read_text()) if base_sha else None
            trusted = module._git(staged, 'write-tree', '--prefix=trusted/').strip()
            result = ('result', {'tree': tree_id, 'proof': observed, 'trusted': trusted})
        except Crash:
            result = ('crash',)
    return recording.finish(result)


REFUSALS_CHECKED = False


def equivalent(before, after, context):
    if before != after:
        def first(old, new, path=()):
            if type(old) is type(new) and isinstance(old, (list, tuple)):
                for index, (left, right) in enumerate(zip(old, new)):
                    if left != right: return first(left, right, (*path, index))
                return (path, 'length', len(old), len(new))
            return (path, old, new)
        raise AssertionError((context, str(first(before, after))[:1600]))


def compare_crashes(function, before_module, after_module, schedule):
    """Interrupt both sides immediately before/after every mutation in the trace."""
    def run(module, crash):
        with tempfile.TemporaryDirectory(prefix='harness-extraction-') as name:
            return function(module, Path(name), {**schedule, 'crash':crash})
    old = run(before_module, -1)
    new = run(after_module, -1)
    equivalent(old, new, ('complete schedule changed', function.__name__, schedule))
    for crash in range(old[3]):
        before, after = run(before_module, crash), run(after_module, crash)
        equivalent(before, after, ('crash schedule changed', function.__name__, schedule, crash))
    return old



def verify_crash_cut(schedule):
    """#823 review-r2: capture unflushed bytes before context-manager cleanup."""
    example = {**schedule, 'initial':0, 'reads':[0], 'selector_reads':[True, True],
               'selectors':['t.py'], 'mode':'run_current', 'crash':-1, 'base':False, 'reattach':False}
    for function, module, write in [(action, test_tool, 'write'), (lifecycle, tests_tool, 'stage-write')]:
        with tempfile.TemporaryDirectory(prefix='harness-cut-') as name:
            complete = function(module, Path(name), example)
        if function is action:
            for index, call in enumerate(complete[0]):
                if call == ('fsync',):
                    assert complete[0][index - 1] == ('after', ('flush',)), 'progress fsync without preceding flush'
        index = next(i for i, call in enumerate(complete[0]) if call[0] == write)
        cut = 2 * sum(call[0] == 'after' for call in complete[0][:index]) + 1
        with tempfile.TemporaryDirectory(prefix='harness-cut-') as name:
            crashed = function(module, Path(name), {**example, 'crash':cut})
        assert crashed[0][-1] == ('after', complete[0][index]), 'cleanup entered the captured crash trace'
        path = 'progress.jsonl' if write == 'write' else complete[0][index][1]
        persisted = crashed[2][path]
        if write == 'stage-write': persisted = persisted[0]
        assert persisted == '', ('crash cleanup published unflushed bytes', function.__name__, path, persisted)


def decide(schedule):
    global REFUSALS_CHECKED
    # The fixed below-cap counterexample must be compared before the random paths.
    if not REFUSALS_CHECKED:
        verify_crash_cut(schedule)
        example = {**schedule, 'reads':[1] * 21, 'selector_reads':[1, 1],
                   'selectors':['t.py'], 'repeat':2, 'mode':'run', 'crash':-1, 'empty_messages':True}
        with tempfile.TemporaryDirectory(prefix='harness-repeat-') as name:
            old = action(legacy_action, Path(name), example)
            new = action(test_tool, Path(name), example)
        equivalent(old, new, 'legacy run-list cap changed')
        vector = json.loads((REFERENCE / 'repeated-run.json').read_text())
        assert old[1] == ('result', vector['action'])
        before = legacy_summary(old[1][1])
        assert before == vector['worker'], ('recorded worker parity vector changed', before)
        compare_consumers(old[1][1], new[1][1], 'repeated-run vector', reference=vector['worker'])
    boundaries = 0
    coverage = set()
    for mode in ('run_current', 'run', 'main'):
        selected_schedule = {**schedule, 'mode':mode}
        if mode == 'main':
            selected_schedule.update(selector_reads=[True, True], reads=schedule['reads'] or [0])
        observed = compare_crashes(action, legacy_action, test_tool, selected_schedule)
        boundaries += observed[3]
        coverage.update(call[0] for call in observed[0])
    for base in (False, True):
        observed = compare_crashes(lifecycle, legacy_worker, tests_tool, {**schedule, 'base':base})
        boundaries += observed[3]
        coverage.update(call[0] for call in observed[0])
    compare_consumers(*summary_facts(schedule), ('default summary', schedule))
    for size in ([] if REFUSALS_CHECKED else [14_000, 14_500, 20_000, 750_000, 800_000, 1_050_000]):
        compare_consumers(*summary_facts({**schedule, 'proof_bytes':size}), ('proof size', size))
    REFUSALS_CHECKED = True
    return {'equal':True, 'crash_boundaries':boundaries, 'coverage':sorted(coverage)}


if __name__ == '__main__':
    for line in sys.stdin:
        print(json.dumps(decide(json.loads(line))), flush=True)
