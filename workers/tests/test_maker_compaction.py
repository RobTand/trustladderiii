"""Repeated maker compaction and recovery through the real worker graph."""

from copy import deepcopy
import json
from pathlib import Path

from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage, ToolMessage
import pytest

from tl3_workers import runtime
from tl3_workers.contracts import validate
from message_invariants import assert_tool_pairs
import message_invariants


UNFINISHED = {"kind": "cant_meet", "report": "Final checks remain unfinished."}
maker_request = message_invariants.maker_request


class Killed(BaseException):
    """An interruption that worker error handlers cannot catch."""


class ContextModel(FakeMessagesListChatModel):
    steps: list
    calls: list = []
    offered: bool = False

    def bind_tools(self, tools, **kwargs):
        self.offered = bool(tools) and kwargs.get("tool_choice") != "none"
        return self

    def _generate(self, messages, **kwargs):
        self.calls.append((deepcopy(messages), self.offered))
        self.offered = False
        step = self.steps.pop(0)
        if isinstance(step, BaseException):
            raise step
        self.responses, self.i = [step], 0
        return super()._generate(messages, **kwargs)


def install(monkeypatch, steps):
    model = ContextModel(responses=[], steps=list(steps), calls=[])
    monkeypatch.setattr(runtime, "create_model", lambda _: model)
    return model


def capture_budget(monkeypatch):
    budgets = []
    original = runtime.ExplorationMiddleware

    def construct(*args, **kwargs):
        budget = original(*args, **kwargs)
        budgets.append(budget)
        return budget

    monkeypatch.setattr(runtime, "ExplorationMiddleware", construct)
    return budgets


def reply(value, *, input_tokens=100):
    return AIMessage(**message_invariants.turn(value, input_tokens=input_tokens))


def finish_requested(messages):
    return any("budget is exhausted" in m.text for m in messages if m.type == "human")


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
@pytest.mark.durability
def test_recovered_maker_recompacts_and_resumes(maker_request, tmp_path, monkeypatch, legacy_dimension):
    maker_request["session_file"] = str(tmp_path / "maker.json")
    maker_request["resume_session"] = False
    visit = Path(maker_request["session_file"]).with_suffix(".visit")
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
    maker_request["operation"]["generation"] += 1
    model = install(monkeypatch, [reply(UNFINISHED, input_tokens=100)])
    events = []
    runtime.run(maker_request, events.append)
    assert model.calls[0][1], "a recovered maker below its limits must retain tools"
    assert not finish_requested(model.calls[0][0])
    notice = next(m for m in reversed(model.calls[0][0]) if m.text.startswith("TL3 execution budget: "))
    assert json.loads(notice.text.removeprefix("TL3 execution budget: "))["phase"] == "working"
    assert "budget_exhaustion" not in events[-1]["reply"]
    assert budgets[-1].spent == spent + 100
    assert budgets[-1].state().get("compaction_count") == 2
    assert Path(maker_request["worktree"], "greeting.py").read_text().startswith("# edit 9")
    assert_tool_pairs(model.calls[0][0])


@pytest.mark.parametrize("invalid", [{"spent": -1}, {"compaction_count": 1},
                                    {"no_progress_turns": 1000}])
def test_invalid_allowance_checkpoint_starts_visit_afresh(maker_request, tmp_path, monkeypatch, invalid):
    """#780 recovery: a decided-invalid checkpoint must not wedge every retry."""
    maker_request["session_file"] = str(tmp_path / "maker.json")
    budgets = capture_budget(monkeypatch)
    install(monkeypatch, [edits(1)[0], Killed()])
    with pytest.raises(Killed):
        runtime.run(maker_request, lambda _: None)
    visit = Path(maker_request["session_file"]).with_suffix(".visit")
    saved = json.loads(visit.read_text())
    saved["allowance"].update(invalid)
    visit.write_text(json.dumps(saved))
    maker_request["operation"]["generation"] += 1
    model = install(monkeypatch, [reply(UNFINISHED)])
    events = []
    runtime.run(maker_request, events.append)
    notice = next(m for m in model.calls[0][0] if m.text.startswith("TL3 execution budget: "))
    assert json.loads(notice.text.removeprefix("TL3 execution budget: "))["tool_turns"]["used"] == 0
    assert budgets[-1].spent == 100
    assert events[-1]["event"] == "reply"
