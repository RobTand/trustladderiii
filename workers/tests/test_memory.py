from dataclasses import replace
import json

from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage
import pytest

from tl3_workers import runtime
from tl3_workers.roles import ROLES
from tl3_workers.tools import build_tools


@pytest.mark.parametrize("name", sorted(ROLES))
def test_every_role_reports_durable_activity_and_reply(tmp_path, monkeypatch, name):
    (tmp_path / "greeting.py").write_text("hello")

    def role_run(model, request, worktree, emit):
        tools = {tool.name: tool for tool in build_tools(worktree, {"read": "Read", "write": "Write"})}
        tools["read"].invoke({"path": "greeting.py"})
        tools["write"].invoke({"path": "greeting.py", "content": "goodbye"})
        return {"verdict": "agree", "objections": []}

    role = replace(ROLES["audit"], name=name, tools={"read": "Read", "write": "Write"},
                   run=role_run, check_reply=None)
    monkeypatch.setitem(ROLES, name, role)
    monkeypatch.setattr(runtime, "create_model", lambda request: object())
    summary = tmp_path / "memory.json"
    events = []
    runtime.run({"role": name, "worktree": str(tmp_path), "inputs": {"goals": ["Fix greeting"]},
                 "memory": {"summary_file": str(summary)}}, events.append)
    assert summary.exists(), "every role must leave a durable run summary"
    record = json.loads(summary.read_text())
    assert record["activity"]["files_read"] == ["greeting.py"]
    assert record["activity"]["files_changed"] == ["greeting.py"]
    assert record["reply"] == {"verdict": "agree", "objections": []}
    assert record["outcome"] == "succeeded"
    assert events[-1]["event"] == "reply"


def test_failure_reports_safe_reason_before_propagating(tmp_path, monkeypatch):
    def failed(request):
        raise RuntimeError("Authorization: Bearer fixture-secret")

    monkeypatch.setattr(runtime, "create_model", failed)
    summary = tmp_path / "memory.json"
    with pytest.raises(RuntimeError):
        runtime.run({"role": "audit", "inputs": {}, "worktree": str(tmp_path),
                     "memory": {"summary_file": str(summary)}}, lambda event: None)
    assert summary.exists(), "failed runs must also leave a summary"
    record = json.loads(summary.read_text())
    assert record["outcome"] == "failed"
    assert record["reason"] == "RuntimeError"
    assert "fixture-secret" not in summary.read_text()


def test_memory_write_failure_does_not_change_worker_reply(tmp_path, monkeypatch):
    class Model(FakeMessagesListChatModel):
        def bind_tools(self, tools, **kwargs):
            return self

    model = Model(responses=[AIMessage(content='{"verdict":"agree","objections":[]}')])
    monkeypatch.setattr(runtime, "create_model", lambda request: model)
    events = []
    runtime.run({"role": "audit", "worktree": str(tmp_path), "inputs": {},
                 "memory": {"summary_file": str(tmp_path)}}, events.append)
    assert events[-1] == {"event": "reply", "role": "audit", "reply": {"verdict": "agree", "objections": []}}


def test_test_tool_preserves_harness_identifiers_results_and_action_key(tmp_path, monkeypatch):
    from tl3_harness.test_tool import Summary
    from tl3_workers import tools

    result = Summary()
    result.add("test_greeting.py::test_hello", "failed", "password=fixture-failure-secret")
    result.result["action_key"] = "a" * 64

    def run_tests(tests: list[str] | None = None) -> dict:
        return result.result

    def role_run(model, request, worktree, emit):
        tool = build_tools(worktree, {"run_tests": "Run tests"})[0]
        tool.invoke({"tests": ["test_greeting.py::test_hello"]})
        return {"verdict": "agree", "objections": []}

    role = replace(ROLES["audit"], tools={"run_tests": "Run tests"}, run=role_run,
                   check_reply=None)
    monkeypatch.setitem(ROLES, "audit", role)
    monkeypatch.setitem(tools.TOOL_FACTORIES, "run_tests", lambda worktree: run_tests)
    monkeypatch.setattr(runtime, "create_model", lambda request: object())
    summary = tmp_path / "memory.json"
    runtime.run({"role": "audit", "worktree": str(tmp_path), "inputs": {},
                 "memory": {"summary_file": str(summary)}}, lambda event: None)
    evidence = json.loads(summary.read_text())["activity"]["tests"][0]
    assert evidence["selectors"] == ["test_greeting.py::test_hello"]
    assert evidence["action_key"] == "a" * 64
    assert evidence["totals"]["failed"] == 1
    assert evidence["tests"] == [{"test": "test_greeting.py::test_hello", "outcome": "failed"}]
    assert "fixture-failure-secret" not in summary.read_text()


def test_unchanged_activity_does_not_rewrite_summary(tmp_path, monkeypatch):
    from tl3_workers.memory import Summary
    summary = Summary({"memory": {"summary_file": str(tmp_path / "memory.json")}})
    summary.file("file.py", False)
    writes = []
    monkeypatch.setattr(summary, "save", lambda: writes.append(True))
    summary.file("file.py", False)
    summary.tool("recall", {"query": "decisions"}, {"results": []})
    assert not writes
