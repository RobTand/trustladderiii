"""Shared first-request prefixes through the real model client, without credentials."""
from copy import deepcopy
import json
from pathlib import Path

import httpx
import pytest

from tl3_workers import runtime
from tl3_workers.evidence import forcing_capabilities
from worker_process import response_frames
import worker_process as test_workers


request_doc = test_workers.request_doc


def shared_request(request_doc, role):
    request = deepcopy(request_doc)
    Path(request["key_file"]).write_text("fixture-only-key")
    request["role"] = role
    request["trace"] = {"repo": "owner/served", "request": "first", "attempt": 1}
    request["prompt"] = {"version": 1, "text": runtime.role_prompt(runtime.ROLES[role])}
    if role == "audit":
        request["inputs"]["ingest"] = deepcopy(test_workers.BRIEF)
    elif role == "planner":
        request["inputs"] = {"base_sha": "a" * 40, "ingest": deepcopy(test_workers.BRIEF),
                             "must_split": False, "findings": [], "protected_paths": [],
                             "forecast": {"fit_tokens": 100_000, "context_tokens": 150_000,
                                          "overhead_tokens": {"prompt": 1000}}}
    return request


@pytest.mark.parametrize("role", ["ingest", "audit", "planner"])
@pytest.mark.parametrize("model", ["claude-sonnet-5-5", "gpt-6.1-sol"])
def test_requests_share_wire_prefix_and_cache_but_keep_attempt_sessions(request_doc, monkeypatch, role, model):
    first = shared_request(request_doc, role)
    first["model"]["name"] = model
    second = deepcopy(first)
    second["operation"] = {"node": 99, "generation": 7, "kind": role}
    second["trace"].update(request="second", attempt=2)
    second["inputs"]["memories"] = [{"id": "different", "content": "PRIVATE_REQUEST_MARKER"}]
    if role == "planner":
        second["inputs"]["ingest"]["brief"] = "PRIVATE_REQUEST_MARKER"
    else:
        second["inputs"]["issue"].update(number=99, title="PRIVATE_REQUEST_MARKER", body="PRIVATE_REQUEST_MARKER")
        if role == "audit":
            second["inputs"]["ingest"]["brief"] = "PRIVATE_REQUEST_MARKER"
    sent = []
    reply = {"ingest": test_workers.BRIEF, "audit": test_workers.AGREE,
             "planner": {"kind": "cant_meet", "files": [], "report": "Fixture refusal."}}[role]

    def respond(request):
        sent.append((request.headers["X-Session-ID"], json.loads(request.content)))
        return response_frames(json.dumps(reply))

    proxy = runtime.ProxyChatOpenAI
    with httpx.Client(transport=httpx.MockTransport(respond)) as client:
        monkeypatch.setattr(runtime, "ProxyChatOpenAI", lambda **kwargs: proxy(http_client=client, **kwargs))
        for request in (first, second):
            runtime.run(request, lambda _: None)
    assert len(sent) == 2
    prefixes = []
    for (_, body), request in zip(sent, (first, second)):
        system, user = body["messages"][:2]
        assert system["role"] == "system" and user["role"] == "user"
        assert isinstance(user["content"], list), "orientation needs a separate stable text block"
        shared, specific = user["content"]
        assert shared["type"] == specific["type"] == "text"
        assert shared.get("cache_control") == ({"type": "ephemeral"} if model.startswith("claude") else None)
        assert "cache_control" not in specific
        inputs = json.loads(shared["text"] + specific["text"])
        expected = {**request["inputs"], "forcing_capabilities": forcing_capabilities()}
        if role == "planner":
            budget = runtime.ROLES["maker"].exploration_budget
            expected["maker_budget"] = {"tool_turns": budget.tool_turns, "input_tokens": budget.input_tokens,
                                       "finalization_turns": budget.finalization_turns}
        assert inputs == expected, "all existing inputs, including independent audit evidence, must survive"
        prefixes.append(json.dumps([system, shared], ensure_ascii=False).encode())
        assert runtime.role_prompt(runtime.ROLES[role]).splitlines()[0] in system["content"]
        assert system["content"].index("It must match this schema:") < len(system["content"])
        assert "PRIVATE_REQUEST_MARKER" not in prefixes[-1].decode()
        assert "PRIVATE_REQUEST_MARKER" not in body["prompt_cache_key"]
    assert prefixes[0] == prefixes[1], "shared prefix must be byte-identical across requests"
    assert sent[0][1]["tools"] == sent[1][1]["tools"]
    assert sent[0][1]["prompt_cache_key"] == sent[1][1]["prompt_cache_key"]
    assert [session for session, _ in sent] == ["tl3-1-0-1", "tl3-99-7-2"]


@pytest.mark.parametrize("change", ["repo", "orientation", "base", "prompt_version", "prompt_text", "schema", "role"])
def test_shared_cache_key_changes_with_shared_identity(request_doc, monkeypatch, change):
    request = shared_request(request_doc, "planner" if change == "base" else "ingest")
    before = runtime.create_model(request).model_kwargs["prompt_cache_key"]
    if change == "repo":
        request["trace"]["repo"] = "owner/other"
    elif change == "orientation":
        request["inputs"]["repo"]["readme"] += " Changed base orientation."
    elif change == "base":
        request["inputs"]["base_sha"] = "b" * 40
    elif change == "prompt_version":
        request["prompt"]["version"] += 1
    elif change == "prompt_text":
        request["prompt"]["text"] += " Changed instructions."
    elif change == "schema":
        original = runtime.schema
        monkeypatch.setattr(runtime, "schema", lambda name: {**original(name), "description": "Changed schema"})
    elif change == "role":
        request["role"] = "audit"
    after = runtime.create_model(request).model_kwargs["prompt_cache_key"]
    assert before != after, f"{change} must change shared cache identity"


def test_cache_key_ignores_request_text_and_orientation_mapping_order(request_doc):
    request = shared_request(request_doc, "ingest")
    before = runtime.create_model(request).model_kwargs["prompt_cache_key"]
    request["operation"].update(node=5, generation=8)
    request["inputs"]["issue"]["body"] = "PRIVATE_REQUEST_MARKER"
    request["trace"].update(request="changed", attempt=3)
    request["inputs"]["repo"] = dict(reversed(list(request["inputs"]["repo"].items())))
    after = runtime.create_model(request).model_kwargs["prompt_cache_key"]
    assert before == after
    assert "PRIVATE_REQUEST_MARKER" not in after


@pytest.mark.parametrize("repair", [False, True])
def test_claude_shared_prefix_keeps_a_rolling_marker_on_every_transport_call(request_doc, monkeypatch, repair):
    request = shared_request(request_doc, "ingest")
    request["model"]["name"] = "claude-sonnet-5-5"
    Path(request["worktree"], "greeting.py").write_text("def greet(): return 'hello'\n")
    sent = []

    def respond(http_request):
        sent.append(json.loads(http_request.content))
        if len(sent) == 1 or len(sent) == 2 and not repair:
            return response_frames("", tool_calls=[{
                "index": 0, "id": f"read-{len(sent)}", "type": "function",
                "function": {"name": "read", "arguments": json.dumps({"path": "greeting.py"})}}])
        return response_frames("Invalid JSON requiring repair" if len(sent) == 2 else json.dumps(test_workers.BRIEF))

    proxy = runtime.ProxyChatOpenAI
    with httpx.Client(transport=httpx.MockTransport(respond)) as client:
        monkeypatch.setattr(runtime, "ProxyChatOpenAI", lambda **kwargs: proxy(http_client=client, **kwargs))
        runtime.run(request, lambda _: None)
    assert len(sent) == 3
    assert [body["messages"][-1]["role"] for body in sent] == ["user", "tool", "user" if repair else "tool"]
    prefixes = []
    for body in sent:
        messages = body["messages"]
        assert messages[-1].get("cache_control") == {"type": "ephemeral"}, \
            "every call needs a rolling marker on its latest turn, including tool results and repair notices"
        assert all("cache_control" not in message for message in messages[:-1]), "old rolling markers must not accumulate"
        prefix = messages[1]["content"][0]
        assert prefix["cache_control"] == {"type": "ephemeral"}
        prefixes.append(prefix)
        assert json.dumps(body).count('"cache_control"') == 2, "shared plus rolling stays below Claude's four-marker limit"
    assert prefixes == [prefixes[0]] * 3, "rolling markers must not mutate the shared prefix"
