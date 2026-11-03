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
from message_invariants import assert_tool_pairs, is_retro
import message_invariants as test_maker
import worker_process as test_workers
from worker_process import response_frames


maker_request = test_maker.maker_request
request_doc = test_workers.request_doc


class ToolModel(FakeMessagesListChatModel):
    def bind_tools(self, tools, **kwargs):
        return self


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


def evidence_explorer(maker_request, seen, usage):
    maker_request["inputs"]["context_size"] = 100_000
    Path(maker_request["worktree"], "evidence.txt").write_text(("source evidence " * 7 + "\n") * 200)

    class Explorer(ToolModel):
        def _generate(self, messages, **kwargs):
            if not is_retro(messages):  # the retro (#648) has its own tests
                seen.append(deepcopy(messages))
            index = len(seen)
            final = index == 10
            tokens = usage.get(index, 10_000)
            self.responses = [AIMessage(
                content=json.dumps({"kind": "cant_meet", "report": "Need more checks."}) if final else "",
                tool_calls=[] if final else [{"name": "read", "args": {"path": "evidence.txt"},
                                              "id": str(index), "type": "tool_call"}],
                usage_metadata={"input_tokens": tokens, "output_tokens": 1, "total_tokens": tokens + 1})]
            self.i = 0
            return super()._generate(messages, **kwargs)

    return Explorer(responses=[])


def test_append_only_prefix_and_one_threshold_compaction(maker_request, monkeypatch):
    seen = []
    model = evidence_explorer(maker_request, seen, {7: 71_000})
    monkeypatch.setattr(runtime, "create_model", lambda _: model)
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


def finish_requested(messages):
    return any("budget is exhausted" in m.text for m in messages if m.type == "human")


def test_context_regrowing_after_compaction_requests_finish(maker_request, monkeypatch):
    seen = []
    # Call 8 compacts below 70%. Call 9 reports 75%, so call 10 must finish.
    model = evidence_explorer(maker_request, seen, {7: 71_000, 9: 75_000})
    monkeypatch.setattr(runtime, "create_model", lambda _: model)
    runtime.run(maker_request, lambda _: None)
    assert [finish_requested(messages) for messages in seen] == [False] * 9 + [True]


def scripted_maker(seen, usage):
    calls = [*[("read", {"path": "greeting.py"})] * 3,
             ("write", {"path": "greeting.py", "content": "def greet(): return 'hello'\n"})]

    class Scripted(ToolModel):
        def _generate(self, messages, **kwargs):
            if not is_retro(messages):  # the retro (#648) has its own tests
                seen.append(deepcopy(messages))
            index = len(seen)
            if index > len(calls):
                content, tool_calls = json.dumps(test_maker.DONE), []
            else:
                name, args = calls[index - 1]
                content, tool_calls = "", [{"name": name, "args": args, "id": str(index), "type": "tool_call"}]
            tokens = usage(index)
            self.responses = [AIMessage(content=content, tool_calls=tool_calls, usage_metadata={
                "input_tokens": tokens, "output_tokens": 1, "total_tokens": tokens + 1})]
            self.i = 0
            return super()._generate(messages, **kwargs)

    return Scripted(responses=[])


def test_instructions_above_threshold_leave_room_to_work(maker_request, monkeypatch):
    # A fix round's accumulated review feedback alone can exceed 70% of the context.
    # Compaction cannot shrink instructions, so it must not end the run on the next turn.
    maker_request["inputs"]["context_size"] = 100_000
    seen = []
    monkeypatch.setattr(runtime, "create_model", lambda _: scripted_maker(seen, lambda index: 80_000))
    events = []
    runtime.run(maker_request, events.append)
    assert len(seen) == 5
    assert not any(finish_requested(messages) for messages in seen)
    assert events[-1]["reply"]["kind"] == "done"


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
    model = ToolModel(responses=[AIMessage(content=json.dumps(test_workers.BRIEF), usage_metadata=usage)])
    monkeypatch.setattr(runtime, "create_model", lambda _: model)
    runtime.run({"role": "ingest", "worktree": str(tmp_path), "inputs": {}}, lambda _: None)
    assert budgets[0].spent == expected


def test_cached_intake_keeps_four_exploration_rounds(tmp_path, monkeypatch):
    Path(tmp_path, "evidence.txt").write_text("source evidence")
    seen = []

    class Explorer(ToolModel):
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


def test_maker_budget_notices_preserve_the_proxy_system_prefix(maker_request, tmp_path, monkeypatch):
    Path(maker_request["key_file"]).write_text("fixture-only-key")
    maker_request["inputs"]["context_size"] = 150_000
    maker_request["session_file"] = str(tmp_path / "session.json")
    sent = []
    offered = []
    retros = []

    def respond(request):
        body = json.loads(request.content)
        if is_retro(body):
            retros.append((len(sent), body["messages"]))
            return response_frames(json.dumps(dict.fromkeys(("slowed_by", "missing", "checks", "noise"), [])))
        sent.append(body["messages"])
        offered.append(bool(body.get('tools')) and body.get('tool_choice') != 'none')
        if len(sent) <= 2:
            return response_frames("", tool_calls=[{
                "index": 0, "id": f"read-{len(sent)}", "type": "function",
                "function": {"name": "read", "arguments": json.dumps({"path": "greeting.py"})}}])
        if len(sent) == 3:
            return response_frames('Malformed final reply requiring one format repair.')
        return response_frames(json.dumps({"kind": "cant_meet", "report": "More proof is needed."}))

    proxy = runtime.ProxyChatOpenAI
    with httpx.Client(transport=httpx.MockTransport(respond)) as client:
        monkeypatch.setattr(runtime, "ProxyChatOpenAI", lambda **kwargs: proxy(http_client=client, **kwargs))
        runtime.run(maker_request, lambda _: None)
        assert len(sent) == 4
        for path in (Path(maker_request['session_file']), Path(maker_request['session_file']).with_suffix('.visit')):
            saved = json.loads(path.read_text())
            history = saved['messages'] if isinstance(saved, dict) else saved
            notices = [m['data']['content'] for m in history
                       if m['data']['content'].startswith('TL3 execution budget: ')]
            assert notices == [messages[-1]['content'] for messages in sent], \
                'the repair notice must survive in both session and visit history'
        maker_request.update(resume_session=True, last_accepted_generation=0)
        maker_request['operation']['generation'] = 1
        maker_request['inputs']['round'] = 1
        runtime.run(maker_request, lambda _: None)

    assert len(sent) == 5
    assert offered == [True, True, True, False, True]
    assert [after for after, _ in retros] == [4, 5], "each run ends with one retro"
    for after, messages in retros:
        assert messages[:len(sent[after - 1])] == sent[after - 1], "the retro reuses the last call's wire prefix"
    system = sent[0][0]["content"].encode("utf-8")
    for index, messages in enumerate(sent):
        assert messages[0]["role"] == "system"
        assert messages[0]["content"].encode("utf-8") == system
        assert not any(m["role"] == "system" for m in messages[1:]), "the proxy must never hoist changing notices"
        notice = messages[-1]
        assert notice["role"] == "user" and notice["content"].startswith("TL3 execution budget: ")
        assert json.loads(notice["content"].removeprefix("TL3 execution budget: "))["tool_turns"]["remaining"] == [512, 511, 510, 509, 512][index]
        if index in (1, 2):
            assert messages[-2]["role"] == "tool", "the user notice follows the completed tool result"
        if index:
            assert messages[:len(sent[index - 1])] == sent[index - 1], "previous wire content stays append-only"
    saved = json.loads(Path(maker_request["session_file"]).read_text())
    notices = [m for m in saved if m["data"]["content"].startswith("TL3 execution budget: ")]
    assert [m["type"] for m in notices] == ["human"] * 5
    assert [m["data"]["content"] for m in notices] == [messages[-1]["content"] for messages in sent]


@pytest.mark.parametrize("corrected_total", [False, True])
def test_duplicate_stream_usage_is_charged_and_recorded_once(request_doc, monkeypatch, corrected_total):
    Path(request_doc["key_file"]).write_text("fixture-only-key")
    recorded = UsageMetadataCallbackHandler()
    budgets = capture_budget(monkeypatch)

    def respond(request):
        trailing = ({"prompt_tokens": 1200, "completion_tokens": 6, "total_tokens": 1206,
                     "prompt_tokens_details": {"cached_tokens": 900, "cache_write_tokens": 200}}
                    if corrected_total else None)
        return response_frames(json.dumps(test_workers.BRIEF), duplicate_usage=True, trailing_usage=trailing)

    proxy = runtime.ProxyChatOpenAI
    with httpx.Client(transport=httpx.MockTransport(respond)) as client:
        monkeypatch.setattr(runtime, "ProxyChatOpenAI", lambda **kwargs: proxy(
            http_client=client, callbacks=[recorded], **kwargs))
        runtime.run(request_doc, lambda _: None)
    usage = recorded.usage_metadata["fixture-model"]
    expected_input, output, read, creation, charge = ((1200, 6, 900, 200, 440) if corrected_total
                                                      else (1000, 5, 800, 100, 305))
    assert usage["input_tokens"] == expected_input, "only the last usage chunk describes the call total"
    assert usage["output_tokens"] == output and usage["total_tokens"] == expected_input + output
    assert usage["input_token_details"] == {"cache_read": read, "cache_creation": creation}
    assert budgets[0].spent == charge


def test_session_identity_reaches_header_and_body_across_turns_and_resume(request_doc, monkeypatch, tmp_path):
    Path(request_doc["key_file"]).write_text("fixture-only-key")
    request_doc.update(role="maker", session_file=str(tmp_path / "session.json"), last_accepted_generation=6)
    request_doc["operation"]["generation"] = 7
    request_doc["inputs"]["attempt"] = 1
    Path(request_doc["session_file"]).with_suffix(".head").write_text(json.dumps(
        {"generation": 7, "head": "a" * 40}))
    sent = []
    stream_options = []

    def respond(request):
        body = json.loads(request.content)
        sent.append((request.headers.get("X-Session-ID"), body.get("prompt_cache_key")))
        stream_options.append(body.get("stream_options"))
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
    assert all(options == {"include_usage": True} for options in stream_options)
    assert "fixture-only-key" not in json.dumps(sent)


def test_interrupted_stream_keeps_last_usage_once(request_doc, tmp_path, monkeypatch):
    from langchain_openai import ChatOpenAI
    from langchain_core.messages import AIMessageChunk
    from langchain_core.outputs import ChatGenerationChunk
    from openai import APIConnectionError
    from tl3_workers.facts import observe

    completion = tmp_path / 'facts.json'
    request_doc['trace'] = {'completion': str(completion)}
    request_doc['key_file'] = str(tmp_path / 'fixture-key')
    Path(request_doc['key_file']).write_text('fixture-key-value')
    usage = {'input_tokens': 1000, 'output_tokens': 5, 'total_tokens': 1005,
             'input_token_details': {'cache_read': 800}}

    def interrupted(self, *args, **kwargs):
        yield ChatGenerationChunk(message=AIMessageChunk(content='piece', usage_metadata=usage))
        yield ChatGenerationChunk(message=AIMessageChunk(content='', usage_metadata=usage))
        raise APIConnectionError(message='secret response body', request=httpx.Request(
            'POST', 'http://model.invalid/v1'))

    monkeypatch.setattr(ChatOpenAI, '_stream', interrupted)
    with pytest.raises(APIConnectionError), observe(request_doc):
        list(runtime.create_model(request_doc).stream('fixture'))
    row = json.loads(completion.read_text())
    assert (row['input_tokens'], row['cached_tokens'], row['output_tokens']) == (1000, 800, 5)
    assert row['session_id'] == runtime.session_identity(request_doc)
    assert row['failure']['origin'] == 'proxy'
    assert 'secret response body' not in completion.read_text()
