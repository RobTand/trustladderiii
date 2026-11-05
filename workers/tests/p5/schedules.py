"""Differential maker I/O schedules over the real runtime, with a frozen checker."""
from contextlib import nullcontext
from copy import deepcopy
import json
import sys
from types import SimpleNamespace
from unittest.mock import patch

from tl3_workers import runtime, tracing
from tl3_workers.roles import maker
from maker_reference import _check_inputs as old_check


class Crash(BaseException):
    pass


def execute(checker, schedule):
    calls, persisted = [], {}
    writes = 0

    class Summary:
        def __init__(self, request):
            pass

        def save(self):
            nonlocal writes
            calls.append("save-running")
            persisted["memory"] = b'{"status":"running"}'
            writes += 1
            if schedule["crash"] == writes:
                raise Crash()

        def finish(self, reply, error):
            nonlocal writes
            calls.append("save-finished")
            persisted["memory"] = json.dumps(tracing.failure_tags(error), sort_keys=True).encode()
            writes += 1
            if schedule["crash"] == writes:
                raise Crash()

    def create_model(request):
        calls.append("read-key")
        if schedule["read_failure"]:
            raise PermissionError("fixture")
        return object()

    def checked(inputs):
        calls.append("check-maker")
        checker(inputs)

    request = {"role": "maker", "worktree": "/fixture", "inputs": {
        "goals": [{"id": "a"}, {"id": "a"}],
        "plan": {"files": ["a.py"], "checks": [{"goal_ids": ["a"]}]}}}
    with patch.object(runtime, "Summary", Summary), \
         patch.object(runtime, "create_model", create_model), \
         patch.object(runtime, "Worktree", lambda *args, **kwargs: nullcontext(SimpleNamespace())), \
         patch.object(tracing, "observe", lambda *args: nullcontext()), \
         patch.object(tracing, "record_reply", lambda *args: None), \
         patch.object(maker, "_check_inputs", checked):
        try:
            runtime.run(deepcopy(request), lambda event: calls.append("emit"))
        except BaseException as error:
            ending = type(error).__name__ if isinstance(error, Crash) else tracing.failure_tags(error)
    return calls, persisted, ending


def main():
    new_check = maker._check_inputs
    for schedule in json.load(sys.stdin):
        assert execute(old_check, schedule) == execute(new_check, schedule), schedule
    print("maker schedules preserve reads, writes, failure tags and persisted bytes")


if __name__ == "__main__":
    main()
