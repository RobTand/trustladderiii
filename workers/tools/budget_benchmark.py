"""Measure the real maker loop with a scripted provider and the Bazel tl3 binary.

Run on BuildBuddy with bazel run //workers/tools:budget_benchmark. Timings include
Python serialization, process startup, Rust decisions and strict response parsing.
The provider does no network I/O. No timing threshold is an assertion.
"""
from collections import Counter
from contextlib import ExitStack
import json
import math
import os
from pathlib import Path
import platform
import statistics
import subprocess
import tempfile
import time
from unittest.mock import patch

from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage
from python.runfiles import runfiles

from tl3_workers import budget_check, exploration, forecast, runtime, session
from tl3_workers.roles import maker


class Model(FakeMessagesListChatModel):
    def bind_tools(self, tools, **kwargs):
        return self


def percentile(values, fraction):
    return sorted(values)[math.ceil(len(values) * fraction) - 1]


def main():
    os.environ["TL3_BINARY"] = runfiles.Create().Rlocation("_main/crates/controller/tl3")
    os.environ["LANGSMITH_TRACING"] = "false"
    rows, visits = [], []
    state = {"turn": -1, "record": False, "middleware": False}
    boundaries = []
    original, wrap = budget_check.check, exploration.ExplorationMiddleware.wrap_model_call

    def measured(kind, **facts):
        start = time.perf_counter_ns()
        result = original(kind, **facts)
        elapsed = (time.perf_counter_ns() - start) / 1_000_000
        if state["record"]:
            rows.append({"kind": kind, "ms": elapsed, "turn": state["turn"],
                         "middleware": state["middleware"]})
        return result

    def model_turn(self, *args, **kwargs):
        state["turn"] += 1
        begin = len(rows)
        state["middleware"] = True
        try:
            return wrap(self, *args, **kwargs)
        finally:
            state["middleware"] = False
            if state["record"]:
                calls = rows[begin:]
                assert len(calls) <= 19, calls
                assert sum(row["kind"] == "retained" for row in calls) <= 1
                boundaries.append({"turn": state["turn"], "calls": len(calls),
                                   "kinds": dict(Counter(row["kind"] for row in calls))})

    def visit(case, repeat):
        state["turn"] = -1
        begin = len(rows)
        with tempfile.TemporaryDirectory(prefix="tl3-budget-visit-") as temporary:
            root = Path(temporary)
            environment = {"PATH": os.defpath, "LANG": "C.UTF-8",
                           "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull}
            def git(*args):
                subprocess.run(["git", "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false",
                                "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", *args],
                               cwd=root, env=environment, check=True, capture_output=True)
            git("init", "--quiet", "--template=")
            (root / "sample.txt").write_text("bounded evidence\n" * (2000 if case == "compaction_many" else 1))
            git("add", ".")
            git("commit", "--quiet", "-m", "Base")
            inputs = {"brief": "Inspect the declared evidence.", "attempt": 1, "round": 0,
                      "goals": [{"id": "g", "description": "Inspect evidence.", "verification": "review"}],
                      "plan": {"files": ["sample.txt"], "checks": [
                          {"name": "inspection", "kind": "behavior-preserving", "goal_ids": ["g"],
                           "tests": ["test_sample.py::test_evidence"]}]},
                      "context_size": 150000,
                      "tests": {"image": "sha256:" + "a" * 64, "tools_path": "/unused"}}
            reply = {"kind": "cant_meet", "report": "Measurement stops after reading the evidence."}
            responses = []
            if case != "final_only":
                tokens = 110000 if case.startswith("compaction") else 1000
                calls = [{"name": "read", "args": {"path": "sample.txt"}, "id": str(i), "type": "tool_call"}
                         for i in range(16 if case == "compaction_many" else 1)]
                if case == "compaction_arguments":
                    calls = [{"name": "write", "args": {"path": "sample.txt", "content": "long argument é\n" * 6000},
                              "id": "write", "type": "tool_call"}]
                responses.append(AIMessage(content="visible message é\n" * 3000 if case.startswith("compaction") else "",
                    tool_calls=calls,
                    usage_metadata={"input_tokens": tokens, "output_tokens": 10, "total_tokens": tokens + 10}))
            responses.extend([AIMessage(content=json.dumps(reply)), AIMessage(content=json.dumps(
                {"slowed_by": [], "missing": [], "checks": [], "noise": []}))])
            events = []
            with patch.object(runtime, "create_model", lambda _: Model(responses=responses)):
                runtime.run({"role": "maker", "protocol": 1, "worktree": str(root),
                             "session_file": str(root / "session.json"), "inputs": inputs,
                             "operation": {"node": 1, "generation": 0, "kind": "make"}}, events.append)
            if events[-1]["event"] != "reply":
                raise RuntimeError("maker visit did not finish")
        if state["record"]:
            calls = rows[begin:]
            visits.append({"case": case, "repeat": repeat,
                           "calls_per_model_turn": [dict(Counter(r["kind"] for r in calls if r["turn"] == i))
                                                    for i in range(state["turn"] + 1)],
                           "milliseconds_per_model_turn": [sum(r["ms"] for r in calls if r["turn"] == i)
                                                           for i in range(state["turn"] + 1)],
                           "calls_outside_model_turns": sum(r["turn"] < 0 for r in calls)})

    with ExitStack() as stack:
        for module in (exploration, forecast, session, maker):
            stack.enter_context(patch.object(module, "budget_check", measured))
        stack.enter_context(patch.object(exploration.ExplorationMiddleware, "wrap_model_call", model_turn))
        for case in ("final_only", "read_then_final", "compaction", "compaction_arguments", "compaction_many"):
            state["record"] = False
            visit(case, -1)
            state["record"] = True
            for repeat in range(4):
                visit(case, repeat)
    values = [row["ms"] for row in rows]
    report = {"platform": platform.platform(), "units": "milliseconds_per_check_budget_call",
              "scope": "real maker visits; scripted provider; warmed schema and release-id caches",
              "samples": len(values), "p50": statistics.median(values), "p95": percentile(values, .95),
              "middleware_call_bound": 19,
              "retention_call_bound": 1,
              "boundaries": boundaries,
              "by_kind": {kind: {"calls": sum(r["kind"] == kind for r in rows),
                                  "p50": statistics.median(r["ms"] for r in rows if r["kind"] == kind),
                                  "p95": percentile([r["ms"] for r in rows if r["kind"] == kind], .95)}
                          for kind in sorted({r["kind"] for r in rows})}, "visits": visits}
    print("BUDGET_LATENCY " + json.dumps(report, sort_keys=True))


if __name__ == "__main__":
    main()
