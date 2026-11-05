"""Differential maker schedules through the real summary and completion writers."""
from contextlib import ExitStack, nullcontext
from copy import deepcopy
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

from tl3_workers import facts, memory, runtime, tracing
from tl3_workers.roles import maker
from durable_io import Disk
from maker_reference import _check_inputs as old_check


def cases():
    base = {'goals': [{'id': 'a'}, {'id': 'b', 'stage': 'checking'}],
            'plan': {'files': ['a.py'], 'checks': [{'goal_ids': ['a', 'b']}]}}
    for kind in ['duplicate', 'empty', 'parent', 'git', 'absolute', 'long', 'unknown',
                 'feedback', 'maker-coverage', 'checking-coverage']:
        value = deepcopy(base)
        if kind == 'duplicate':
            value['goals'].append(value['goals'][0])
        paths = {'empty': '.', 'parent': '../a', 'git': '.git/config',
                 'absolute': '/a', 'long': 'a' * 256}
        if kind in paths:
            value['plan']['files'] = [paths[kind]]
        if kind == 'unknown':
            value['plan']['checks'][0]['goal_ids'] = ['other']
        if kind == 'feedback':
            value['feedback'] = [{'goal_ids': ['other']}]
        if kind.endswith('-coverage'):
            value['plan']['checks'][0]['goal_ids'] = ['b' if kind == 'maker-coverage' else 'a']
        yield kind, value


def execute(checker, schedule, inputs, crash=0):
    with tempfile.TemporaryDirectory() as directory, ExitStack() as stack:
        root = Path(directory)
        output = root / 'evidence'
        output.mkdir()
        key = root / 'fixture-model-key'
        key.write_text('p5-fixture-only-key')
        session = root / 'session.head'
        session.write_text('{"generation": 0}')
        disk = Disk(output, crash).install(stack)
        ticks = iter([1_000_000_000_000, 1_000_500_000_000])
        read_text = Path.read_text
        reads = session_reads = 0

        def clock_ns():
            disk.note('clock-ns')
            return next(ticks)

        def clock():
            disk.note('quota-clock')
            return 1000.0 + schedule['crash'] / 8

        def read(path, *args, **kwargs):
            nonlocal reads, session_reads
            if path == session:
                disk.note('read-session', session_reads)
                session_reads += 1
                with disk.original_open(session, 'w') as handle:
                    json.dump({'generation': session_reads + schedule['crash']}, handle)
            if path == key:
                disk.note('read-key', reads)
                reads += 1
                if schedule['read_failure']:
                    raise PermissionError('fixture read denied')
                # Only this fixture path is read. It changes if sampled twice.
                with disk.original_open(key, 'w') as handle:
                    handle.write(f'p5-fixture-only-key-{reads}')
            return read_text(path, *args, **kwargs)

        def checked(value):
            disk.note('check-maker')
            checker(value)

        request = {'role': 'maker', 'worktree': str(root), 'mlflow': None,
                   'operation': {'node': 1, 'generation': 0}, 'key_file': str(key),
                   'session_file': str(root / 'session.json'),
                   'model': {'base_url': 'http://proxy.invalid', 'name': 'fixture',
                             'effort': 'high', 'budget_tokens': 1000},
                   'trace': {'completion': str(output / 'completion.json'), 'release': 'fixture'},
                   'memory': {'summary_file': str(output / 'summary.json')}, 'inputs': inputs}
        # External model/worktree construction is inert; create_model and its key read are real.
        stack.enter_context(patch.object(runtime, 'ProxyChatOpenAI', lambda **kwargs: object()))
        stack.enter_context(patch.object(runtime, 'Worktree', lambda *args, **kwargs: nullcontext(SimpleNamespace())))
        stack.enter_context(patch.object(maker, '_check_inputs', checked))
        stack.enter_context(patch.object(Path, 'read_text', read))
        stack.enter_context(patch.object(facts.time, 'time_ns', clock_ns))
        stack.enter_context(patch.object(facts.time, 'time', clock))
        variables = [memory.current, facts.current, tracing.active, tracing.model_key,
                     runtime.run_prompt, runtime.run_test_cpus]
        tokens = [(var, var.set(None)) for var in variables]
        try:
            runtime.run(deepcopy(request), lambda event: disk.note('emit'))
        except BaseException as error:
            ending = 'Crash' if disk.frozen is not None else tracing.failure_tags(error)
        finally:
            for var, token in reversed(tokens):
                var.reset(token)
        calls, persisted = disk.result()
        return calls, persisted, ending, disk.points


def main():
    new_check = maker._check_inputs
    schedules = {json.dumps(row, sort_keys=True) for row in json.load(sys.stdin)}
    for encoded in sorted(schedules):
        schedule = json.loads(encoded)
        for kind, inputs in cases():
            if sys.argv[1:] and kind != sys.argv[1]:
                continue
            old = execute(old_check, schedule, inputs)
            new = execute(new_check, schedule, inputs)
            assert old == new, (kind, schedule, old, new)
            assert set(old[1]) == {'summary.json', 'completion.json'}
            if not schedule['read_failure']:
                assert json.loads(old[1]['completion.json'])['failure']['class'] == 'ValueError'
            # Discover cuts from actual raw writes, flushes, fsyncs and renames.
            for crash in range(1, old[3] + 1):
                before = execute(old_check, schedule, inputs, crash)
                after = execute(new_check, schedule, inputs, crash)
                assert before == after, (kind, schedule, crash, before, after)
    print('maker schedules preserve real syscall order and actual persisted bytes')


if __name__ == '__main__':
    main()
