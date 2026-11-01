"""Q50: a blocked maker reply cites environment evidence recorded in the same run."""

from pathlib import Path

from jsonschema import ValidationError
from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage
import pytest

from tl3_workers import runtime
from tl3_workers.tools import TOOL_FACTORIES, Worktree, build_tools
from test_maker import maker_request, turn  # noqa: F401

KEY = "c" * 64
TOTALS = {"passed": 0, "failed": 0, "error": 0, "skipped": 0}
INFRASTRUCTURE = {"status": "infrastructure_error", "message": "PrismaBuild failed.",
                  "tests": [], "totals": TOTALS, "note": ""}


def run_maker(maker_request, monkeypatch, result, reply):  # noqa: F811
    observed = []

    def factory(worktree):
        def run_tests(tests: list[str] | None = None, base: bool = False) -> dict:
            return dict(result)
        return run_tests

    class Model(FakeMessagesListChatModel):
        def _generate(self, messages, **kwargs):
            observed.append(messages)
            return super()._generate(messages, **kwargs)

    model = Model(responses=[AIMessage(**turn(calls=[{
        "name": "run_tests", "args": {"tests": ["test_greeting.py::test_hello"]},
        "id": "test", "type": "tool_call",
    }])), AIMessage(**turn(reply))])
    monkeypatch.setitem(TOOL_FACTORIES, "run_tests", factory)
    monkeypatch.setattr(runtime, "create_model", lambda request: model)
    events = []
    runtime.run(maker_request, events.append)
    return events[-1]["reply"], observed[-1][-1].text


def test_blocked_cites_an_infrastructure_action_key_from_this_run(maker_request, monkeypatch):  # noqa: F811
    reply = {"kind": "blocked", "evidence": {"action_key": KEY},
             "report": "PrismaBuild could not run the tests."}
    result, _ = run_maker(maker_request, monkeypatch, {**INFRASTRUCTURE, "action_key": KEY}, reply)
    assert result == reply


def test_blocked_cites_a_recorded_tool_error(maker_request, monkeypatch):  # noqa: F811
    reply = {"kind": "blocked", "evidence": {"tool_error": "tool-error-1"},
             "report": "The test tool could not submit to PrismaBuild."}
    result, seen = run_maker(maker_request, monkeypatch, INFRASTRUCTURE, reply)
    assert "tool-error-1" in seen, "the model sees the id it may cite"
    assert result == reply


@pytest.mark.parametrize("result,evidence", [
    ({"status": "completed", "tests": [], "totals": TOTALS, "note": "", "action_key": KEY},
     {"action_key": KEY}),
    ({**INFRASTRUCTURE, "action_key": KEY}, {"action_key": "d" * 64}),
    (INFRASTRUCTURE, {"tool_error": "tool-error-2"}),
])
def test_blocked_without_recorded_evidence_becomes_cant_meet(maker_request, monkeypatch, result, evidence):  # noqa: F811
    reply = {"kind": "blocked", "evidence": evidence, "report": "The sandbox is broken."}
    result, _ = run_maker(maker_request, monkeypatch, result, reply)
    assert result["kind"] == "cant_meet"
    assert "The sandbox is broken." in result["report"]


def test_blocked_needs_structured_evidence(maker_request, monkeypatch):  # noqa: F811
    reply = {"kind": "blocked", "report": "The sandbox is broken."}
    with pytest.raises(ValidationError):
        run_maker(maker_request, monkeypatch, {**INFRASTRUCTURE, "action_key": KEY}, reply)


@pytest.mark.parametrize("error,recorded", [
    (PermissionError("sandbox refused"), True),
    (ValueError("Use a relative worktree path"), False),
    (FileNotFoundError("missing"), False),
])
def test_environment_tool_errors_get_an_id(tmp_path: Path, monkeypatch, error, recorded):
    def factory(worktree):
        def read(path: str) -> str:
            """Fail like the environment or the model's input."""
            raise error
        return read

    monkeypatch.setitem(TOOL_FACTORIES, "read", factory)
    with Worktree(tmp_path) as worktree:
        tool, = build_tools(worktree, {"read": "Read."})
        message = tool.invoke({"name": "read", "args": {"path": "a.py"}, "id": "read", "type": "tool_call"})
    assert message.status == "error"
    assert ("tool-error-1" in message.text) is recorded
