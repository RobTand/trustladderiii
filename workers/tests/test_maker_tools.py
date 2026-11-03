# ruff: noqa: F811

import json
from pathlib import Path
from threading import Barrier, Event
import httpx
from langchain_core.callbacks import BaseCallbackHandler
from langchain_core.messages import AIMessage
import pytest
from tl3_workers import runtime
from tl3_workers.roles import ROLES
from tl3_workers.roles import maker
from tl3_workers.tools import TOOL_FACTORIES
from message_invariants import DONE as DONE
from message_invariants import is_retro
from message_invariants import DONE_TOKENS as DONE_TOKENS
from message_invariants import ToolModel as ToolModel
from message_invariants import git as git
from message_invariants import maker_request as maker_request
from message_invariants import tool_capable_process_model as tool_capable_process_model
from message_invariants import turn as turn
from message_invariants import write_turn as write_turn


def test_done_schema_is_selected_by_kind(maker_request, monkeypatch):
    original = maker.schema

    def reordered(name):
        document = original(name)
        return {**document, "oneOf": list(reversed(document["oneOf"]))}

    monkeypatch.setattr(maker, "schema", reordered)
    model = ToolModel(responses=[AIMessage(**write_turn()), AIMessage(**turn(DONE))])
    monkeypatch.setattr(runtime, "create_model", lambda request: model)
    events = []
    runtime.run(maker_request, events.append)
    assert events[-1]["reply"] == {
        **DONE, "head": git(maker_request["worktree"], "rev-parse", "HEAD"),
    }


@pytest.mark.parametrize("names", [("write", "run_tests"), ("edit", "run_tests"), ("write", "edit")])
def test_mutations_and_tests_from_one_turn_do_not_overlap(maker_request, monkeypatch, names):
    dispatched = Barrier(2)
    dispatches = []
    entered = {name: Event() for name in names}
    trace = []
    snapshots = []
    root = Path(maker_request["worktree"])

    class DispatchBarrier(BaseCallbackHandler):
        raise_error = True

        def on_tool_start(self, serialized, input_str, **kwargs):
            dispatches.append(serialized["name"])
            dispatched.wait(timeout=5)

    build_agent = runtime.build_agent

    def with_barrier(*args, **kwargs):
        return build_agent(*args, **kwargs).with_config(callbacks=[DispatchBarrier()])

    monkeypatch.setattr(runtime, "build_agent", with_barrier)

    def observe(name, operation):
        trace.append((name, "start"))
        entered[name].set()
        other = next(item for item in names if item != name)
        entered[other].wait(timeout=0.2)
        result = operation()
        trace.append((name, "end"))
        return result

    def write_factory(worktree):
        def write(path: str, content: str) -> str:
            return observe("write", lambda: worktree.write(path, content))
        return write

    def edit_factory(worktree):
        def edit(path: str, old: str, new: str) -> str:
            return observe("edit", lambda: worktree.edit(path, old, new))
        return edit

    def test_factory(worktree):
        def run_tests() -> dict:
            def snapshot():
                snapshots.append((root / "greeting.py").read_text())
                return {"status": "completed", "totals": {"passed": 1}}
            return observe("run_tests", snapshot)
        return run_tests

    for name, factory in (("write", write_factory), ("edit", edit_factory), ("run_tests", test_factory)):
        monkeypatch.setitem(TOOL_FACTORIES, name, factory)
    arguments = {
        "write": {"path": "greeting.py", "content": "def greet(): return 'hello'\n"},
        "edit": {"path": "greeting.py", "old": "return", "new": "return  "},
        "run_tests": {},
    }
    response = {"kind": "cant_meet", "report": "More work is needed."}
    model = ToolModel(responses=[AIMessage(**turn(calls=[
        {"name": name, "args": arguments[name], "id": name, "type": "tool_call"} for name in names
    ])), AIMessage(**turn(response))])
    monkeypatch.setattr(runtime, "create_model", lambda request: model)
    events = []
    runtime.run(maker_request, events.append)
    assert sorted(dispatches) == sorted(names)
    assert [phase for _, phase in trace] == ["start", "end", "start", "end"]
    assert trace[0][0] == trace[1][0]
    assert trace[2][0] == trace[3][0]
    assert trace[0][0] != trace[2][0]
    if "run_tests" in names:
        assert len(snapshots) == 1
        assert snapshots[0] in {
            "def greet(): return 'goodbye'\n", "def greet(): return 'hello'\n",
            "def greet(): return   'goodbye'\n",
        }
    assert events[-1]["reply"] == response
    assert [event["unit"] for event in events[:-1]].count("tool_result") == 2


@pytest.mark.parametrize("input_tokens,output_tokens,kind", [
    (849 - DONE_TOKENS, 100, "done"), (850 - DONE_TOKENS, 100, "out_of_room"),
    (0, 0, "out_of_room"), (None, None, "out_of_room"),
])
def test_proxy_stream_requests_usage_and_serial_tools_and_uses_usage(
    maker_request, monkeypatch, input_tokens, output_tokens, kind,
):
    requests, retros = [], []
    root = Path(maker_request["worktree"])
    before = git(root, "rev-parse", "HEAD")
    Path(maker_request["key_file"]).write_text("fixture-key")

    def respond(request):
        body = json.loads(request.content)
        if is_retro(body):
            # The retro turn (#648) is tool-free, so it sends no tool options.
            retros.append(body)
            assert "tools" not in body and "parallel_tool_calls" not in body
            frame = {"id": "fixture-retro", "object": "chat.completion.chunk", "created": 0,
                     "model": "fixture-model", "choices": [{"index": 0, "finish_reason": "stop", "delta": {
                         "role": "assistant", "content": json.dumps(dict.fromkeys(
                             ("slowed_by", "missing", "checks", "noise"), []))}}]}
            return httpx.Response(200, headers={"content-type": "text/event-stream"},
                                  content="data: " + json.dumps(frame) + "\n\ndata: [DONE]\n\n")
        requests.append(body)
        assert request.url == "http://model.invalid/v1/chat/completions"
        assert body["stream"] is True
        assert body["stream_options"] == {"include_usage": True}
        assert body["parallel_tool_calls"] is False
        assert {tool["function"]["name"] for tool in body["tools"]} == set(ROLES["maker"].tools)
        if len(requests) == 1:
            delta = {"role": "assistant", "tool_calls": [{
                "index": 0, "id": "write", "type": "function",
                "function": {"name": "write", "arguments": json.dumps({
                    "path": "greeting.py", "content": "def greet(): return 'hello'\n",
                })},
            }]}
            finish_reason = "tool_calls"
            usage = {"prompt_tokens": 100, "completion_tokens": 50, "total_tokens": 150}
        else:
            assert len(requests) == 2
            assert any(message["role"] == "tool" for message in body["messages"])
            delta = {"role": "assistant", "content": json.dumps(DONE)}
            finish_reason = "stop"
            usage = None if input_tokens is None else {
                "prompt_tokens": input_tokens, "completion_tokens": output_tokens,
                "total_tokens": input_tokens + output_tokens,
            }
        chunk = {"id": "fixture-completion", "object": "chat.completion.chunk", "created": 0,
                 "model": "fixture-model"}
        frames = [
            {**chunk, "choices": [{"index": 0, "delta": delta, "finish_reason": None}]},
            {**chunk, "choices": [{"index": 0, "delta": {}, "finish_reason": finish_reason}]},
        ]
        if usage is not None:
            frames.append({**chunk, "choices": [], "usage": usage})
        content = "".join("data: " + json.dumps(frame) + "\n\n" for frame in frames) + "data: [DONE]\n\n"
        return httpx.Response(200, headers={"content-type": "text/event-stream"}, content=content)

    proxy = runtime.ProxyChatOpenAI
    with httpx.Client(transport=httpx.MockTransport(respond)) as client:
        monkeypatch.setattr(runtime, "ProxyChatOpenAI", lambda **kwargs: proxy(http_client=client, **kwargs))
        events = []
        runtime.run(maker_request, events.append)
    assert len(requests) == 2
    assert len(retros) == (0 if kind == "out_of_room" else 1), "a full context ends without a retro"
    # The retro reports its model turn before the reply, except after out_of_room.
    retro_turn = [] if kind == "out_of_room" else ["model_turn"]
    assert [event["unit"] for event in events[:-1]] == ["model_turn", "tool_result", "model_turn", *retro_turn]
    assert events[-1]["reply"]["kind"] == kind
    if kind == "done":
        assert events[-1]["reply"] == {**DONE, "head": git(root, "rev-parse", "HEAD")}
        assert git(root, "rev-parse", "HEAD^") == before
    else:
        assert git(root, "rev-parse", "HEAD") == before
    assert (root / "greeting.py").read_text() == "def greet(): return 'hello'\n"
