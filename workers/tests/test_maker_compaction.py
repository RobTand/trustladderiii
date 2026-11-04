"""Repeated maker compaction and recovery through the real worker graph."""

import json
from pathlib import Path

from langchain_core.messages import AIMessage, ToolMessage
import pytest

from tl3_workers import runtime
from tl3_workers.contracts import validate
from message_invariants import assert_tool_pairs
import message_invariants
from test_prompt_cache import capture_budget, finish_requested
from test_turns import Killed, install, maker_visit, reply, rerun


UNFINISHED = {"kind": "cant_meet", "report": "Final checks remain unfinished."}
maker_request = message_invariants.maker_request


def edits(count):
    return [AIMessage(content="", tool_calls=[{
        "name": "write", "args": {"path": "greeting.py", "content": f"# edit {index}\n" + "x" * 22_000},
        "id": str(index), "type": "tool_call"}], usage_metadata={
            "input_tokens": 71_000 if index == 7 else 75_000 if index == 9 else 10_000,
            "output_tokens": 1, "total_tokens": (71_000 if index == 7 else 75_000 if index == 9 else 10_000) + 1})
            for index in range(1, count + 1)]


def test_progressing_maker_compacts_twice_and_continues(maker_request, monkeypatch):
    maker_request["inputs"]["context_size"] = 100_000
    budgets = capture_budget(monkeypatch)
    model = install(monkeypatch, [*edits(9), reply(UNFINISHED)])
    events = []
    runtime.run(maker_request, events.append)
    assert not any(finish_requested(messages) for messages, _ in model.calls)
    assert "budget_exhaustion" not in events[-1]["reply"]
    assert budgets[0].state().get("compaction_count") == 2
    assert budgets[0].last_progress_kind == "write"
    for index in (7, 9):
        messages = model.calls[index][0]
        assert_tool_pairs(messages)
        assert any(isinstance(m, ToolMessage) and m.tool_call_id == str(index) for m in messages)
        assert sum(len(json.dumps(m.model_dump(), ensure_ascii=False).encode()) for m in messages
                   if isinstance(m, (AIMessage, ToolMessage))) <= budgets[0].budget.history_bytes


def test_ineffective_compaction_reports_context_tokens(maker_request, monkeypatch):
    maker_request["inputs"]["context_size"] = 100_000
    maker_request["inputs"]["brief"] += "x" * 220_000
    first = edits(1)[0]
    first.usage_metadata = {"input_tokens": 80_000, "output_tokens": 1, "total_tokens": 80_001}
    model = install(monkeypatch, [first, reply(UNFINISHED, input_tokens=80_000)])
    events = []
    runtime.run(maker_request, events.append)
    diagnostic = events[-1]["reply"].get("budget_exhaustion", {})
    assert diagnostic.get("dimension") == "context_tokens", diagnostic
    assert diagnostic["compaction_count"] == 1
    assert diagnostic["context_fraction"] >= 0.7
    assert diagnostic["input_tokens"]["used"] < diagnostic["input_tokens"]["allowed"]
    assert finish_requested(model.calls[1][0])
    validate("maker-reply", events[-1]["reply"])


@pytest.mark.parametrize("legacy_dimension", [None, "context_tokens", "input_tokens"])
def test_recovered_maker_recompacts_and_resumes(maker_request, tmp_path, monkeypatch, legacy_dimension):
    visit = maker_visit(maker_request, tmp_path)
    maker_request["inputs"]["context_size"] = 100_000
    budgets = capture_budget(monkeypatch)
    install(monkeypatch, [*edits(9), Killed()])
    with pytest.raises(Killed):
        runtime.run(maker_request, lambda _: None)
    saved = json.loads(visit.read_text())
    assert saved["allowance"]["turns"] == 9
    assert saved["allowance"]["compacted"] is True
    # Old workers could persist a context stop, including its incorrect label.
    saved["allowance"]["exhaustion"] = legacy_dimension
    visit.write_text(json.dumps(saved))
    spent = saved["allowance"]["spent"]
    model, events = rerun(maker_request, monkeypatch, [reply(UNFINISHED, input_tokens=100)])
    assert model.calls[0][1], "a recovered maker below its limits must retain tools"
    assert not finish_requested(model.calls[0][0])
    notice = next(m for m in reversed(model.calls[0][0]) if m.text.startswith("TL3 execution budget: "))
    assert json.loads(notice.text.removeprefix("TL3 execution budget: "))["phase"] == "working"
    assert "budget_exhaustion" not in events[-1]["reply"]
    assert budgets[-1].spent == spent + 100
    assert budgets[-1].state().get("compaction_count") == 2
    assert Path(maker_request["worktree"], "greeting.py").read_text().startswith("# edit 9")
    assert_tool_pairs(model.calls[0][0])
