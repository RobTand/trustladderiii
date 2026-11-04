# ruff: noqa: F811

import json
from pathlib import Path
from langchain_core.messages import AIMessage
import pytest
from tl3_workers import runtime
from tl3_workers.tools import TOOL_FACTORIES
from worker_process import start_worker
from message_invariants import DONE as DONE
from message_invariants import NO_USAGE as NO_USAGE
from message_invariants import ToolModel as ToolModel
from message_invariants import git as git
from message_invariants import maker_request as maker_request
from message_invariants import tool_capable_process_model as tool_capable_process_model
from message_invariants import turn as turn
from message_invariants import write_turn as write_turn


@pytest.mark.parametrize("input_tokens,output_tokens", [(850, 0), (840, 11), (900, 30)])
def test_usage_cutoff_stops_before_pending_tool_dispatch(tmp_path, maker_request, input_tokens, output_tokens):
    root = Path(maker_request["worktree"])
    before = git(root, "rev-parse", "HEAD")
    result = start_worker(tmp_path, maker_request, [
        write_turn("must-not-exist", input_tokens=input_tokens, output_tokens=output_tokens), turn(DONE),
    ])
    assert result.exit_code == 0, result.stderr
    assert result.events == [
        {"event": "progress", "unit": "model_turn"},
        {"event": "reply", "role": "maker", "reply": {"kind": "out_of_room"}},
    ]
    assert not (root / "must-not-exist").exists()
    assert git(root, "rev-parse", "HEAD") == before


def test_cutoff_after_edit_keeps_changes_uncommitted(tmp_path, maker_request):
    root = Path(maker_request["worktree"])
    before = git(root, "rev-parse", "HEAD")
    result = start_worker(tmp_path, maker_request, [write_turn(), turn(DONE, input_tokens=810, output_tokens=50)])
    assert result.exit_code == 0, result.stderr
    assert result.events[-1]["reply"] == {"kind": "out_of_room"}
    assert git(root, "rev-parse", "HEAD") == before
    assert (root / "greeting.py").read_text() == "def greet(): return 'hello'\n"


def test_usage_is_latest_context_not_cumulative_tokens(tmp_path, maker_request):
    maker_request["inputs"]["context_size"] = 100_000
    result = start_worker(tmp_path, maker_request, [
        write_turn(input_tokens=60_000, output_tokens=100), turn(DONE, input_tokens=74_900, output_tokens=100),
    ])
    assert result.exit_code == 0, result.stderr
    assert result.events[-1]["reply"]["kind"] == "done"


@pytest.mark.parametrize("usage", NO_USAGE, ids=["missing", "zero"])
def test_usage_fallback_counts_system_prompt_before_tools(tmp_path, maker_request, usage):
    response = write_turn("must-not-exist")
    response["usage_metadata"] = usage
    # The input alone is below the cutoff. The system prompt and schema exceed it.
    assert len(json.dumps(maker_request["inputs"], ensure_ascii=False)) < 2550
    result = start_worker(tmp_path, maker_request, [response, turn(DONE)])
    assert result.exit_code == 0, result.stderr
    assert not (Path(maker_request["worktree"]) / "must-not-exist").exists()
    assert result.events == [
        {"event": "progress", "unit": "model_turn"},
        {"event": "reply", "role": "maker", "reply": {"kind": "out_of_room"}},
    ]


@pytest.mark.parametrize("usage", NO_USAGE, ids=["missing", "zero"])
def test_usage_fallback_allows_work_below_cutoff(tmp_path, maker_request, usage):
    maker_request["inputs"]["context_size"] = 100_000
    turns = [write_turn(), turn(DONE)]
    for response in turns:
        response["usage_metadata"] = usage
    result = start_worker(tmp_path, maker_request, turns)
    assert result.exit_code == 0, result.stderr
    assert result.events[-1]["reply"] == {
        **DONE, "head": git(maker_request["worktree"], "rev-parse", "HEAD"),
    }


@pytest.mark.parametrize("usage", NO_USAGE, ids=["missing", "zero"])
@pytest.mark.parametrize("source", ["history", "tool_result"])
def test_usage_fallback_includes_history_and_tool_results(maker_request, monkeypatch, usage, source):
    maker_request["inputs"]["context_size"] = 10_000
    root = Path(maker_request["worktree"])
    before = git(root, "rev-parse", "HEAD")
    if source == "history":
        first = write_turn(content="x" * 30_000)
    else:
        def factory(worktree):
            def run_tests() -> dict:
                return {"status": "completed", "details": "x" * 30_000}
            return run_tests
        monkeypatch.setitem(TOOL_FACTORIES, "run_tests", factory)
        first = turn(calls=[{"name": "run_tests", "args": {}, "id": "test", "type": "tool_call"}])
    pending = write_turn("must-not-exist")
    pending["usage_metadata"] = usage
    model = ToolModel(responses=[AIMessage(**first), AIMessage(**pending), AIMessage(**turn(DONE))])
    monkeypatch.setattr(runtime, "create_model", lambda request: model)
    events = []
    runtime.run(maker_request, events.append)
    assert events == [
        {"event": "progress", "unit": "model_turn"},
        {"event": "progress", "unit": "tool_result"},
        {"event": "progress", "unit": "model_turn"},
        {"event": "reply", "role": "maker", "reply": {"kind": "out_of_room"}},
    ]
    assert not (root / "must-not-exist").exists()
    assert git(root, "rev-parse", "HEAD") == before
