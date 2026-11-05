# ruff: noqa: F811
"""Q54: extraction preserves checker, Git samples and durable save ordering."""
from copy import deepcopy
import random
import json
from pathlib import Path
from types import SimpleNamespace

from langchain_core.messages import AIMessage
import pytest

from tl3_workers import runtime
from tl3_workers.roles import maker
from maker_reply_cases import reference
from message_invariants import DONE, maker_request  # noqa: F401


class Crash(Exception):
    pass


@pytest.mark.parametrize("seed", range(21))
def test_checker_and_handoff_preserve_the_io_schedule(maker_request, monkeypatch, tmp_path, seed):
    old = SimpleNamespace(**reference())
    # Execute the verbatim old run with the exact same injected I/O boundary.
    old.run = old.__dict__["run"]
    (tmp_path / "session.head").write_text('{"generation":0}')
    traces = []
    for implementation in (old, maker):
        events, persisted = [], {}
        request = deepcopy(maker_request)
        request["session_file"] = str(tmp_path / "session.json")
        request["inputs"]["check_base_sha"] = "a" * 40
        # Independently changing read results, repair outcomes, and each durable
        # write's before/after crash point. No clock or real Git can hide a move.
        replies = [deepcopy(DONE)]
        if seed % 3 == 1:
            replies.insert(0, {**deepcopy(DONE), "self_check": []})
        if seed % 3 == 2:
            replies = [{"kind": "cant_meet", "report": "needs work"}]
        random_reads = random.Random(seed)
        reads = iter(random_reads.randrange(65536) for _ in range(100))
        write_index = 0
        crash = seed // 3
        crash_write = (crash + 1) // 2
        crash_after = crash > 0 and crash % 2 == 0

        class Agent:
            def stream(self, *args, **kwargs):
                events.append(["model", next(reads)])
                value = replies.pop(0)
                yield {"model": {"messages": [AIMessage(content=json.dumps(value))]}}

        class Evidence:
            def state(self):
                events.append(["evidence.state", next(reads)])
                return {"test_runs": [], "environment_failures": [], "sample": next(reads)}

            def test_evidence(self):
                events.append(["evidence.test_evidence", next(reads)])
                return []

        def build(*args, **kwargs):
            kwargs["control"].allowance = None
            return Agent()

        def save(path, value):
            nonlocal write_index
            data = json.dumps(value, ensure_ascii=False, sort_keys=True).encode()
            events.append(["save", Path(path).suffix, data])
            write_index += 1
            if write_index == crash_write and not crash_after:
                raise Crash()
            persisted[str(path)] = data
            if write_index == crash_write and crash_after:
                raise Crash()

        def starting(request, session):
            events.append(["starting", next(reads)])
            return f"base-{next(reads)}", None, None

        def read_json(text):
            events.append(["read_json", next(reads)])
            return {"generation": next(reads)}

        def commit(*args):
            events.append(["commit", next(reads)])
            return f"{next(reads):040x}"

        def tree(*args):
            sample = next(reads)
            events.append(["tree", sample])
            return f"tree-{sample}"

        globals_ = implementation.run.__globals__
        original_check = globals_["_check_reply"]
        original_handoff = globals_["_checking_handoff"]

        def check(inputs, reply):
            events.append(["check", deepcopy(reply)])
            return original_check(inputs, reply)

        def handoff(inputs, head, tree):
            events.append(["handoff", head, tree])
            return original_handoff(inputs, head, tree)

        with monkeypatch.context() as patch:
            patch.setattr(runtime, "build_agent", build)
            for name, value in {"save": save, "_starting_state": starting,
                                "read_json": read_json, "_load_visit": lambda *args: None,
                                "_commit": commit, "_tree": tree,
                                "_check_reply": check, "_checking_handoff": handoff}.items():
                patch.setitem(globals_, name, value)
            patch.setattr(maker.retro, "run", lambda *args: events.append(["retro"]))
            worktree = SimpleNamespace(evidence=Evidence())
            try:
                result = implementation.run(object(), request, worktree,
                                             lambda event: events.append(["emit", event]))
            except Crash:
                result = "crashed"
        traces.append((events, persisted, result))
    assert traces[0] == traces[1]
