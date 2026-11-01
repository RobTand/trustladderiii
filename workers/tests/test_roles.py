from dataclasses import replace
from importlib.resources import files
import json
import shutil
import subprocess
import sys

from jsonschema import Draft202012Validator
from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage
import pytest

from tl3_workers import runtime
from tl3_workers.contracts import schema
from tl3_workers import roles
from tl3_workers.roles import ROLES
from tl3_workers.tools import READ_TOOLS, TOOL_FACTORIES, WRITE_TOOLS


REPLY = {"verdict": "agree", "objections": []}


def test_discovered_roles_have_prompts_and_schemas_without_orphans():
    modules = {path.name.removesuffix(".py") for path in files(roles).iterdir()
               if path.name.endswith(".py") and not path.name.startswith("_")
               and path.name != "base.py"}
    assert set(ROLES) == modules
    expected_schemas = {"request.json"}
    role_pattern = schema("request")["properties"]["role"]
    assert "enum" not in role_pattern
    for name, role in ROLES.items():
        Draft202012Validator(role_pattern).validate(name)
        assert role.name == name
        assert role.input_schema == name + "-input"
        assert role.reply_schema == name + "-reply"
        assert files("tl3_workers").joinpath("prompts", role.prompt).read_text().strip()
        for resource in (role.input_schema, role.reply_schema):
            Draft202012Validator.check_schema(schema(resource))
            expected_schemas.add(resource + ".json")
        assert role.tools.keys() <= TOOL_FACTORIES.keys()
    actual_schemas = {path.name for path in files("tl3_workers").joinpath("schemas").iterdir()
                      if path.name.endswith(".json")}
    assert actual_schemas == expected_schemas


def test_new_module_is_discovered_without_registration(tmp_path):
    package = tmp_path / "tl3_workers"
    shutil.copytree(files("tl3_workers"), package, ignore=shutil.ignore_patterns("__pycache__"))
    (package / "roles/fixture.py").write_text(
        'from .base import Role\n'
        'ROLE = Role(name="fixture", prompt="fixture.md", '
        'input_schema="fixture-input", reply_schema="fixture-reply")\n'
    )
    result = subprocess.run(
        [sys.executable, "-c", "from tl3_workers.roles import ROLES; "
         "assert ROLES['fixture'].name == 'fixture'; "
         "assert {'ingest', 'audit'} <= ROLES.keys()"],
        cwd=tmp_path, capture_output=True, text=True,
    )
    assert result.returncode == 0, result.stderr


@pytest.mark.parametrize("names", [READ_TOOLS, WRITE_TOOLS, frozenset(TOOL_FACTORIES)])
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
            "recall": {"query": "greeting"}, "run_tests": {}, "list": {},
            "read": {"path": "hello.txt"}, "glob": {"pattern": "*.txt"},
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
    # The vetoed reply gets one repair turn, which the fake answers the same way.
    assert checked == [({}, REPLY)] * 2
    assert events == [{"event": "progress", "unit": "model_turn"}] * 2
