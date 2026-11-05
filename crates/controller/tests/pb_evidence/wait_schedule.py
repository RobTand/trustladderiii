"""Compare external I/O schedules while the Rust parent answers checker calls."""
import ast
from contextlib import ExitStack
import inspect
from copy import deepcopy
import hashlib
import importlib.util
import io
import itertools
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

from tl3_workers import pb_tests as new
from tl3_workers import pb_tests as pb_evidence

spec = importlib.util.spec_from_file_location('pb_tests_before', sys.argv[1])
old = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = old
spec.loader.exec_module(old)


def checker(command, document, **kwargs):
    assert "PRIVATE_NOT_ON_WIRE" not in json.dumps(document)
    print(json.dumps({'request': {'operation': command[1], **document}}), flush=True)
    return json.loads(sys.stdin.readline())


for name in ('_stage', '_git'):
    assert ast.dump(ast.parse(inspect.getsource(getattr(old, name)))) == ast.dump(ast.parse(inspect.getsource(getattr(new, name)))), name
pb_evidence.run_check = checker
key = 'a' * 64
image = 'sha256:' + 'b' * 64
summary = {'status': 'completed', 'tests': [], 'totals': dict(passed=0, failed=0, error=0, skipped=0), 'note': ''}


def execute(module, directory, waits, ending_kind, base, forcing, crash):
    events, durable = [], {}
    selection = ['a.py::test']
    proof = {'mode': 'head-observer'} if base else None
    result = deepcopy(summary)
    if base:
        result['proof'] = proof
    if forcing:
        result.update(forcing={'symptom': 'forced'}, selectors=selection)
    if ending_kind == 'proof_mismatch':
        result['proof'] = {'mode': 'other'}
    if ending_kind == 'forcing_mismatch':
        result['selectors'] = ['other.py::test']
    raw = ('TL3_TEST_SUMMARY ' + json.dumps(result) + '\n').encode()
    ending = {'action_key': key, 'status': 'executed', 'exit_code': 0, 'succeeded': True,
              'result': {'verified': True, 'receipt_sha256': 'b' * 64,
                         'stdout': {'path': 'action.log', 'bytes': len(raw), 'sha256': hashlib.sha256(raw).hexdigest()}}}
    if ending_kind == 'wrong_key':
        ending['action_key'] = 'c' * 64
    if ending_kind == 'unverified':
        ending['result']['verified'] = False
    if ending_kind in ('failed', 'oom'):
        ending.update(status='failed', exit_code=137, succeeded=False, publication={'published_unix': 4})
        if ending_kind == 'oom':
            ending['reason'] = 'memory_limit_oom'
    polls = 0

    def command(args):
        nonlocal polls
        if args[1].endswith('pbrun.py'):
            # The staging directory is fixed for both implementations.
            events.append(('submit', args))
            return 0, {'schema': 'prismabuild.submission.v1', 'action_key': key}
        polls += 1
        events.append(('wait', polls, args))
        if polls <= waits:
            # Values really change between reads, including a terminal-looking
            # incomplete observation before the authoritative complete ending.
            status = ['waiting', 'executed', 'cache_hit'][(polls - 1) % 3]
            return 124, {'schema': 'prismabuild.endings.v1', 'complete': False,
                         'endings': [{'action_key': key, 'status': status}]}
        return 0, {'schema': 'prismabuild.endings.v1', 'complete': True, 'endings': [deepcopy(ending)]}

    def stage(tree, root, sha, **kwargs):
        kwargs = {**kwargs, 'forcing': kwargs['forcing'].as_json() if kwargs.get('forcing') else None}
        events.append(('stage', sha, kwargs))
        durable['stage'] = 'tree'
        if crash == 'stage':
            raise OSError('crash at stage')
        return 'tree'

    def git(root, *args):
        events.append(('git', args))
        if crash == 'git':
            raise OSError('crash at trusted read')
        return 'trusted\n'

    def submitted(key, base, tree):
        events.append(('submitted', key, base, tree))
        durable['checkpoint'] = [key, base, tree]
        if crash == 'checkpoint':
            raise OSError('crash after checkpoint')

    class Stream(io.BytesIO):
        def read(self, length=-1):
            events.append(('read', self.name, length))
            if crash == 'log':
                raise OSError('crash at log read')
            return super().read(length)

    def opened(path, mode='r', **kwargs):
        name = 'failed.json' if str(path).endswith('.json') else str(path)
        events.append(('open', name, mode))
        record = {'action_key': key, 'published_unix': 4, 'detail': {'termination_reason': 'memory_limit_oom',
                  'argv': ['PRIVATE_NOT_ON_WIRE'], 'env': {'PRIVATE': 'NOT_ON_WIRE'}}}
        stream = Stream(json.dumps(record).encode() if name == 'failed.json' else raw)
        stream.name = name
        return stream

    def read_proof(path, **kwargs):
        events.append(('read_text', path.name))
        return json.dumps(proof)

    settings = module.TestSettings(image, directory / 'tools')
    tree = SimpleNamespace(check_base_sha='d' * 40, test_checkout=directory, pb_submitted=submitted)
    projected_forcing = SimpleNamespace(as_json=lambda: {'symptom': 'forced'}) if forcing else None
    with ExitStack() as stack:
        stack.enter_context(patch.object(module, '_stage', stage))
        stack.enter_context(patch.object(module, '_git', git))
        stack.enter_context(patch.object(module, '_json_command', command))
        stack.enter_context(patch.object(module, 'scratch_environment', lambda: {}))
        stack.enter_context(patch.object(module.tempfile, 'TemporaryDirectory', lambda **kwargs: io_context(str(directory))))
        stack.enter_context(patch('builtins.open', opened))
        stack.enter_context(patch.object(Path, 'open', opened))
        stack.enter_context(patch.object(Path, 'read_text', read_proof))
        tool = module.make_test_tool(tree, settings, emit=lambda event: events.append(('emit', event)), forcing=projected_forcing, declared_symptom={'exception':'TimeoutError'} if ending_kind == 'symptom_mismatch' else None)
        observed = tool(selection, base=base)
    return events, durable, observed, getattr(tree, 'pb_identity', None)


class io_context:
    def __init__(self, value):
        self.value = value

    def __enter__(self):
        return self.value

    def __exit__(self, *_):
        pass


count = 0
with tempfile.TemporaryDirectory() as directory:
    directory = Path(directory)
    schedules = itertools.product(range(4), ['valid', 'wrong_key', 'unverified', 'failed', 'oom', 'proof_mismatch', 'forcing_mismatch', 'symptom_mismatch'],
                                  [False, True], [False, True], [None, 'stage', 'git', 'checkpoint', 'log'])
    schedules = list(schedules) + json.loads(sys.argv[2])
    for schedule in schedules:
        before = execute(old, directory, *schedule)
        after = execute(new, directory, *schedule)
        assert before == after, (schedule, before, after)
        count += 1
# An unmatched record must not inspect its malformed detail; a matching one
# retains the original escaping AttributeError. Private object fields stay local.
for matches in (False, True):
    record = {'action_key': key if matches else 'x' * 64, 'published_unix': 4, 'detail': 7}
    ending = {'action_key': key, 'status': 'failed', 'publication': {'published_unix': 4}}
    def failure(module):
        with patch.object(Path, 'open', lambda *args, **kwargs: io.BytesIO(json.dumps(record).encode())):
            try:
                return {'return': module._failure_receipt(ending, SimpleNamespace(tools_path=Path('tools')))}
            except AttributeError as error:
                return {'raise': type(error).__name__, 'message': str(error)}
    assert failure(old) == failure(new)
print(json.dumps({'done': count}), flush=True)
