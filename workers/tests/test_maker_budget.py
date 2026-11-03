"""Exercise bounded maker finalization through the real graph and confined tools."""

from copy import deepcopy
from dataclasses import asdict, replace
import json
import os
from pathlib import Path
import subprocess
from types import SimpleNamespace

from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage, HumanMessage
import pytest

from tl3_workers import runtime
from tl3_workers.contracts import validate
from tl3_workers.roles import maker
from message_invariants import assert_tool_pairs


DONE = {
    "kind": "done",
    "self_check": [
        {"goal_id": "greeting", "status": "met", "evidence": "Greeting now returns hello."},
        {"goal_id": "interface", "status": "met", "evidence": "The greet() signature is unchanged."},
    ],
}


def git(root, *args):
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    env.update({
        "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_AUTHOR_NAME": "Fixture", "GIT_AUTHOR_EMAIL": "fixture@example.invalid",
        "GIT_COMMITTER_NAME": "Fixture", "GIT_COMMITTER_EMAIL": "fixture@example.invalid",
    })
    return subprocess.run(
        ["git", "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false", *args],
        cwd=root, env=env, check=True, capture_output=True, text=True,
    ).stdout.strip()


@pytest.fixture
def maker_request(tmp_path):
    root = tmp_path / "repo"
    root.mkdir()
    git(root, "init", "--quiet", "--template=")
    (root / "greeting.py").write_text("def greet(): return 'goodbye'\n")
    git(root, "add", ".")
    git(root, "commit", "--quiet", "-m", "Base")
    return {
        "protocol": 1,
        "operation": {"node": 1, "generation": 0, "kind": "make"},
        "role": "maker",
        "model": {"base_url": "http://model.invalid/v1", "name": "fixture-model",
                  "effort": "high", "budget_tokens": 32000},
        "key_file": str(tmp_path / "unused-key"),
        "worktree": str(root),
        "inputs": {
            "brief": "Return hello without changing greet's interface.",
            "goals": [{"id": "greeting", "description": "Return hello.", "verification": "test"},
                      {"id": "interface", "description": "Keep greet's interface.", "verification": "review"}],
            "plan": {
                "files": ["greeting.py", "test_greeting.py"],
                "checks": [
                    {'proof': {'subjects': [], 'observer_support': [], 'red_message': 'expected regression'}, "name": "hello", "kind": "demonstrating",
                     "tests": ["test_greeting.py::test_hello"], "goal_ids": ["greeting"]},
                    {"name": "signature", "kind": "behavior-preserving",
                     "tests": ["test_greeting.py::test_signature"], "goal_ids": ["interface"]},
                ],
            },
            "attempt": 1, "round": 0,
            "tests": {"image": "sha256:" + "a" * 64,
                      "tools_path": "/mnt/shared/prismabuild-fleet/repo/tools"},
            "context_size": 1000,
        },
    }


class BudgetModel(FakeMessagesListChatModel):
    calls: list = []
    offered: set = set()
    actions: list = []
    tokens: int = 100
    final_reply: dict = {"kind": "cant_meet", "report": "CHANGELOG and final checks unfinished."}

    def bind_tools(self, tools, **kwargs):
        self.offered = {tool.name for tool in tools} if kwargs.get("tool_choice") != "none" else set()
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
                            for m in reversed(messages) if m.text.startswith("TL3 execution budget: ")), "{}"))
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
    assert model.calls[2][1] == model.calls[0][1], "finalization retains the full tool list"
    assert any("finalization reserve" in m.text for m in model.calls[-1][0] if m.type == "tool")
    assert len(model.calls) == 5, "four bounded tool turns and one tools-free final reply"


def test_exploration_without_implementation_warns_halfway_and_keeps_the_reserve(maker_request, monkeypatch):
    model = install(monkeypatch, maker_request, turns=8, reserve=2, actions=[
        ("write", {"path": "test_greeting.py", "content": "def test_hello(): assert False\n"}),
        *[("read", {"path": "greeting.py"})] * 7])
    events = []
    runtime.run(maker_request, events.append)
    budgets = snapshots(model)
    assert not budgets[3].get("warning")
    assert "Stop broad exploration" in budgets[4].get("warning", ""), budgets[4]
    assert budgets[4]["tool_turns"]["remaining"] == 4
    assert budgets[4]["implementation_started"] is False, "a test edit is not implementation progress"
    assert "write" in model.calls[4][1], "the warning must leave implementation capacity"
    assert model.calls[6][1] == model.calls[0][1], "dispatch enforces the finalization restriction"
    assert len(model.calls) == 9
    assert events[-1]["reply"]["budget_exhaustion"]["tool_turns"] == {"used": 8, "allowed": 8}
    assert Path(maker_request["worktree"], "greeting.py").read_text().startswith("def greet")


def test_implementation_progress_suppresses_the_halfway_warning_and_survives_restore(maker_request, monkeypatch):
    model = install(monkeypatch, maker_request, turns=8, reserve=2, actions=[
        ("write", {"path": "greeting.py", "content": "def greet(): return 'hello'\n"})])
    budgets = []
    original = runtime.ExplorationMiddleware

    def construct(*args, **kwargs):
        budget = original(*args, **kwargs)
        budgets.append(budget)
        return budget

    monkeypatch.setattr(runtime, "ExplorationMiddleware", construct)
    runtime.run(maker_request, lambda _: None)
    assert snapshots(model)[4].get("implementation_started") is True
    assert not snapshots(model)[4].get("warning")
    restored = original(budgets[0].budget, "maker")
    restored.restore(budgets[0].state())
    assert restored.remaining()["implementation_started"] is True


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
    assert "write" in model.calls[0][1], "restored finalization still advertises stable definitions"
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


def test_planner_receives_actual_budget_and_approved_split_guidance(monkeypatch, tmp_path):
    from tl3_workers.roles import ROLES

    class PromptModel(FakeMessagesListChatModel):
        calls: list = []

        def _generate(self, messages, **kwargs):
            self.calls.append(deepcopy(messages))
            return super()._generate(messages, **kwargs)

    model = PromptModel(responses=[AIMessage(content=json.dumps({
        "kind": "cant_meet", "report": "The required API decision is unresolved.", "files": []}))], calls=[])
    monkeypatch.setattr(runtime, "create_model", lambda _: model)
    # build_agent exposes code-owned maker capacity even when old inputs lack it.
    with pytest.MonkeyPatch.context() as patch:
        patch.setattr(runtime, "_checked", lambda role, inputs, reply, **kwargs: reply)
        runtime.run({"role": "planner", "worktree": str(tmp_path), "inputs": {}}, lambda _: None)
    text = "\n".join(m.text for m in model.calls[0] if isinstance(m, HumanMessage))
    assert '"maker_budget"' in text
    assert str(ROLES["maker"].exploration_budget.tool_turns) in text
    system = "\n".join(m.text for m in model.calls[0] if m.type == "system")
    assert "finalization reserve" in system
    assert "new split reason" in system
    assert "Q54 is unchanged" in system
