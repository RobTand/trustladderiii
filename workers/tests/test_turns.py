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
        if callable(step):
            step = step()
        if isinstance(step, BaseException):
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


class Killed(BaseException):
    """Stands in for SIGKILL: no handler in the worker can catch it."""


def maker_visit(maker_request, tmp_path):
    maker_request["inputs"]["context_size"] = 150_000
    maker_request["session_file"] = str(tmp_path / "sessions/1/plan-0/attempt-1.json")
    maker_request["resume_session"] = False
    return Path(maker_request["session_file"]).with_suffix(".visit")


def rerun(maker_request, monkeypatch, steps):
    # RunFailed advances the generation; no reply was accepted.
    maker_request["operation"]["generation"] += 1
    model = install(monkeypatch, steps)
    events = []
    runtime.run(maker_request, events.append)
    return model, events


def interrupted(maker_request, monkeypatch, steps):
    install(monkeypatch, steps)
    with pytest.raises((Killed, openai.APIConnectionError)):
        runtime.run(maker_request, lambda _: None)


@pytest.mark.parametrize("damage", ["unversioned", "future", "not_json", "bad_messages"])
def test_unusable_visit_checkpoint_starts_the_visit_afresh(maker_request, tmp_path, monkeypatch, capsys, damage):
    visit = maker_visit(maker_request, tmp_path)
    interrupted(maker_request, monkeypatch, [ROLES["maker"][1], dropped(), dropped()])
    saved = json.loads(visit.read_text())
    if damage == "unversioned":
        saved.pop("version", None)
    elif damage == "future":
        saved["version"] = 999
    elif damage == "bad_messages":
        saved["messages"] = [{"type": "no-such-message"}]
    visit.write_text("{not json" if damage == "not_json" else json.dumps(saved))
    capsys.readouterr()
    model, events = rerun(maker_request, monkeypatch, [ROLES["maker"][1], reply(test_maker.DONE)])
    assert events[-1]["reply"]["kind"] == "done"
    first = model.calls[0][0]
    assert not tool_results(first) and sum(isinstance(m, HumanMessage) for m in first) == 1
    assert "tl3_workers: maker: visit checkpoint unusable; starting the visit afresh" in capsys.readouterr().err


def pb_scripts(tmp_path):
    scripts = tmp_path / "pb-tools"
    scripts.mkdir()
    (scripts / "pbrun.py").write_text(
        "import json, pathlib\n"
        "log = pathlib.Path(__file__).with_name('submitted.log')\n"
        "log.write_text(log.read_text() + 'x' if log.exists() else 'x')\n"
        "print(json.dumps({'schema': 'prismabuild.submission.v1', 'action_key': 'a' * 64}))\n")
    (scripts / "pbwait.py").write_text(
        "import hashlib, json, pathlib, sys\n"
        "here = pathlib.Path(__file__).parent\n"
        "(here / 'waited.log').open('a').write(sys.argv[-1] + '\\n')\n"
        "summary = {'status': 'completed', 'totals': {'passed': 1, 'failed': 0, 'error': 0, 'skipped': 0},\n"
        "           'tests': [{'test': 'test_greeting.py::test_hello', 'outcome': 'passed', 'message': ''}],\n"
        "           'note': ''}\n"
        "raw = ('TL3_TEST_SUMMARY ' + json.dumps(summary) + '\\n').encode()\n"
        "(here / 'action.log').write_bytes(raw)\n"
        "ending = {'action_key': 'a' * 64, 'status': 'executed', 'succeeded': True, 'exit_code': 0,\n"
        "          'result': {'verified': True, 'receipt_sha256': 'b' * 64, 'stdout': {\n"
        "              'path': str(here / 'action.log'), 'bytes': len(raw), 'sha256': hashlib.sha256(raw).hexdigest()}}}\n"
        "print(json.dumps({'schema': 'prismabuild.endings.v1', 'complete': True, 'endings': [ending]}))\n")
    return scripts


def test_resume_reattaches_to_the_in_flight_test_action(maker_request, tmp_path, monkeypatch):
    from tl3_workers import pb_tests

    maker_visit(maker_request, tmp_path)
    scripts = pb_scripts(tmp_path)
    maker_request["inputs"]["tests"]["tools_path"] = str(scripts)
    original = pb_tests._json_command
    killing = True

    def command(args):
        if killing and args[1].endswith("pbwait.py"):
            raise Killed()
        return original(args)

    monkeypatch.setattr(pb_tests, "_json_command", command)
    tests = call("run_tests", {"tests": ["test_greeting.py::test_hello"]})
    interrupted(maker_request, monkeypatch, [ROLES["maker"][1], tests])
    assert (scripts / "submitted.log").read_text() == "x"
    killing = False
    model, events = rerun(maker_request, monkeypatch, [reply(test_maker.DONE)])
    assert events[-1]["reply"]["kind"] == "done"
    assert (scripts / "submitted.log").read_text() == "x", "the resumed run waits; it never resubmits"
    assert (scripts / "waited.log").read_text() == "a" * 64 + "\n"
    results = tool_results(model.calls[0][0])
    assert results[-1].tool_call_id == "run_tests" and '"status": "completed"' in results[-1].text
    assert_tool_pairs(model.calls[0][0])


def test_resume_warns_that_an_interrupted_file_tool_may_have_run(maker_request, tmp_path, monkeypatch):
    from tl3_workers import tools

    maker_visit(maker_request, tmp_path)
    factory = tools.TOOL_FACTORIES["read"]

    def crashing(worktree):
        def read(path: str) -> str:
            """Read a file."""
            raise Killed()
        return read

    monkeypatch.setitem(tools.TOOL_FACTORIES, "read", crashing)
    interrupted(maker_request, monkeypatch, [ROLES["maker"][1], call("read", {"path": "greeting.py"})])
    monkeypatch.setitem(tools.TOOL_FACTORIES, "read", factory)
    model, _ = rerun(maker_request, monkeypatch, [reply(test_maker.DONE)])
    resumed = model.calls[0][0]
    assert_tool_pairs(resumed)
    assert isinstance(resumed[-1], HumanMessage) and "may have run" in resumed[-1].text


def test_resume_keeps_compaction_and_cache_pricing(maker_request, tmp_path, monkeypatch):
    maker_visit(maker_request, tmp_path)
    maker_request["inputs"]["context_size"] = 100_000
    Path(maker_request["worktree"], "evidence.txt").write_text(("source evidence " * 7 + "\n") * 200)
    budgets = test_prompt_cache.capture_budget(monkeypatch)
    read = call("read", {"path": "evidence.txt"})
    cached = {"input_token_details": {"cache_read": 9_000}}

    def usage(tokens):
        message = copy.deepcopy(read)
        message.usage_metadata = {"input_tokens": tokens, "output_tokens": 1, "total_tokens": tokens + 1,
                                  **cached}
        return message

    first = install(monkeypatch, [usage(10_000), usage(71_000), usage(20_000), dropped(), dropped()])
    with pytest.raises(openai.APIConnectionError):
        runtime.run(maker_request, lambda _: None)
    assert budgets[0].compacted
    seen = []

    def observe():
        seen.append((budgets[-1].compacted, budgets[-1].cache_active))
        return ROLES["maker"][1]

    model, _ = rerun(maker_request, monkeypatch, [observe, reply(test_maker.DONE)])
    assert payload(model.calls[0][0]) == payload(first.calls[-1][0]), "the compacted prefix is resumed as sent"
    assert seen == [(True, True)], "resumed calls are priced warm and do not compact again"


def test_resume_keeps_an_exhausted_budget(maker_request, tmp_path, monkeypatch):
    from dataclasses import replace
    from tl3_workers.exploration import ExplorationBudget
    from tl3_workers.roles import maker

    visit = maker_visit(maker_request, tmp_path)
    monkeypatch.setattr(maker, "ROLE", replace(maker.ROLE, exploration_budget=ExplorationBudget(
        tool_turns=0, input_tokens=6_000_000, history_bytes=65_536)))

    def killed(*args):
        raise Killed()

    monkeypatch.setattr(maker, "_commit", killed)
    interrupted(maker_request, monkeypatch, [reply(test_maker.DONE)])
    assert json.loads(visit.read_text())["budget_exhausted"] is True


def test_dropped_call_retry_keeps_the_final_answer_reserve(request_doc, monkeypatch):
    # The first exploration call leaves room for one more call plus the final answer.
    # Its dropped attempt is charged, so a retry would eat the reserve.
    model = install(monkeypatch, [call("read", {"path": "evidence.txt"}), dropped(), reply(test_workers.BRIEF)])
    model.steps[0].usage_metadata = {"input_tokens": 80_000, "output_tokens": 1, "total_tokens": 80_001}
    Path(request_doc["worktree"], "evidence.txt").write_text("source evidence\n")
    with pytest.raises(openai.APIConnectionError):
        runtime.run(request_doc, lambda _: None)
    assert len(model.calls) == 2


def test_repair_prompt_caps_an_echoed_invalid_reply(tmp_path, monkeypatch):
    doc = classifier_request(tmp_path)
    model = install(monkeypatch, [reply({"kind": "remark", "extra": "x" * 20_000}), reply({"kind": "remark"})])
    runtime.run(doc, lambda _: None)
    repair = model.calls[-1][0][-1]
    assert isinstance(repair, HumanMessage)
    assert len(repair.text.encode()) <= 2_048 + 512
    assert "oneOf" in repair.text
