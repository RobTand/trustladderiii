"""Recorded I/O interface shared by the verbatim base and extracted shells."""
from contextlib import ExitStack, nullcontext
from dataclasses import dataclass, replace
from pathlib import PurePosixPath
import os
from copy import deepcopy
import json
from types import SimpleNamespace
from unittest.mock import patch

from langchain_core.messages import AIMessage, HumanMessage, ToolMessage
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


class File:
    """The buffered text-file interface used by the actual session.save."""
    def __init__(self, io, path):
        self.io, self.path, self.buffer = io, path, ""
        self.chunks = 0
        self.descriptor = len(io.handles) + 10
        io.handles[self.descriptor] = self
        io.files[path] = ""

    def __enter__(self):
        return self

    def __exit__(self, *_):
        if not self.io.dead:
            self.io.calls.append(["close", self.path])

    def write(self, text):
        self.io.calls.append(["write_chunk", self.path, text])
        self.buffer += text
        self.chunks += 1
        if self.chunks in {1, 1 + self.io.schedule[0] % 200}:
            self.io.stage("partial_write", self.path)
        return len(text)

    def flush(self):
        # json.dump's chunks are buffered writes. Crash once the write completes,
        # before flush; record every chunk as well, so partial writes cannot hide.
        self.io.stage("write", self.path)
        self.io.files[self.path] = self.buffer
        self.io.stage("flush", self.path)

    def fileno(self):
        self.io.calls.append(["fileno", self.path])
        return self.descriptor


class IOPath:
    def __init__(self, io, path):
        self.io, self.path = io, PurePosixPath(path)

    def __str__(self):
        return str(self.path)

    def __eq__(self, other):
        return isinstance(other, IOPath) and self.path == other.path

    def __bool__(self):
        return True

    @property
    def parent(self):
        return IOPath(self.io, self.path.parent)

    def with_suffix(self, suffix):
        return IOPath(self.io, self.path.with_suffix(suffix))

    def resolve(self):
        self.io.calls.append(["resolve", str(self)])
        return self

    def mkdir(self, **kwargs):
        self.io.calls.append(["mkdir", str(self), kwargs])

    def exists(self):
        self.io.calls.append(["exists", str(self)])
        return str(self) in self.io.files

    def read_text(self):
        self.io.calls.append(["read_text", str(self)])
        return self.io.files[str(self)]

    def open(self, mode):
        assert mode == "w"
        self.io.calls.append(["open", str(self), mode])
        return File(self.io, str(self))


class Filesystem(IO):
    def __init__(self, schedule, crash):
        super().__init__(schedule, crash)
        self.files, self.durable, self.namespace, self.handles = {}, {}, {}, {}
        self.stages, self.dead = [], False
        self.os = SimpleNamespace(open=self.open, fsync=self.fsync, replace=self.rename,
                                  close=self.close, O_RDONLY=os.O_RDONLY, O_DIRECTORY=os.O_DIRECTORY)

    def stage(self, kind, path):
        self.calls.append([kind, path])
        self.stages.append((kind, path))
        if len(self.stages) == self.crash:
            self.dead = True
            raise Crash()

    def open(self, path, flags):
        self.calls.append(["open_directory", str(path), flags])
        return 1

    def fsync(self, descriptor):
        if descriptor == 1:
            self.namespace = deepcopy(self.durable)
            self.stage("directory_fsync", "/visit")
        else:
            handle = self.handles[descriptor]
            self.durable[handle.path] = self.files[handle.path]
            self.stage("fsync", handle.path)

    def rename(self, source, destination):
        source, destination = str(source), str(destination)
        self.files[destination] = self.files.pop(source)
        if source in self.durable:
            self.durable[destination] = self.durable.pop(source)
        self.stage("rename", destination)

    def close(self, descriptor):
        if not self.dead:
            self.calls.append(["close_directory", descriptor])

    def snapshot(self):
        return {"visible": self.files, "fsynced": self.durable, "directory_synced": self.namespace,
                "buffered": {h.path: h.buffer for h in self.handles.values()}}


def loop(modules, schedule, crash):
    """Run the real maker.run closure and session.save, rather than simulating it."""
    from tl3_workers import runtime
    io = Filesystem(schedule, crash)
    module, edge, session = modules
    budget = edge.ExplorationBudget(tool_turns=8, finalization_turns=1, input_tokens=1_000_000,
                                   history_bytes=1024, no_progress_window=4)
    allowance = edge.ExplorationMiddleware(budget, "maker", 100_000)
    request = {"operation": {"node": 1, "generation": 2, "kind": "make"},
               "session_file": "/visit/maker.json", "worktree": "/tree", "inputs": {
                   "brief": "Preserve the recorded evidence.", "context_size": 100_000,
                   "goals": [{"id": "g", "description": "Preserve behavior."}],
                   "tests": {"image": "sha256:" + "a" * 64, "tools_path": "/unused"},
                   "plan": {"files": ["f"], "checks": [{"name": "check", "goal_ids": ["g"],
                                                            "tests": ["test.py"]}]}}}
    evidence = SimpleNamespace(state=lambda: {"sample": io.read("evidence")},
                               restore=lambda value: io.calls.append(["restore_evidence", value]))
    worktree = SimpleNamespace(evidence=evidence, pb_identity={"fixture": True})
    original_estimate = edge.input_estimate

    def estimate(request):
        io.calls.append(["estimate", messages(request.messages), list(request.tools)])
        return original_estimate(request) + io.read("estimate") % 17

    def git(_request, *args, **kwargs):
        io.calls.append(["git", list(args), kwargs])
        value = io.read("git-" + "-".join(args))
        return SimpleNamespace(stdout="/tree" if "--show-toplevel" in args else "head" + str(value),
                               returncode=1 if "MERGE_HEAD" in args else 0)

    class Agent:
        def stream(self, source, **kwargs):
            history = list(source["messages"])
            for turn in range(3):
                def provider(model_request):
                    io.calls.append(["model", messages(model_request.messages), model_request.tool_choice])
                    n = io.read("provider-usage")
                    tool = "read" if turn == 0 else "run_tests"
                    # Both argument clipping and content-only clipping are sampled.
                    argument = "long argument café\\\"\n" * (200 if schedule[0] % 2 else 1)
                    calls = [{"name": tool, "args": {"z": argument, "path": "f"}, "id": str(turn)}]
                    content = "long visible message é\n" * 200 if turn < 2 else json.dumps(
                        {"kind": "cant_meet", "report": "Recorded fixture finished."})
                    return SimpleNamespace(result=[AIMessage(content=content, tool_calls=calls if turn < 2 else [],
                        usage_metadata={"input_tokens": 40_000 + n % 100 if turn < 2 else 100,
                                        "output_tokens": 2,
                                        "total_tokens": 40_002 + n % 100 if turn < 2 else 102})])
                response = allowance.wrap_model_call(Request(list(history)), provider)
                history.extend(response.result)
                yield {"model": {"messages": response.result}}
                if turn < 2:
                    call = response.result[-1].tool_calls[0]
                    def tool_handler(_):
                        if turn == 1:
                            worktree.pb_submitted("action" + str(io.read("submission")), False, "tree")
                        content = "long tool message é\n" * 5000 + str(io.read("file"))
                        return ToolMessage(content=content, tool_call_id=call["id"])
                    fresh = allowance.begin_tool(call["name"], call["args"])
                    result = allowance.wrap_tool_call(SimpleNamespace(tool_call=call), tool_handler)
                    allowance.observe_tool(call["name"], call["args"], result.content, fresh=fresh)
                    history.append(result)
                    yield {"tools": {"messages": [result]}}

    def build(_model, _role, _worktree, **kwargs):
        kwargs["control"].allowance = allowance
        return Agent()

    outcome = None
    try:
        with ExitStack() as stack:
            stack.enter_context(patch.object(runtime, "build_agent", build))
            stack.enter_context(patch.object(module, "Path", lambda path: IOPath(io, path)))
            stack.enter_context(patch.object(module, "_git", git))
            stack.enter_context(patch.object(module, "_check_reply", lambda *_: None))
            stack.enter_context(patch.object(module.retro, "run", lambda *_: io.calls.append(["retro"])))
            stack.enter_context(patch.object(edge, "input_estimate", estimate))
            stack.enter_context(patch.object(session, "os", io.os))
            outcome = module.run(SimpleNamespace(), request, worktree, lambda event: io.calls.append(["emit", event]))
    except Crash:
        outcome = "crash"
    except Exception as error:
        outcome = (type(error).__name__, str(error))
    if isinstance(outcome, dict):
        assert allowance.compaction_count >= 1, "fixture did not reach compaction"
    return io.calls, io.snapshot(), outcome, io.stages


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
    from tl3_workers import session
    # Verbatim reference modules import current names. Bind their original edge
    # functions explicitly so the old run really uses the old implementation.
    old["maker"].save = old["session"].save
    old["maker"].out_of_room = old["session"].out_of_room
    old["maker"].BudgetUnavailable = old["exploration"].BudgetUnavailable
    cache = {}
    original = exploration.budget_check
    def memoized(kind, **facts):
        # Only pure, actual CLI verdicts are cached across crash-prefix replays.
        key = json.dumps([kind, facts], ensure_ascii=False)
        if key not in cache:
            cache[key] = original(kind, **facts)
        return deepcopy(cache[key])
    with patch.object(exploration, "budget_check", memoized), patch.object(maker, "budget_check", memoized), \
            patch.object(session, "budget_check", memoized):
        for schedule in schedules:
            reference = (old["maker"], old["exploration"], old["session"])
            current = (maker, exploration, session)
            expected = loop(reference, schedule, 0)
            actual = loop(current, schedule, 0)
            assert isinstance(expected[2], dict) and expected[2]["kind"] == "cant_meet", expected[2]
            assert expected == actual, ("maker.run", schedule, 0)
            assert {kind for kind, _ in expected[3]} == {"partial_write", "write", "flush", "fsync", "rename", "directory_fsync"}
            for crash in range(1, len(expected[3]) + 1):
                expected_crash = loop(reference, schedule, crash)
                actual_crash = loop(current, schedule, crash)
                assert expected_crash == actual_crash, ("maker.run", schedule, crash, expected[3][crash-1])
                assert expected_crash[2] == "crash"
            expected = exhausted(old["maker"], schedule, 0)
            assert expected == exhausted(maker, schedule, 0), ("budget_reply", schedule, 0)
            for crash in range(1, sum(call[0] == "write" for call in expected[0]) + 1):
                assert exhausted(old["maker"], schedule, crash) == exhausted(maker, schedule, crash)
    print("real maker/session I/O, write/flush/fsync/rename crashes matched", len(schedules))
