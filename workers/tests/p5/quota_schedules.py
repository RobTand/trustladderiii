"""Frozen/current main and quota decisions through both real durable writers."""
from contextlib import ExitStack
import io
import json
from pathlib import Path
import sys
import tempfile
from unittest.mock import patch

import httpx
import openai

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from worker_process import _EntryOs, request_doc  # noqa: E402
import main_reference  # noqa: E402
import quota_reference  # noqa: E402
from durable_io import Crash, Disk  # noqa: E402
from tl3_workers import __main__ as current, facts, memory, quota, runtime, tracing  # noqa: E402
from tl3_workers.reply_check import run_check  # noqa: E402


def execute(module, legacy, schedule, crash=0):
    with tempfile.TemporaryDirectory() as directory, ExitStack() as stack:
        root = Path(directory)
        output = root / 'evidence'
        output.mkdir()
        disk = Disk(output, crash).install(stack)
        wire, errors = io.BytesIO(), io.StringIO()
        request = request_doc.__wrapped__(root)
        request['mlflow'] = None
        request['trace'] = {'completion': str(output / 'completion.json'), 'release': 'fixture'}
        request['memory'] = {'summary_file': str(output / 'summary.json'), 'repo': 'fixture',
                             'issue': 1, 'request': 'fixture', 'epoch': 0}
        stdin = io.TextIOWrapper(io.BytesIO(json.dumps(request).encode()))
        response = httpx.Response(429, headers={'x-ratelimit-reset': schedule['reset']},
                                  request=httpx.Request('POST', 'http://proxy.invalid'))
        error = openai.APIStatusError('private', response=response, body=None)
        ticks = iter([1_000_000_000_000, 1_000_500_000_000])
        samples = iter([schedule['sample'], schedule['sample'] + schedule['transport_ms'] / 1000])

        class Os(_EntryOs):
            def chdir(self, path):
                disk.note('chdir')

        def clock_ns():
            disk.note('clock-ns')
            return next(ticks)

        def sampled():
            disk.note('quota-clock')
            return next(samples)

        def body(request, emit):
            disk.note('runtime-body')
            raise error

        stack.enter_context(patch.object(module, 'os', Os(wire)))
        stack.enter_context(patch.object(facts.time, 'time_ns', clock_ns))
        stack.enter_context(patch.object(facts.time, 'time', sampled))
        # Only the external body is inert: runtime.run, Summary and Facts stay real.
        stack.enter_context(patch.object(runtime, '_run', body))
        stack.enter_context(patch.object(quota, 'event', quota_reference.event if legacy else quota.event))
        stack.enter_context(patch.object(quota, 'ending_ms', quota_reference.ending_ms if legacy else quota.ending_ms))
        stack.enter_context(patch.object(sys, 'argv', ['tl3_workers', 'run']))
        stack.enter_context(patch.object(sys, 'stdin', stdin))
        stack.enter_context(patch.object(sys, 'stderr', errors))
        stack.enter_context(patch.object(sys, 'stdout', errors))
        variables = [memory.current, facts.current, tracing.active, tracing.model_key,
                     runtime.run_prompt, runtime.run_test_cpus]
        tokens = [(var, var.set(None)) for var in variables]
        try:
            status = module.main()
        except Crash:
            status = 'Crash'
        finally:
            for var, token in reversed(tokens):
                var.reset(token)
        events = [json.loads(line) for line in wire.getvalue().splitlines()]
        for document in events:
            if 'facts' in document:
                raw = document.pop('facts')
                document['reset_ms'] = run_check(['check-quota'], {'kind': 'ending', 'facts': raw})['reset_ms']
        calls, persisted = disk.result()
        return status, calls, persisted, events, errors.getvalue(), disk.points


for index, schedule in enumerate(json.load(sys.stdin)):
    if sys.argv[1:] and index % 4 != int(sys.argv[1]):
        continue
    before = execute(main_reference, True, schedule)
    after = execute(current, False, schedule)
    assert before == after, (schedule, before, after)
    assert after[1].count(('quota-clock',)) == 2
    assert set(after[2]) == {'summary.json', 'completion.json'}
    assert json.loads(after[2]['completion.json'])['outcome'] == 'quota'
    for crash in range(1, before[5] + 1):
        before = execute(main_reference, True, schedule, crash)
        after = execute(current, False, schedule, crash)
        assert before == after, (schedule, crash, before, after)
print('quota schedules preserve actual persisted bytes and the stale-reset boundary')
