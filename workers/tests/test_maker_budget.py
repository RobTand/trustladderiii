"""Exercise bounded maker finalization through the real graph and confined tools."""

from copy import deepcopy
from dataclasses import asdict, replace
import json
from pathlib import Path
from types import SimpleNamespace

from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage, HumanMessage
import pytest

from tl3_workers import runtime
from tl3_workers.contracts import validate
from tl3_workers.roles import maker
from message_invariants import assert_tool_pairs
import test_maker
from test_maker import DONE, git

maker_request = test_maker.maker_request


class BudgetModel(FakeMessagesListChatModel):
    calls: list = []
    offered: set = set()
    actions: list = []
    tokens: int = 100
    final_reply: dict = {"kind": "cant_meet", "report": "CHANGELOG and final checks unfinished."}

    def bind_tools(self, tools, **kwargs):
        self.offered = {tool.name for tool in tools}
        return self

    def _generate(self, messages, **kwargs):
        self.calls.append((deepcopy(messages), set(self.offered)))
        index = len(self.calls) - 1
        final = not self.offered or index == 8
        self.offered = set()
        name, args = (self.actions[index] if index < len(self.actions)
                      else ("read", {"path": "greeting.py"}))
        self.responses = [AIMessage(
            content=json.dumps(self.final_reply) if final else "",
            tool_calls=[] if final else [{"name": name, "args": args, "id": str(index), "type": "tool_call"}],
            usage_metadata={"input_tokens": self.tokens, "output_tokens": 1, "total_tokens": self.tokens + 1})]
        self.i = 0
        return super()._generate(messages, **kwargs)


def install(monkeypatch, request, *, turns=4, reserve=2, tokens=6_000_000, **model_args):
    # A structural budget keeps RED executable before the new reserve field exists.
    budget = SimpleNamespace(**{**asdict(maker.ROLE.exploration_budget), "tool_turns": turns,
                                "input_tokens": tokens, "finalization_turns": reserve})
    monkeypatch.setattr(maker, "ROLE", replace(maker.ROLE, exploration_budget=budget))
    request["inputs"]["context_size"] = 150_000
    model = BudgetModel(responses=[], calls=[], **model_args)
    monkeypatch.setattr(runtime, "create_model", lambda _: model)
    return model


def snapshots(model):
    return [json.loads(next((m.text.removeprefix("TL3 execution budget: ")
                            for m in messages if m.text.startswith("TL3 execution budget: ")), "{}"))
            for messages, _ in model.calls]


def test_each_turn_exposes_remaining_allowance(maker_request, monkeypatch):
    model = install(monkeypatch, maker_request)
    runtime.run(maker_request, lambda _: None)
    budgets = snapshots(model)
    assert [b.get("tool_turns", {}).get("remaining") for b in budgets] == [4, 3, 2, 1, 0]
    assert [b["input_tokens"]["remaining"] for b in budgets] == [6_000_000 - n * 100 for n in range(5)]
    assert [b["phase"] for b in budgets] == ["working", "working", "finalizing", "finalizing", "final"]
    assert all(b["finalization_turns"] == 2 for b in budgets)
    for messages, _ in model.calls:
        assert_tool_pairs(messages)


def test_reserve_blocks_edits_even_when_model_calls_a_hidden_tool(maker_request, monkeypatch):
    actions = [("write", {"path": "greeting.py", "content": "preserved\n"}),
               ("read", {"path": "greeting.py"}),
               ("write", {"path": "greeting.py", "content": "must not overwrite\n"}),
               ("read", {"path": "greeting.py"})]
    model = install(monkeypatch, maker_request, actions=actions)
    runtime.run(maker_request, lambda _: None)
    assert Path(maker_request["worktree"], "greeting.py").read_text() == "preserved\n"
    assert {"read", "diff", "run_tests"} <= model.calls[2][1]
    assert not {"write", "edit", "run_generator", "recall"} & model.calls[2][1]
    assert any("finalization reserve" in m.text for m in model.calls[-1][0] if m.type == "tool")
    assert len(model.calls) == 5, "four bounded tool turns and one tools-free final reply"


def test_cap_with_unfinished_work_produces_typed_preserved_diagnostic(maker_request, monkeypatch):
    root = Path(maker_request["worktree"])
    maker_request["inputs"]["plan"]["files"].append("CHANGELOG.md")
    model = install(monkeypatch, maker_request, actions=[
        ("write", {"path": "greeting.py", "content": "partial implementation\n"})])
    before = git(root, "rev-parse", "HEAD")
    events = []
    runtime.run(maker_request, events.append)
    reply = events[-1]["reply"]
    diagnostic = reply.get("budget_exhaustion", {})
    assert diagnostic.get("kind") == "execution_budget_exhausted", reply
    assert diagnostic["dimension"] == "tool_turns"
    assert diagnostic["tool_turns"] == {"used": 4, "allowed": 4}
    assert diagnostic["input_tokens"] == {"used": 500, "allowed": 6_000_000}
    assert diagnostic["operation"] == maker_request["operation"]
    assert diagnostic["preserved_work"]["head"] == before == git(root, "rev-parse", "HEAD")
    git(root, "add", "--all")
    assert diagnostic["preserved_work"]["tree"] == git(root, "write-tree")
    assert "CHANGELOG" in json.dumps(diagnostic["unfinished_obligations"])
    assert "execution_budget_exhausted" in reply["report"]
    assert not model.calls[-1][1]
    validate("maker-reply", reply)


def test_input_exhaustion_is_distinct_from_tool_turn_exhaustion(maker_request, monkeypatch):
    model = install(monkeypatch, maker_request, turns=64, tokens=300_000)
    model.tokens = 120_000
    events = []
    runtime.run(maker_request, events.append)
    diagnostic = events[-1]["reply"].get("budget_exhaustion", {})
    assert diagnostic.get("dimension") == "input_tokens", events[-1]
    assert diagnostic["tool_turns"]["used"] < 64
    assert diagnostic["input_tokens"]["used"] <= 300_000
    assert len(model.calls) <= 2


def test_restoring_allowance_does_not_restore_edit_capacity(maker_request, monkeypatch):
    install(monkeypatch, maker_request)
    original = runtime.ExplorationMiddleware
    restored = []

    def construct(*args, **kwargs):
        allowance = original(*args, **kwargs)
        state = allowance.state()
        state.update(turns=3, spent=300)
        allowance.restore(state)
        restored.append(allowance)
        return allowance

    monkeypatch.setattr(runtime, "ExplorationMiddleware", construct)
    model = BudgetModel(responses=[], calls=[], actions=[
        ("write", {"path": "greeting.py", "content": "must not overwrite\n"})])
    monkeypatch.setattr(runtime, "create_model", lambda _: model)
    events = []
    runtime.run(maker_request, events.append)
    assert len(model.calls) == 2
    assert "write" not in model.calls[0][1]
    assert snapshots(model)[0]["tool_turns"]["remaining"] == 1
    assert events[-1]["reply"]["budget_exhaustion"]["input_tokens"]["used"] == 500
    assert Path(maker_request["worktree"], "greeting.py").read_text().startswith("def greet")


def test_final_done_cannot_hide_unverified_final_files(maker_request, monkeypatch):
    install(monkeypatch, maker_request, final_reply=DONE, actions=[
        ("write", {"path": "greeting.py", "content": "def greet(): return 'hello'\n"})])
    events = []
    runtime.run(maker_request, events.append)
    reply = events[-1]["reply"]
    assert reply["kind"] == "cant_meet", reply
    assert any("final check" in item for item in reply["budget_exhaustion"]["unfinished_obligations"])


def test_planner_receives_budget_guidance_without_new_split_rule(monkeypatch, tmp_path):
    from test_planner import CANT_MEET, planner_model
    from tl3_workers.roles import ROLES

    model = planner_model(monkeypatch, CANT_MEET)
    # build_agent exposes code-owned maker capacity even when old inputs lack it.
    with pytest.MonkeyPatch.context() as patch:
        patch.setattr(runtime, "_checked", lambda role, inputs, reply, **kwargs: reply)
        runtime.run({"role": "planner", "worktree": str(tmp_path), "inputs": {}}, lambda _: None)
    text = "\n".join(m.text for m in model.calls[0][0] if isinstance(m, HumanMessage))
    assert '"maker_budget"' in text
    assert str(ROLES["maker"].exploration_budget.tool_turns) in text
    system = "\n".join(m.text for m in model.calls[0][0] if m.type == "system")
    assert "finalization reserve" in system
    assert "new split reason" in system
