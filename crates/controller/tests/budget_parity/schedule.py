"""Recorded I/O interface shared by the verbatim base and extracted shells."""
from contextlib import nullcontext
from dataclasses import dataclass, replace
from copy import deepcopy
import json
from types import SimpleNamespace
from unittest.mock import patch

from langchain_core.messages import AIMessage, HumanMessage
from tl3_workers import exploration
from tl3_workers.roles import maker


class Crash(BaseException):
    pass


class IO:
    def __init__(self, schedule, crash):
        self.schedule, self.crash = schedule, crash
        self.calls, self.persisted, self.reads, self.writes = [], {}, 0, 0

    def read(self, name):
        self.calls.append(["read", name])
        result = self.schedule[self.reads % len(self.schedule)]
        self.reads += 1
        return result

    def write(self, name, value):
        self.calls.append(["write", name, value])
        self.writes += 1
        if self.writes == self.crash:
            raise Crash()
        self.persisted[name] = json.dumps(value, ensure_ascii=False)


@dataclass
class Request:
    messages: list
    system_message: object = None
    tools: tuple = ()
    tool_choice: object = None

    def override(self, **kwargs):
        return replace(self, **kwargs)


def messages(values):
    return [value.model_dump() for value in values]


def loop(module, schedule, crash):
    io = IO(schedule, crash)
    budget = module.ExplorationBudget(tool_turns=4, finalization_turns=1, input_tokens=400_000,
                                      history_bytes=2048, no_progress_window=3)
    allowance = module.ExplorationMiddleware(budget, "maker", 150_000)
    source = [HumanMessage(content="unchanged inputs")]

    def estimate(request):
        io.calls.append(["estimate", messages(request.messages), list(request.tools)])
        return io.read("input_estimate") * 3

    def handler(request):
        io.calls.append(["model", messages(request.messages), request.tool_choice])
        io.write("provider-request", messages(request.messages))
        count = io.read("provider-usage") * 2
        return SimpleNamespace(result=[AIMessage(content="result", usage_metadata={
            "input_tokens": count, "output_tokens": 2, "total_tokens": count + 2,
            "input_token_details": {"cache_read": count // 2}})])

    outcome = None
    try:
        with patch.object(module, "input_estimate", estimate):
            for _ in schedule[:4]:
                response = allowance.wrap_model_call(Request(list(source)), handler)
                source.extend(response.result)
                io.write("checkpoint", allowance.state())
                allowance.observe_tool("read", {"path": "f"}, str(io.read("file")), fresh=True)
                # Restore invokes the same persisted checkpoint, without resetting
                # accounting. Every write may crash before changing its bytes.
                state = allowance.state()
                restored = module.ExplorationMiddleware(budget, "maker", 150_000)
                restored.restore(deepcopy(state))
                allowance = restored
    except Crash:
        outcome = "crash"
    except module.BudgetUnavailable:
        outcome = "budget_unavailable"
    return io.calls, io.persisted, outcome


def exhausted(module, schedule, crash):
    io = IO(schedule, crash)
    request = {"operation": {"node": 1, "generation": 2, "kind": "make"}, "inputs": {
        "goals": [{"id": "g", "description": "preserve behavior"}],
        "plan": {"checks": [{"name": "final check", "tests": ["test.py"]}]}}}
    evidence = [{"base": False, "tree": "tree", "status": "completed", "totals": {"passed": 1}, "tests": ["test.py"]}]

    def git(_request, *args, **kwargs):
        io.calls.append(["git", list(args), bool(kwargs.get("index"))])
        if args[0] in {"read-tree", "add"}:
            io.write("index", args[0])
            value = ""
        else:
            n = io.read("git-" + args[0])
            value = "tree" if args[0] == "write-tree" else "head" + str(n) if args[0] == "rev-parse" else "changed.py\n" if n % 2 else ""
        return SimpleNamespace(stdout=value)

    def test_evidence():
        return evidence if io.read("evidence") % 2 else []

    def generator_tool(tree, restore_only):
        assert restore_only
        def run():
            receipt = {"status": "completed", "output_tree": "tree"}
            io.write("generation-receipt", receipt)
            return receipt
        return run

    worktree = SimpleNamespace(generator=bool(schedule[0] % 2), generation_receipt={},
                               evidence=SimpleNamespace(test_evidence=test_evidence), _mutation_lock=nullcontext())
    remaining = {"tool_turns": {"used": 4, "allowed": 4}, "input_tokens": {"used": 100, "allowed": 300000},
                 "no_progress_window": 3, "no_progress_turns": 0, "last_progress_kind": "read"}
    def sample_remaining():
        io.read("remaining")
        return remaining
    allowance = SimpleNamespace(remaining=sample_remaining, exhaustion="backstop", compaction_count=0, context_fraction=0.2)
    reply = {"kind": "done", "self_check": []} if schedule[-1] % 2 else {"kind": "cant_meet", "report": "unfinished"}
    outcome = None
    try:
        with patch.object(module, "_git", git), patch.object(module, "make_generator_tool", generator_tool):
            outcome = module._budget_reply(request, allowance, [HumanMessage(content="inputs")], worktree, reply)
            io.write("reply", outcome)
    except Crash:
        outcome = "crash"
    return io.calls, io.persisted, outcome


def compare(old, schedules):
    for schedule in schedules:
        for exercise, reference, current in [
            (loop, old["exploration"], exploration),
            (exhausted, old["maker"], maker),
        ]:
            expected = exercise(reference, schedule, 0)
            assert expected == exercise(current, schedule, 0), (exercise.__name__, schedule, 0)
            writes = sum(call[0] == "write" for call in expected[0])
            for crash in range(1, writes + 1):
                assert exercise(reference, schedule, crash) == exercise(current, schedule, crash), (
                    exercise.__name__, schedule, crash)
    print("recorded I/O schedules and every write crash matched", len(schedules))
