"""A dropped model turn retries once; an invalid reply gets one tool-free repair turn."""
import copy
import json
from pathlib import Path

import httpx
from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage, HumanMessage, ToolMessage
import openai
import pytest

from tl3_workers import runtime
from tl3_workers.turns import InvalidReply
from message_invariants import assert_tool_pairs
import test_maker
import test_prompt_cache
import test_reviewer
import test_workers


maker_request = test_maker.maker_request
review_request = test_reviewer.review_request
request_doc = test_workers.request_doc


def dropped():
    return openai.APIConnectionError(request=httpx.Request("POST", "http://model.invalid/v1/chat/completions"))


def call(name, args):
    return AIMessage(content="", tool_calls=[{"name": name, "args": args, "id": name, "type": "tool_call"}])


def reply(value, **usage):
    message = AIMessage(content=json.dumps(value))
    if usage:
        message.usage_metadata = {**usage, "output_tokens": 1, "total_tokens": usage["input_tokens"] + 1}
    return message


class Scripted(FakeMessagesListChatModel):
    """Plays steps in order. An exception step is raised by that model call."""
    steps: list
    calls: list = []
    offered: bool = False

    def bind_tools(self, tools, **kwargs):
        self.offered = bool(tools)
        return self

    def _generate(self, messages, **kwargs):
        self.calls.append((copy.deepcopy(messages), self.offered))
        self.offered = False
        step = self.steps.pop(0)
        if isinstance(step, Exception):
            raise step
        self.responses, self.i = [step], 0
        return super()._generate(messages, **kwargs)


def classifier_request(tmp_path):
    return {
        "protocol": 1, "operation": {"node": 1, "generation": 0, "kind": "classifier"},
        "role": "classifier",
        "model": {"base_url": "http://model.invalid/v1", "name": "fixture-model",
                  "effort": "high", "budget_tokens": 32000},
        "key_file": str(tmp_path / "never-read-this-key"),
        "inputs": {"comment": {"author": "requester", "body": "Thanks for the update."},
                   "state": {"phase": "Making", "closed": False, "open_question": None}},
        "worktree": str(tmp_path),
    }


# role: (request fixture, tool step, valid reply, invalid reply, text of its validation message)
ROLES = {
    "ingest": ("request_doc", call("read", {"path": "evidence.txt"}), test_workers.BRIEF,
               {**test_workers.BRIEF, "memory_ids": ["not-supplied"]}, "reply cites a memory it was not given"),
    "classifier": (None, None, {"kind": "remark"}, {"kind": "maybe"}, "is not valid under any of the given schemas"),
    "reviewer": ("review_request", call("read", {"path": "greeting.py"}), test_reviewer.APPROVE,
                 {**test_reviewer.APPROVE, "head_sha": "c" * 40}, "approval names a head other than the reviewed head"),
    "maker": ("maker_request", call("write", {"path": "greeting.py", "content": "def greet(): return 'hello'\n"}),
              test_maker.DONE, {"kind": "done"}, "is not valid under any of the given schemas"),
}


def prepare(role, request, tmp_path):
    fixture = ROLES[role][0]
    doc = classifier_request(tmp_path) if fixture is None else request.getfixturevalue(fixture)
    Path(doc["worktree"], "evidence.txt").write_text("source evidence\n")
    if role == "maker":
        doc["inputs"]["context_size"] = 150_000
    return doc


def install(monkeypatch, steps):
    model = Scripted(responses=[], steps=list(steps), calls=[])
    monkeypatch.setattr(runtime, "create_model", lambda _: model)
    return model


def payload(messages):
    return test_prompt_cache.payload(messages)


def tool_results(messages):
    return [m for m in messages if isinstance(m, ToolMessage)]


@pytest.mark.parametrize("role", ROLES)
def test_dropped_turn_retries_once_with_the_same_history(role, request, tmp_path, monkeypatch):
    doc = prepare(role, request, tmp_path)
    _, tool, valid, _, _ = ROLES[role]
    model = install(monkeypatch, [*([tool] if tool else []), dropped(), reply(valid)])
    events = []
    runtime.run(doc, events.append)
    assert events[-1]["event"] == "reply" and events[-1]["role"] == role
    failed, retried = model.calls[-2][0], model.calls[-1][0]
    assert payload(failed) == payload(retried), "the retry resends the turn with its history"
    assert len(tool_results(retried)) == (1 if tool else 0), "earlier tool calls are not repeated"
    for messages, _ in model.calls:
        assert_tool_pairs(messages)


@pytest.mark.parametrize("role", ROLES)
def test_second_failure_of_the_same_turn_fails_the_run(role, request, tmp_path, monkeypatch):
    doc = prepare(role, request, tmp_path)
    tool = ROLES[role][1]
    model = install(monkeypatch, [*([tool] if tool else []), dropped(), dropped()])
    with pytest.raises(openai.APIConnectionError):
        runtime.run(doc, lambda _: None)
    assert len(model.calls) == (3 if tool else 2)


def test_stream_dropped_mid_response_retries_through_the_proxy_client(request_doc, monkeypatch):
    Path(request_doc["key_file"]).write_text("fixture-only-key")
    bodies = []

    def respond(http_request):
        bodies.append(http_request.content)
        if len(bodies) == 1:
            def broken():
                yield b'data: {"id": "x", "object": "chat.completion.chunk", "created": 0, '
                yield b'"model": "fixture-model", "choices": [{"index": 0, "delta": {"role": "assistant", '
                raise httpx.RemoteProtocolError("peer closed connection without sending complete message body")
            return httpx.Response(200, headers={"content-type": "text/event-stream"}, content=broken())
        return test_prompt_cache.response_frames(json.dumps(test_workers.BRIEF))

    proxy = runtime.ProxyChatOpenAI
    events = []
    with httpx.Client(transport=httpx.MockTransport(respond)) as client:
        monkeypatch.setattr(runtime, "ProxyChatOpenAI", lambda **kwargs: proxy(http_client=client, **kwargs))
        runtime.run(request_doc, events.append)
    assert len(bodies) == 2 and bodies[0] == bodies[1]
    assert events[-1]["reply"] == test_workers.BRIEF


def test_dropped_call_counts_against_the_input_budget(request_doc, monkeypatch):
    budgets = test_prompt_cache.capture_budget(monkeypatch)
    install(monkeypatch, [dropped(), reply(test_workers.BRIEF, input_tokens=1000)])
    runtime.run(request_doc, lambda _: None)
    assert budgets[0].spent > 1000, "the dropped call may have been billed"


def test_maker_rerun_after_worker_failure_resumes_the_saved_session(maker_request, tmp_path, monkeypatch):
    maker_request["inputs"]["context_size"] = 150_000
    maker_request["session_file"] = str(tmp_path / "sessions/1/plan-0/attempt-1.json")
    maker_request["resume_session"] = False
    budgets = test_prompt_cache.capture_budget(monkeypatch)
    write = ROLES["maker"][1]
    first = install(monkeypatch, [write, call("read", {"path": "greeting.py"}), dropped(), dropped()])
    with pytest.raises(openai.APIConnectionError):
        runtime.run(maker_request, lambda _: None)
    assert len(first.calls) == 4
    # RunFailed advances the generation; no reply was accepted.
    maker_request["operation"]["generation"] = 1
    second = install(monkeypatch, [reply(test_maker.DONE)])
    events = []
    runtime.run(maker_request, events.append)
    assert events[-1]["reply"]["kind"] == "done" and "head" in events[-1]["reply"]
    resumed = second.calls[0][0]
    assert payload(resumed) == payload(first.calls[-1][0]), "resume from the last complete tool result"
    assert sum(isinstance(m, HumanMessage) for m in resumed) == 1, "the visit's inputs are not repeated"
    assert budgets[-1].turns == budgets[0].turns + 1, "a resume does not reset the run's budgets"
    assert budgets[-1].spent >= budgets[0].spent


@pytest.mark.parametrize("role", ROLES)
def test_invalid_reply_gets_one_tool_free_repair_turn(role, request, tmp_path, monkeypatch):
    doc = prepare(role, request, tmp_path)
    _, tool, valid, invalid, message = ROLES[role]
    model = install(monkeypatch, [*([tool] if tool else []), reply(invalid), reply(valid)])
    events = []
    runtime.run(doc, events.append)
    assert events[-1]["event"] == "reply"
    repair, offered = model.calls[-1]
    assert isinstance(repair[-1], HumanMessage) and message in repair[-1].text
    assert not offered, "the repair turn offers no tools"
    if tool:
        assert model.calls[0][1], "earlier turns keep their tools"
    assert len(tool_results(repair)) == (1 if tool else 0), "earlier tool calls are not repeated"
    assert len(model.calls) == (3 if tool else 2)


@pytest.mark.parametrize("role", ROLES)
def test_reply_invalid_twice_fails_with_both_messages(role, request, tmp_path, monkeypatch):
    doc = prepare(role, request, tmp_path)
    doc["memory"] = {"summary_file": str(tmp_path / "summary.json")}
    invalid, message = ROLES[role][3:]
    install(monkeypatch, [reply(invalid), reply({"kind": "not a reply"})])
    with pytest.raises(InvalidReply) as raised:
        runtime.run(doc, lambda _: None)
    assert len(raised.value.errors) == 2 and message in str(raised.value.errors[0])
    reason = json.loads(Path(doc["memory"]["summary_file"]).read_text())["reason"]
    assert reason["failure_class"] == "InvalidReply"
    assert "; repaired reply: " in reason["failure_message"]


def test_ingest_diagnostic_keeps_both_safe_messages(request_doc, tmp_path, monkeypatch):
    request_doc["memory"] = {"summary_file": str(tmp_path / "summary.json")}
    invalid = ROLES["ingest"][3]
    install(monkeypatch, [reply(invalid), reply(invalid)])
    with pytest.raises(InvalidReply):
        runtime.run(request_doc, lambda _: None)
    reason = json.loads(Path(request_doc["memory"]["summary_file"]).read_text())["reason"]
    assert reason["failure_message"] == ("invalid reply: reply cites a memory it was not given; "
                                         "repaired reply: reply cites a memory it was not given")


def test_tool_call_in_a_repair_turn_is_never_executed(maker_request, monkeypatch):
    maker_request["inputs"]["context_size"] = 150_000
    install(monkeypatch, [ROLES["maker"][1], reply({"kind": "done"}),
                          call("write", {"path": "must-not-exist", "content": "x"})])
    with pytest.raises(InvalidReply):
        runtime.run(maker_request, lambda _: None)
    assert not Path(maker_request["worktree"], "must-not-exist").exists()


def test_no_room_for_a_repair_fails_with_the_validation_message(request_doc, monkeypatch):
    invalid = ROLES["ingest"][3]
    model = install(monkeypatch, [reply(invalid, input_tokens=160_000), reply(test_workers.BRIEF)])
    with pytest.raises(ValueError, match="reply cites a memory it was not given") as raised:
        runtime.run(request_doc, lambda _: None)
    assert not isinstance(raised.value, InvalidReply)
    assert len(model.calls) == 1, "a repair that cannot fit the input budget is not sent"
