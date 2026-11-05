"""Run the frozen and extracted main against the same clock and runtime schedule."""
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
from tl3_workers import __main__ as current, facts, quota, runtime  # noqa: E402
from tl3_workers.reply_check import run_check  # noqa: E402


def execute(module, legacy, schedule, root):
    output, errors = io.BytesIO(), io.StringIO()
    calls, persisted = [], {}
    request = request_doc.__wrapped__(root)
    stdin = io.TextIOWrapper(io.BytesIO(json.dumps(request).encode()))
    response = httpx.Response(429, headers={"x-ratelimit-reset": schedule["reset"]},
                              request=httpx.Request("POST", "http://proxy.invalid"))
    error = openai.APIStatusError("private", response=response, body=None)

    class Os(_EntryOs):
        def chdir(self, path):
            calls.append("chdir")

    def sampled():
        calls.append("clock")
        return schedule["sample"]

    def run(request, emit):
        calls.append("runtime")
        persisted["memory"] = b'{"status":"failed"}'
        raise error

    event = quota_reference.event if legacy else quota.event
    with patch.object(module, "os", Os(output)), patch.object(module.time, "time", sampled), \
         patch.object(runtime, "run", run), patch.object(quota, "event", event), \
         patch.object(sys, "argv", ["tl3_workers", "run"]), patch.object(sys, "stdin", stdin), \
         patch.object(sys, "stderr", errors), patch.object(sys, "stdout", errors):
        status = module.main()
    events = [json.loads(line) for line in output.getvalue().splitlines()]
    for document in events:
        if "facts" in document:
            facts = document.pop("facts")
            document["reset_ms"] = run_check(["check-quota"], {"kind": "ending", "facts": facts})["reset_ms"]
    return status, calls, persisted, events, errors.getvalue()


class Crash(BaseException):
    pass


def completion(legacy, schedule, crash):
    calls, persisted = [], {}
    ticks = iter([1_000_000_000_000, 1_000_500_000_000])
    writes = 0

    class Capture:
        def __init__(self, request):
            pass

        def save(self, outcome, finished=None, failure=None):
            nonlocal writes
            calls.append(('save', outcome))
            persisted['completion'] = json.dumps([outcome, finished, failure], sort_keys=True).encode()
            writes += 1
            if writes == crash:
                raise Crash()

    def clock_ns():
        calls.append('clock-ns')
        return next(ticks)

    def sampled():
        calls.append('quota-clock')
        return schedule['sample']

    request = {'role': 'maker', 'model': {'base_url': 'http://proxy.invalid'}}
    response = httpx.Response(429, headers={'x-ratelimit-reset': schedule['reset']},
                              request=httpx.Request('POST', 'http://proxy.invalid'))
    error = openai.APIStatusError('private', response=response, body=None)
    ending = quota_reference.ending_ms if legacy else quota.ending_ms
    with patch.object(facts, 'Facts', Capture), patch.object(facts.time, 'time_ns', clock_ns), \
         patch.object(facts.time, 'time', sampled), patch.object(quota, 'ending_ms', ending):
        try:
            with facts.observe(request):
                calls.append('body')
                raise error
        except BaseException as caught:
            outcome = type(caught).__name__
    return calls, persisted, outcome


for schedule in json.load(sys.stdin):
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        before = execute(main_reference, True, schedule, root)
        after = execute(current, False, schedule, root)
        assert before == after, (schedule, before, after)
        assert after[1].count("clock") == 1
    for crash in range(3):
        assert completion(True, schedule, crash) == completion(False, schedule, crash), (schedule, crash)
print("quota schedules preserve the original sample and stale-reset boundary")
