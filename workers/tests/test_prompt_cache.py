"""Offline cache behavior through the worker graph and real proxy client."""
from copy import deepcopy
import json
from pathlib import Path

import httpx
from langchain_core.callbacks import UsageMetadataCallbackHandler
from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage
import pytest

from tl3_workers import runtime
from message_invariants import assert_tool_pairs
import test_maker
import test_workers


maker_request = test_maker.maker_request
request_doc = test_workers.request_doc


def capture_budget(monkeypatch):
    instances = []
    original = runtime.ExplorationMiddleware

    def construct(*args, **kwargs):
        instance = original(*args, **kwargs)
        instances.append(instance)
        return instance

    monkeypatch.setattr(runtime, "ExplorationMiddleware", construct)
    return instances


def payload(messages):
    return json.dumps([m.model_dump(include={"type", "content", "tool_calls", "tool_call_id", "name"})
                       for m in messages], ensure_ascii=False)


def test_append_only_prefix_and_one_threshold_compaction(maker_request, monkeypatch):
    maker_request["inputs"]["context_size"] = 100_000
    Path(maker_request["worktree"], "evidence.txt").write_text("source evidence\n" * 1400)
    seen = []

    class Explorer(test_maker.ToolModel):
        def _generate(self, messages, **kwargs):
            seen.append(deepcopy(messages))
            index = len(seen)
            final = index == 10
            self.responses = [AIMessage(
                content=json.dumps({"kind": "cant_meet", "report": "Need more checks."}) if final else "",
                tool_calls=[] if final else [{"name": "read", "args": {"path": "evidence.txt"},
                                              "id": str(index), "type": "tool_call"}],
                usage_metadata={"input_tokens": 71_000 if index == 7 else 10_000,
                                "output_tokens": 1, "total_tokens": 71_001 if index == 7 else 10_001})]
            self.i = 0
            return super()._generate(messages, **kwargs)

    monkeypatch.setattr(runtime, "create_model", lambda _: Explorer(responses=[]))
    runtime.run(maker_request, lambda _: None)
    assert len(seen) == 10
    for messages in seen:
        assert_tool_pairs(messages)
    breaks = [index for index in range(1, len(seen))
              if payload(seen[index][:len(seen[index - 1])]) != payload(seen[index - 1])]
    assert breaks == [7], "the prefix changes once, after the 70%-context call, never as a rolling window"
    compacted = seen[7]
    assert any("removed or clipped" in m.text for m in compacted if m.type == "human")
    assert any(m.type == "tool" and m.tool_call_id == "7" for m in compacted), "keep the newest group"


@pytest.mark.parametrize("details,weights,expected", [
    ({"cache_read": 80_000, "cache_creation": 10_000}, {}, 30_500),
    ({"cache_read": 80_000, "cache_creation": 10_000},
     {"UNCACHED": "2", "CREATION": "3", "READ": "0.5"}, 90_000),
    ({"cache_read": 80_000}, {}, 28_000),
    ({"cache_creation": 10_000}, {}, 102_500),
    ({}, {}, 100_000),
    (None, {}, 100_000),
])
def test_cache_weighted_budget_uses_reported_components(tmp_path, monkeypatch, details, weights, expected):
    for name, value in weights.items():
        monkeypatch.setenv("TL3_CACHE_" + name + "_WEIGHT", value)
    budgets = capture_budget(monkeypatch)
    usage = {"input_tokens": 100_000, "output_tokens": 1, "total_tokens": 100_001}
    if details is not None:
        usage["input_token_details"] = details
    model = test_maker.ToolModel(responses=[AIMessage(content=json.dumps(test_workers.BRIEF), usage_metadata=usage)])
    monkeypatch.setattr(runtime, "create_model", lambda _: model)
    runtime.run({"role": "ingest", "worktree": str(tmp_path), "inputs": {}}, lambda _: None)
    assert budgets[0].spent == expected


def test_cached_intake_keeps_four_exploration_rounds(tmp_path, monkeypatch):
    Path(tmp_path, "evidence.txt").write_text("source evidence")
    seen = []

    class Explorer(test_maker.ToolModel):
        def _generate(self, messages, **kwargs):
            seen.append(messages)
            index = len(seen)
            final = index == 5 or any("budget is exhausted" in m.text for m in messages if m.type == "human")
            self.responses = [AIMessage(
                content=json.dumps(test_workers.BRIEF) if final else "",
                tool_calls=[] if final else [{"name": "read", "args": {"path": "evidence.txt"},
                                              "id": str(index), "type": "tool_call"}],
                usage_metadata={"input_tokens": 100_000, "output_tokens": 1, "total_tokens": 100_001,
                                "input_token_details": {"cache_read": 90_000}})]
            self.i = 0
            return super()._generate(messages, **kwargs)

    budgets = capture_budget(monkeypatch)
    monkeypatch.setattr(runtime, "create_model", lambda _: Explorer(responses=[]))
    runtime.run({"role": "ingest", "worktree": str(tmp_path), "inputs": {}}, lambda _: None)
    assert len(seen) == 5, "cached input must not consume the uncached-input allowance"
    assert budgets[0].spent == 95_000


def response_frames(content, *, duplicate_usage=False):
    chunk = {"id": "fixture-completion", "object": "chat.completion.chunk", "created": 0,
             "model": "fixture-model"}
    usage = {"prompt_tokens": 1000, "completion_tokens": 5, "total_tokens": 1005,
             "prompt_tokens_details": {"cached_tokens": 800, "cache_write_tokens": 100}}
    frames = [{**chunk, "choices": [{"index": 0, "delta": {"role": "assistant", "content": content},
                                    "finish_reason": None}]},
              {**chunk, "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": usage}]
    if duplicate_usage:
        frames.append({**chunk, "choices": [], "usage": usage})
    stream = "".join("data: " + json.dumps(frame) + "\n\n" for frame in frames) + "data: [DONE]\n\n"
    return httpx.Response(200, headers={"content-type": "text/event-stream"}, content=stream)


def test_duplicate_stream_usage_is_charged_and_recorded_once(request_doc, monkeypatch):
    Path(request_doc["key_file"]).write_text("fixture-only-key")
    recorded = UsageMetadataCallbackHandler()
    budgets = capture_budget(monkeypatch)

    def respond(request):
        return response_frames(json.dumps(test_workers.BRIEF), duplicate_usage=True)

    proxy = runtime.ProxyChatOpenAI
    with httpx.Client(transport=httpx.MockTransport(respond)) as client:
        monkeypatch.setattr(runtime, "ProxyChatOpenAI", lambda **kwargs: proxy(
            http_client=client, callbacks=[recorded], **kwargs))
        runtime.run(request_doc, lambda _: None)
    usage = recorded.usage_metadata["fixture-model"]
    assert usage["input_tokens"] == 1000, "finish and trailing usage describe one model call"
    assert usage["output_tokens"] == 5 and usage["total_tokens"] == 1005
    assert usage["input_token_details"] == {"cache_read": 800, "cache_creation": 100}
    assert budgets[0].spent == 305


def test_session_identity_reaches_header_and_body_across_turns_and_resume(request_doc, monkeypatch, tmp_path):
    Path(request_doc["key_file"]).write_text("fixture-only-key")
    request_doc.update(role="maker", session_file=str(tmp_path / "session.json"), last_accepted_generation=6)
    request_doc["operation"]["generation"] = 7
    request_doc["inputs"]["attempt"] = 1
    Path(request_doc["session_file"]).with_suffix(".head").write_text(json.dumps(
        {"generation": 7, "head": "a" * 40}))
    sent = []

    def respond(request):
        body = json.loads(request.content)
        sent.append((request.headers.get("X-Session-ID"), body.get("prompt_cache_key")))
        assert body["stream_options"] == {"include_usage": True}
        return response_frames("A fixture reply.")

    proxy = runtime.ProxyChatOpenAI
    with httpx.Client(transport=httpx.MockTransport(respond)) as client:
        monkeypatch.setattr(runtime, "ProxyChatOpenAI", lambda **kwargs: proxy(http_client=client, **kwargs))
        model = runtime.create_model(request_doc)
        model.invoke("First turn")
        model.invoke("Second turn")
        request_doc["operation"]["generation"] = 8
        runtime.create_model(request_doc).invoke("Resumed worker")
        request_doc["operation"]["generation"] = 9
        request_doc["last_accepted_generation"] = 8
        request_doc["inputs"]["attempt"] = 2
        runtime.create_model(request_doc).invoke("Next attempt")
    expected = f"tl3-{request_doc['operation']['node']}-7-1"
    assert sent[:3] == [(expected, expected)] * 3, "affinity and cache identity survive a worker retry"
    assert sent[3][0] == sent[3][1] and sent[3][0] != expected
    assert "fixture-only-key" not in json.dumps(sent)
