"""The reply schema in each system prompt is the one the model can use (#648).

The model sees only the JSON in its system prompt: a $ref to another schema file
dangles, and a $defs entry nothing references is dead weight on every call.
"""

import json

from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage, SystemMessage
import pytest

from tl3_workers import runtime
import message_invariants

# pytest collects the shared fixture under this module-level name.
maker_request = message_invariants.maker_request

MARKER = " It must match this schema:\n"
BRIEF = {"kind": "brief", "brief": "Fix the greeting without changing its interface.",
         "goals": [{"description": "Return the requested greeting.", "verification": "test"}],
         "memory_ids": []}
REPLIES = {
    "maker": {"kind": "cant_meet", "report": "The plan needs a file it does not declare."},
    "ingest": BRIEF,
    "audit": {"verdict": "agree", "objections": []},
}


class Recording(FakeMessagesListChatModel):
    systems: list = []

    def bind_tools(self, tools, **kwargs):
        return self

    def _generate(self, messages, **kwargs):
        self.systems.extend(m.text for m in messages if isinstance(m, SystemMessage))
        return super()._generate(messages, **kwargs)


def intake_request(tmp_path, role):
    inputs = {"issue": {"number": 1, "title": "Fix greeting", "body": "Return hello.",
                        "author": "requester", "comments": []},
              "repo": {"readme": "A greeting library.", "file_list": ["greeting.py"]}}
    if role == "audit":
        inputs["ingest"] = BRIEF
    return {"protocol": 1, "operation": {"node": 1, "generation": 0, "kind": role}, "role": role,
            "model": {"base_url": "http://model.invalid/v1", "name": "fixture-model",
                      "effort": "high", "budget_tokens": 32000},
            "key_file": str(tmp_path / "never-read-this-key"), "inputs": inputs, "worktree": str(tmp_path)}


def rendered_schema(request, monkeypatch):
    model = Recording(responses=[AIMessage(content=json.dumps(REPLIES[request["role"]]))], systems=[])
    monkeypatch.setattr(runtime, "create_model", lambda _: model)
    runtime.run(request, lambda event: None)
    system = model.systems[0]
    assert MARKER in system
    return json.loads(system.split(MARKER, 1)[1])


def references(value):
    if isinstance(value, dict):
        for key, item in value.items():
            if key == "$ref":
                yield item
            else:
                yield from references(item)
    elif isinstance(value, list):
        for item in value:
            yield from references(item)


@pytest.fixture(params=["maker", "ingest", "audit"])
def schema(request, maker_request, monkeypatch, tmp_path):
    role = request.param
    if role == "maker":
        maker_request["inputs"]["context_size"] = 150_000
        return rendered_schema(maker_request, monkeypatch)
    return rendered_schema(intake_request(tmp_path, role), monkeypatch)


def test_every_reference_resolves_inside_the_rendered_schema(schema):
    for ref in references(schema):
        assert ref.startswith("#/$defs/"), f"{ref} points outside the schema the model sees"
        assert ref.removeprefix("#/$defs/") in schema.get("$defs", {}), ref


def test_the_rendered_schema_carries_no_unused_defs(schema):
    used = {ref.removeprefix("#/$defs/") for ref in references(schema) if ref.startswith("#/$defs/")}
    assert set(schema.get("$defs", {})) <= used, sorted(set(schema.get("$defs", {})) - used)
