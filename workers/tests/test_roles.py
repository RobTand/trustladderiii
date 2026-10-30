from dataclasses import replace
from importlib.resources import files
import json
from pathlib import Path
import subprocess
import sys

from jsonschema import Draft202012Validator
from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage
import pytest

from tl3_workers import runtime
from tl3_workers.contracts import schema
from tl3_workers.roles import ROLE_MODULES, ROLES
from tl3_workers.tools import READ_TOOLS, TOOL_NAMES, WRITE_TOOLS


REPLY = {"verdict": "agree", "objections": []}


def test_registry_matches_protocol_role_enum():
    assert schema("request")["properties"]["role"]["enum"] == list(ROLES)
    assert len(ROLE_MODULES) == len(ROLES)
    for name, role in ROLES.items():
        assert role.name == name
        assert files("tl3_workers").joinpath("prompts", role.prompt).read_text().strip()
        Draft202012Validator.check_schema(schema(role.input_schema))
        Draft202012Validator.check_schema(schema(role.reply_schema))
        assert role.tools.keys() <= TOOL_NAMES


def test_role_enum_generator_is_current():
    script = Path(__file__).resolve().parents[1] / "tools/sync_role_schema.py"
    result = subprocess.run([sys.executable, str(script), "--check"], capture_output=True, text=True)
    assert result.returncode == 0, result.stderr


@pytest.mark.parametrize("names", [READ_TOOLS, WRITE_TOOLS, TOOL_NAMES])
def test_registered_role_gets_exactly_its_tools_and_descriptions(tmp_path, monkeypatch, names):
    observed = []
    descriptions = {name: f"Role-owned instructions for {name}." for name in sorted(names)}
    role = replace(ROLES["audit"], name="fixture", tools=descriptions)
    monkeypatch.setitem(ROLES, role.name, role)
    target = tmp_path / "hello.txt"
    target.write_text("hello\n")

    class InspectModel(FakeMessagesListChatModel):
        def bind_tools(self, tools, **kwargs):
            assert {tool.name: tool.description for tool in tools} == descriptions
            return self

        def _generate(self, messages, **kwargs):
            observed.append(messages)
            return super()._generate(messages, **kwargs)

    calls = []
    for name in sorted(names):
        args = {
            "list": {}, "read": {"path": "hello.txt"}, "glob": {"pattern": "*.txt"},
            "grep": {"pattern": "hello"}, "write": {"path": "new.txt", "content": "hello"},
            "edit": {"path": "hello.txt", "old": "hello", "new": "edited"},
        }[name]
        calls.append({"name": name, "args": args, "id": name, "type": "tool_call"})
    fake = InspectModel(responses=[AIMessage(content="", tool_calls=calls), AIMessage(content=json.dumps(REPLY))])
    monkeypatch.setattr(runtime, "create_model", lambda request: fake)
    events = []
    runtime.run({"role": "fixture", "worktree": str(tmp_path), "inputs": {}}, events.append)
    assert len(observed) == 2
    results = [message for message in observed[-1] if message.type == "tool"]
    assert len(results) == len(names)
    assert all(message.status == "success" for message in results)
    assert events == [
        {"event": "progress", "unit": "model_turn"},
        *[{"event": "progress", "unit": "tool_result"} for _ in names],
        {"event": "progress", "unit": "model_turn"},
        {"event": "reply", "role": "fixture", "reply": REPLY},
    ]
    if "write" in names:
        assert (tmp_path / "new.txt").read_text() == "hello"
        assert target.read_text() == "edited\n"
    else:
        assert target.read_text() == "hello\n"


@pytest.mark.parametrize("name,args", [
    ("execute", {"command": "touch forbidden"}), ("task", {"description": "delegate"}),
    ("fetch", {"url": "http://network.invalid"}), ("read_file", {"file_path": "/outside"}),
    ("glob", {"pattern": "**/*"}), ("grep", {"pattern": "hello"}),
    ("write", {"path": "forbidden", "content": "no"}), ("read", {"path": "../outside"}),
])
def test_role_dispatch_rejects_forbidden_calls(tmp_path, monkeypatch, name, args):
    observed = []
    role = replace(ROLES["audit"], tools={"read": "Read worktree text only."})
    monkeypatch.setitem(ROLES, "audit", role)

    class RecordingModel(FakeMessagesListChatModel):
        def bind_tools(self, tools, **kwargs):
            assert [tool.name for tool in tools] == ["read"]
            return self

        def _generate(self, messages, **kwargs):
            observed.append(messages)
            return super()._generate(messages, **kwargs)

    fake = RecordingModel(responses=[
        AIMessage(content="", tool_calls=[{"name": name, "args": args, "id": "bad", "type": "tool_call"}]),
        AIMessage(content=json.dumps(REPLY)),
    ])
    monkeypatch.setattr(runtime, "create_model", lambda request: fake)
    events = []
    runtime.run({"role": "audit", "worktree": str(tmp_path), "inputs": {}}, events.append)
    result = observed[-1][-1]
    assert result.type == "tool"
    assert result.status == "error"
    assert not (tmp_path / "forbidden").exists()
    assert events[-1]["reply"] == REPLY


def test_role_owns_optional_reply_check(tmp_path, monkeypatch):
    checked = []

    def check(inputs, reply):
        checked.append((inputs, reply))
        raise ValueError("fixture veto")

    monkeypatch.setitem(ROLES, "audit", replace(ROLES["audit"], check_reply=check))
    monkeypatch.setattr(runtime, "create_model", lambda request: FakeMessagesListChatModel(
        responses=[AIMessage(content=json.dumps(REPLY))],
    ))
    events = []
    with pytest.raises(ValueError, match="fixture veto"):
        runtime.run({"role": "audit", "worktree": str(tmp_path), "inputs": {}}, events.append)
    assert checked == [({}, REPLY)]
    assert events == [{"event": "progress", "unit": "model_turn"}]
