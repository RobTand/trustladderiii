"""A dropped model turn retries once; an invalid reply gets one tool-free repair turn."""
import copy
from dataclasses import replace
import json
from pathlib import Path

import httpx
from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage, HumanMessage, ToolMessage
import openai
import pytest

from tl3_workers import runtime
from tl3_workers.turns import InvalidReply
from message_invariants import agent_calls, assert_tool_pairs
import message_invariants as test_maker
from pb_fixtures import machine_cli, supported_fake_runtime as supported_fake_runtime
import test_prompt_cache
import test_reviewer
import test_workers


@pytest.fixture(autouse=True)
def classifier_validation_boundary(monkeypatch):
    # These tests exercise turn orchestration. Rust owns classifier decisions.
    monkeypatch.setitem(runtime.ROLES, "classifier", replace(
        runtime.ROLES["classifier"], check_reply=None,
    ))
    from tl3_workers.reply_check import ReplyViolation
    from tl3_workers.roles import reviewer

    def validate_reviewer(role, inputs, reply):
        assert role == "reviewer"
        if reply == ROLES["reviewer"][3]:
            raise ReplyViolation(ROLES["reviewer"][4])

    monkeypatch.setattr(reviewer, "validate_reply", validate_reviewer)


def test_turn_fixture_keeps_the_real_budget_validator():
    from tl3_workers.budget_check import check
    assert check("finalizing", maker=True, turns=0, no_progress=0,
                 allowed=4, window=3, reserve=1) is False


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
        self.offered = bool(tools) and kwargs.get("tool_choice") != "none"
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


def instructions(messages):
    return [m for m in messages if isinstance(m, HumanMessage) and not m.text.startswith("TL3 execution budget: ")]


@pytest.mark.parametrize("role", ROLES)
def test_dropped_turn_retries_once_with_the_same_history(role, request, tmp_path, monkeypatch):
    doc = prepare(role, request, tmp_path)
    _, tool, valid, _, _ = ROLES[role]
    model = install(monkeypatch, [*([tool] if tool else []), dropped(), reply(valid)])
    events = []
    runtime.run(doc, events.append)
    assert events[-1]["event"] == "reply" and events[-1]["role"] == role
    failed, retried = agent_calls(model.calls)[-2][0], agent_calls(model.calls)[-1][0]
    assert payload(failed) == payload(retried), "the retry resends the turn with its history"
    assert len(tool_results(retried)) == (1 if tool else 0), "earlier tool calls are not repeated"
    for messages, _ in agent_calls(model.calls):
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


@pytest.mark.durability
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
    assert len(instructions(resumed)) == 1, "the visit's inputs are not repeated"
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
    repair, offered = agent_calls(model.calls)[-1]
    # A code-owned budget notice may follow the repair instruction.
    instruction = instructions(repair)[-1]
    if role in ('classifier', 'maker'):
        assert 'schema.' in instruction.text and 'Allowed:' in instruction.text
    else:
        assert message in instruction.text
    assert not offered, "the repair turn disables tool calls"
    if tool:
        assert agent_calls(model.calls)[0][1], "earlier turns keep their tools"
    assert len(tool_results(repair)) == (1 if tool else 0), "earlier tool calls are not repeated"
    assert len(agent_calls(model.calls)) == (3 if tool else 2)


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


SCOPE_REPAIR = "Remove the generated-output prohibition: the source restricts hand edits only"


def generated_scope_request(doc):
    doc["inputs"]["issue"]["body"] = (
        "Do not hand-edit generated BUILD files; configured regeneration is permitted.")
    doc["inputs"]["tool_capabilities"] = [{
        "tool": "run_generator", "executor": "flux_generator", "runner_features": [],
        "artifacts": [], "methods": [], "protected_files": []}]
    return doc


def frozen_brief(field):
    invalid = copy.deepcopy(test_workers.BRIEF)
    invalid["evidence_contract"] = []
    freeze = "Leave generated BUILD files untouched. private-request-canary"
    if field == "brief":
        invalid["brief"] = freeze
    else:
        invalid["goals"][0]["description"] = freeze
    return invalid


@pytest.mark.parametrize("field", ["brief", "goal"])
def test_generated_freeze_repaired_before_audit_once(request_doc, monkeypatch, field):
    from tl3_workers.roles.audit import ROLE as AUDIT
    generated_scope_request(request_doc)
    corrected = copy.deepcopy(test_workers.BRIEF)
    corrected["evidence_contract"] = []
    corrected["brief"] = test_workers.REJECTED_FREEZES[0]
    model = install(monkeypatch, [reply(frozen_brief(field)), reply(corrected)])
    events = []
    runtime.run(request_doc, events.append)
    accepted = events[-1]["reply"]
    assert len(model.calls) == 2
    repair, offered = model.calls[-1]
    assert repair[-1].text == (
        f"Your reply failed validation: {SCOPE_REPAIR}\nTools are unavailable for this turn. Return one "
        "corrected JSON object matching your reply schema and nothing else.")
    assert not offered
    assert accepted == {**corrected, "evidence_errors": []}
    assert [event for event in events if event["event"] == "reply"] == [
        {"event": "reply", "role": "ingest", "reply": accepted}]
    audit = {**copy.deepcopy(test_workers.AGREE), "evidence_contract": []}
    AUDIT.check_reply({**request_doc["inputs"], "ingest": accepted}, audit)
    assert audit == {**test_workers.AGREE, "evidence_contract": [], "evidence_errors": []}


def test_generated_freeze_second_invalid_reply_is_bounded_and_safe(request_doc, tmp_path, monkeypatch):
    generated_scope_request(request_doc)
    request_doc["memory"] = {"summary_file": str(tmp_path / "scope-summary.json")}
    model = install(monkeypatch, [reply(frozen_brief("brief")), reply(frozen_brief("goal"))])
    events = []
    with pytest.raises(InvalidReply) as raised:
        runtime.run(request_doc, events.append)
    assert len(model.calls) == 2
    assert not model.calls[-1][1]
    assert all(type(error) is not ValueError and str(error) == SCOPE_REPAIR for error in raised.value.errors)
    assert not any(event["event"] == "reply" for event in events)
    reason = json.loads(Path(request_doc["memory"]["summary_file"]).read_text())["reason"]
    assert reason == {"failure_class": "InvalidReply", "failure_message": (
        f"invalid reply: {SCOPE_REPAIR}; repaired reply: {SCOPE_REPAIR}")}
    assert "private-request-canary" not in json.dumps(reason)


def test_generated_freeze_without_repair_budget_keeps_safe_cause(request_doc, monkeypatch):
    from tl3_workers.tracing import failure_tags
    generated_scope_request(request_doc)
    model = install(monkeypatch, [reply(frozen_brief("brief"), input_tokens=160_000)])
    with pytest.raises(ValueError) as raised:
        runtime.run(request_doc, lambda _: None)
    assert len(model.calls) == 1
    assert type(raised.value) is not ValueError
    assert failure_tags(raised.value)["failure_message"] == SCOPE_REPAIR
    assert failure_tags(ValueError(SCOPE_REPAIR + " private-request-canary"))["failure_message"] == "invalid worker data"


@pytest.mark.parametrize("source,capability,conflict", [
    ("Never regenerate generated files.", True, True),
    ("Do not hand-edit generated files.", False, False),
])
def test_generated_scope_source_and_capability_controls(request_doc, monkeypatch, source, capability, conflict):
    from tl3_workers.roles.audit import ROLE as AUDIT
    generated_scope_request(request_doc)
    request_doc["inputs"]["issue"]["body"] = source
    if not capability:
        request_doc["inputs"].pop("tool_capabilities")
    invalid = frozen_brief("brief")
    model = install(monkeypatch, [reply(invalid)])
    events = []
    runtime.run(request_doc, events.append)
    accepted = events[-1]["reply"]
    expected = {**invalid, "evidence_errors": []} if capability else invalid
    assert accepted == expected and len(model.calls) == 1
    audit = {**copy.deepcopy(test_workers.AGREE), "evidence_contract": []}
    AUDIT.check_reply({**request_doc["inputs"], "ingest": accepted}, audit)
    assert audit["verdict"] == ("disagree" if conflict else "agree")
    if conflict:
        assert any(source in objection and "issue.body" in objection for objection in audit["objections"])


def test_generated_scope_follows_the_admitted_run_generator_tool_not_its_executor_name():
    from tl3_workers.generated_scope import generation_scope_conflicts
    issue = {"title": "Regenerate outputs", "body": "Never regenerate generated files."}
    brief = {"kind": "brief", "brief": "Change the generator input.", "goals": []}

    def conflicts(*capabilities):
        return generation_scope_conflicts({"issue": issue, "tool_capabilities": list(capabilities)}, brief)

    def capability(tool, executor):
        return {"tool": tool, "executor": executor, "runner_features": [], "artifacts": [],
                "methods": [], "protected_files": []}

    expected = [("issue.body", "Never regenerate generated files.")]
    assert conflicts(capability("run_generator", "flux_generator")) == expected
    assert conflicts(capability("run_generator", "any_backend_generator")) == expected
    assert conflicts(capability("run_tests", "flux_pytest")) == []
    assert conflicts() == []


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
@pytest.mark.durability
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
    assert not tool_results(first) and len(instructions(first)) == 1
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
    for name in ("pbrun.py", "pbwait.py"):
        script = scripts / name
        script.write_text(machine_cli(script.read_text()))
    return scripts


@pytest.mark.durability
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


@pytest.mark.durability
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
    instruction = instructions(resumed)[-1]
    assert "may have run" in instruction.text


@pytest.mark.durability
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

    first = install(monkeypatch, [*[usage(10_000) for _ in range(4)], usage(71_000), usage(20_000),
                                  dropped(), dropped()])
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


@pytest.mark.durability
def test_resume_keeps_an_exhausted_budget(maker_request, tmp_path, monkeypatch):
    from dataclasses import replace
    from tl3_workers.exploration import ExplorationBudget
    from tl3_workers.roles import maker

    visit = maker_visit(maker_request, tmp_path)
    monkeypatch.setattr(maker, "ROLE", replace(maker.ROLE, exploration_budget=ExplorationBudget(
        tool_turns=0, input_tokens=6_000_000, history_bytes=65_536)))

    def killed(*args):
        raise Killed()

    budget_reply = maker._budget_reply
    monkeypatch.setattr(maker, "_budget_reply", killed)
    interrupted(maker_request, monkeypatch, [reply(test_maker.DONE)])
    assert json.loads(visit.read_text())["budget_exhausted"] is True
    # A tools-free done with unverified obligations is now a typed budget stop.
    monkeypatch.setattr(maker, "_budget_reply", budget_reply)
    model, events = rerun(maker_request, monkeypatch, [reply(test_maker.DONE), reply(test_maker.DONE)])
    assert events[-1]["reply"]["kind"] == "cant_meet"
    assert events[-1]["reply"]["budget_exhaustion"]["tool_turns"] == {"used": 0, "allowed": 0}
    assert len(agent_calls(model.calls)) == 1


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
    assert 'schema.additionalproperties' in repair.text
    assert '/extra' in repair.text and 'remove' in repair.text
    assert "x" * 100 not in repair.text


@pytest.mark.durability
def test_environment_evidence_survives_a_checkpoint_round_trip():
    from tl3_workers.tools import Evidence

    evidence = Evidence()
    key = "c" * 64
    head = (False, ("tests/test_a.py::test_one",))
    evidence.test_result({"status": "infrastructure_error", "action_key": key}, head)
    error = evidence.tool_error("read", "os_error", head)
    restored = Evidence()
    restored.restore(json.loads(json.dumps(evidence.state())))
    assert restored.detail({"action_key": key}) == {"class": "test_infrastructure"}
    assert restored.detail({"tool_error": error}) == {"tool": "read", "class": "os_error"}
    # A later complete run of the same selectors still retires the restored entries.
    restored.test_result({"status": "completed"}, head)
    assert restored.detail({"action_key": key}) is None and restored.detail({"tool_error": error}) is None


@pytest.mark.parametrize('message, expected', [
    ('ordinary reviewer failure', 'ordinary reviewer failure'),
    ('head-' + 'x' * 3000 + '-tail', 'head-' + 'x' * 1531 + ' [...] ' + 'x' * 495 + '-tail'),
], ids=['short', 'truncated'])
def test_non_planner_repair_message_is_byte_identical(message, expected):
    from tl3_workers.turns import repair_message

    assert repair_message(ValueError(message)).encode() == expected.encode()


def test_repair_reports_schema_and_independent_missing_contract():
    from tl3_workers.turns import TurnControl, repaired
    control, history = TurnControl(), []
    valid = {**test_workers.EARLY, 'evidence_contract': []}
    replies = iter([{**test_workers.EARLY, 'outcome': 'too_big'}, valid])
    seen = []

    def ask():
        seen.append(control.repairing)
        return next(replies)

    result = repaired(control, history, ask, lambda value: runtime._checked(
        runtime.ROLES['ingest'], {'tool_capabilities': []}, value))
    assert result['outcome'] == valid['outcome']
    refusal = history[-1].text
    assert 'evidence.contract_missing' in refusal
    assert '/evidence_contract' in refusal and 'add' in refusal
    assert '/outcome' in refusal and 'too_big' in refusal
    assert 'already_done' in refusal and 'duplicate' in refusal
    assert 'Allowed' in refusal and 'replace' in refusal
    assert seen == [False, True] and control.repair_attempts == 1


@pytest.mark.parametrize('constraint,bad,good', [
    ({'enum': ['approve', 'changes']}, 'maybe', 'approve'),
    ({'const': 'done'}, 'maybe', 'done'),
    ({'pattern': '^ok$'}, 'bad', 'ok'),
    ({'type': 'integer'}, 'bad', 2),
    ({'minLength': 2}, '', 'ok'),
])
def test_schema_row_rendered_repair_passes_its_guard(constraint, bad, good):
    from jsonschema import Draft202012Validator
    from tl3_workers.contracts import schema_violations
    from tl3_workers.rules import apply_edit, permits, register
    validator = Draft202012Validator({'properties': {'a/b~c': constraint}})
    original = {'a/b~c': bad, 'untouched': True}
    violations = schema_violations(validator.iter_errors(original))
    assert len(violations) == 1
    violation = violations[0]
    text = violation.render(role='schema')
    assert '/a~1b~0c' in text and 'Allowed' in text and 'replace' in text
    assert json.dumps(next(iter(constraint.values()))) in text
    facts = {**violation.facts, 'reply': original}
    register(replace(violation.rule, example=facts, witness=good), roles=('schema',))
    fixed = apply_edit(original, violation.rule.repair(facts), good)
    assert permits(violations, original, fixed, role='schema')
    assert validator.is_valid(fixed)
    assert not permits(violations, original, {**fixed, 'untouched': False}, role='schema')
    assert not permits(violations, original, {**fixed, 'untouched': 1}, role='schema')


def test_evidence_row_has_a_bounded_renderable_repair():
    from tl3_workers.evidence import CONTRACT_MISSING, contract_violations
    from tl3_workers.rules import apply_edit, permits, register
    facts = {'inputs': {'tool_capabilities': []}, 'reply': {'kind': 'brief'}}
    register(replace(CONTRACT_MISSING, example=facts, witness=[]), roles=('ingest',))
    violations = contract_violations(**facts)
    text = violations[0].render(role='ingest')
    assert '/evidence_contract' in text and 'array' in text and 'add' in text
    fixed = apply_edit(facts['reply'], CONTRACT_MISSING.repair(violations[0].facts), [])
    assert permits(violations, facts['reply'], fixed, role='ingest')
    assert not contract_violations(facts['inputs'], fixed)
    assert not permits(violations, facts['reply'], {**fixed, 'evidence_contract': 'not an array'}, role='ingest')


def test_registration_rejects_unrenderable_and_self_rejecting_rows():
    from tl3_workers.evidence import CONTRACT_MISSING, EVIDENCE_FIELD
    from tl3_workers.rules import Allowed, CantMeet, Edit, register
    facts = {'inputs': {'tool_capabilities': []}, 'reply': {}}
    with pytest.raises(ValueError, match='renderable repair'):
        register(replace(CONTRACT_MISSING, repair=lambda facts: None, example=facts, witness=[]), roles=('ingest',))
    with pytest.raises(ValueError, match='renderable repair'):
        register(replace(CONTRACT_MISSING, repair=lambda facts: CantMeet(''), example=facts), roles=('ingest',))
    wrong = replace(CONTRACT_MISSING,
        repair=lambda facts: Edit(facts['field'], 'add', Allowed('type', 'string')),
        allowed=lambda facts: Allowed('type', 'string'))
    with pytest.raises(ValueError, match='own witness'):
        register(replace(wrong, example=facts, witness=[]), roles=('ingest',))
    self_rejecting = replace(CONTRACT_MISSING, check=lambda facts: ({'field': EVIDENCE_FIELD},))
    with pytest.raises(ValueError, match='own witness'):
        register(replace(self_rejecting, example=facts, witness=[]), roles=('ingest',))


def test_typed_subjects_and_gating_report_all_independent_violations():
    from tl3_workers.rules import Allowed, CantMeet, Edit, Name, Rule, evaluate, register, render_all
    subjects = (Name('field', '/proof/observer_support/1'), Name('path', 'tests/test_x.py'),
                Name('symbol', 'tests/test_x.py::test_x'))
    row = register(Rule('proof.subject', lambda facts: ({}, {}), lambda facts: subjects,
               lambda facts: Allowed('enum', ['observer']), lambda facts: CantMeet('subject identity differs'),
               example={'reply': {}}, witness={'kind': 'cant_meet'}), roles=('planner', 'maker'))
    text = render_all(evaluate((row,), {}, role='planner'), role='planner')
    assert text.count('proof.subject') == 2
    assert 'reply field' in text and 'source path' in text and 'symbol' in text
    assert 'Further checks were not run' not in text
    calls = []
    later = replace(row, id='proof.later', check=lambda facts: calls.append(True) or ())
    gating = replace(row, gating=True)
    found = evaluate((gating, later), {}, role='planner')
    assert not calls and len(found) == 2
    assert all('Further checks were not run' in item.render(role='planner') for item in found)
    register(replace(gating, example={'reply': {}}), roles=('planner',))
    with pytest.raises(ValueError, match='field pointer'):
        Edit(subjects[1], 'replace', Allowed('enum', ['observer']))


def test_schema_remove_extra_field_uses_the_same_guard():
    from jsonschema import Draft202012Validator
    from tl3_workers.contracts import schema_violations
    from tl3_workers.rules import apply_edit, permits, register
    validator = Draft202012Validator({'type': 'object', 'properties': {'kind': {'const': 'remark'}},
                                     'additionalProperties': False})
    original = {'kind': 'remark', 'extra': 'x' * 20_000}
    violations = schema_violations(validator.iter_errors(original))
    violation, = violations
    register(replace(violation.rule, example={**violation.facts, 'reply': original}), roles=('schema',))
    fixed = apply_edit(original, violation.rule.repair(violation.facts))
    assert permits(violations, original, fixed, role='schema') and validator.is_valid(fixed)
    assert '/extra' in violation.render(role='schema') and 'remove' in violation.render(role='schema')


def test_all_schema_rows_survive_the_legacy_message_limit():
    from jsonschema import Draft202012Validator
    from tl3_workers.contracts import schema_violations
    from tl3_workers.turns import REPAIR_MESSAGE_BYTES, repair_message
    validator = Draft202012Validator({'type': 'array', 'items': {'enum': ['ok']}})
    errors = list(validator.iter_errors(['bad'] * 30))
    errors[0].rule_violations = schema_violations(errors)
    message = repair_message(errors[0])
    assert len(message.encode()) > REPAIR_MESSAGE_BYTES
    assert message.count('schema.enum:') == 30
    assert all(f'reply field "/{index}"' in message for index in range(30))


def test_composite_schema_row_preserves_reference_context():
    from jsonschema import Draft202012Validator
    from tl3_workers.contracts import schema_violations
    from tl3_workers.rules import register
    proposal = {'$defs': {'text': {'type': 'string'}}, 'oneOf': [
        {'type': 'object', 'properties': {'kind': {'const': 'x'}, 'item': {'$ref': '#/$defs/text'}},
         'required': ['kind', 'item']},
        {'type': 'object', 'properties': {'kind': {'const': 'y'}}, 'required': ['kind']}]}
    bad = {'kind': 'unknown', 'item': 3}
    violation, = schema_violations(Draft202012Validator(proposal).iter_errors(bad))
    register(replace(violation.rule, example={**violation.facts, 'reply': bad}, witness={'kind': 'x', 'item': 'ok'}), roles=('schema',))
    assert 'oneOf' in violation.render(role='schema') and 'replace' in violation.render(role='schema')


def test_pilot_repair_rejects_changes_outside_its_row():
    from tl3_workers.turns import InvalidReply, RepairScopeError, TurnControl, repaired
    original = {**test_workers.EARLY}
    corrected = {**original, 'evidence_contract': [], 'outcome': 'already_done'}
    replies = iter([original, corrected])
    control, history = TurnControl(), []
    with pytest.raises(InvalidReply) as caught:
        repaired(control, history, lambda: next(replies), lambda value: runtime._checked(
            runtime.ROLES['ingest'], {'tool_capabilities': []}, value))
    assert isinstance(caught.value.errors[1], RepairScopeError)
    from tl3_workers.tracing import failure_tags
    assert 'repair_scope' in failure_tags(caught.value.errors[1])['failure_message']
    assert control.repair_attempts == 1 and len(history) == 1


def test_worker_rule_adapter_reuses_the_harness_core():
    from tl3_harness import rules as core
    from tl3_workers import rules as adapter
    assert adapter.Rule is core.Rule and adapter.Violation is core.Violation
    assert adapter.permits is core.permits and adapter.register is core.register
    assert adapter.PLANNER is core.terminal_for('planner') and adapter.MAKER is core.terminal_for('maker')
    assert isinstance(adapter.Allowed('type', 'array'), core.Allowed)
    assert adapter.Allowed('type', 'array').accepts([])
    assert not adapter.Allowed('type', 'array').accepts('not an array')


@pytest.mark.parametrize('nested', [False, True], ids=['root', 'nested'])
def test_required_rows_add_only_the_missing_properties(nested):
    from jsonschema import Draft202012Validator
    from tl3_workers.contracts import schema_violations
    from tl3_workers.rules import apply_edit, permits, register
    required = {'type': 'object', 'required': ['a/b~c', 'count'], 'properties': {
        'a/b~c': {'$ref': '#/$defs/name'}, 'count': {'type': 'integer'}}}
    proposal = {'$defs': {'name': {'enum': ['ok']}}}
    proposal.update({'properties': {'box': required}} if nested else required)
    validator = Draft202012Validator(proposal)
    before = {'untouched': True, **({'box': {'kept': True}} if nested else {})}
    violations = schema_violations(validator.iter_errors(before), proposal)
    prefix = '/box' if nested else ''
    assert len(violations) == 2
    assert {item.facts['pointer'] for item in violations} == {prefix + '/a~1b~0c', prefix + '/count'}
    fixed = before
    for item in violations:
        edit = item.rule.repair(item.facts)
        value = 'ok' if edit.pointer.value.endswith('/a~1b~0c') else 3
        assert edit.op == 'add' and edit.allowed.accepts(value)
        assert 'add reply field' in item.render(role='schema')
        register(replace(item.rule, example={**item.facts, 'reply': before}, witness=value), roles=('schema',))
        fixed = apply_edit(fixed, edit, value)
    assert validator.is_valid(fixed) and permits(violations, before, fixed, role='schema')
    assert not permits(violations, before, {**fixed, 'untouched': False}, role='schema')
    if nested:
        assert not permits(violations, before, {**fixed, 'box': {**fixed['box'], 'kept': False}}, role='schema')


def test_cant_meet_registration_requires_a_terminal_reply_form():
    from tl3_workers.rules import Allowed, CantMeet, Name, Rule, register
    row = Rule('proof.identity', lambda facts: ({},),
               lambda facts: (Name('symbol', 'tests/test_x.py::test_x'),),
               lambda facts: Allowed('enum', []), lambda facts: CantMeet('identity differs'))
    with pytest.raises(ValueError, match='terminal reply form'):
        register(replace(row, example={'reply': {}}, witness={'kind': 'cant_meet'}), roles=('postmortem',))


def test_allowed_value_equality_crosses_the_adapter_boundary():
    from tl3_harness.rules import Allowed as CoreAllowed
    from tl3_workers.rules import Allowed
    assert CoreAllowed('enum', ['ok']) == Allowed('enum', ['ok'])
    assert Allowed('enum', ['ok']) == CoreAllowed('enum', ['ok'])
    assert CoreAllowed('const', True) != Allowed('const', 1)
    from tl3_workers.rules import Edit, Name, Rule, register
    row = register(Rule('test.membership', lambda facts: () if facts['reply']['value'] == 'ok' else ({},),
        lambda facts: (Name('field', '/value'),), lambda facts: CoreAllowed('enum', ['ok']),
        lambda facts: Edit(Name('field', '/value'), 'replace', Allowed('enum', ['ok'])),
        example={'reply': {'value': 'bad'}}, witness='ok'), roles=('schema',))
    assert row.id == 'test.membership'


def test_symbol_names_use_source_path_and_python_qualname():
    from tl3_workers.rules import Name
    assert Name('symbol', 'pkg/mod.py::Class.method').value == 'pkg/mod.py::Class.method'
    for invalid in ('tl3_workers.roles.postmortem::envelope', 'pkg/mod.py::Class::method',
                    'pkg/mod.py::Class..method'):
        with pytest.raises(ValueError, match='path::qualname'):
            Name('symbol', invalid)


def test_planner_terminal_reply_passes_the_one_repair_turn():
    from tl3_workers.rules import Allowed, CantMeet, Name, Rule, evaluate, permits, register, terminal_for
    from tl3_workers.turns import TurnControl, repaired
    before, terminal = {'kind': 'leaf'}, {'kind': 'cant_meet', 'report': 'subject identity differs'}
    row = register(Rule('proof.identity', lambda facts: ({},),
        lambda facts: (Name('symbol', 'tests/test_x.py::test_x'),),
        lambda facts: Allowed('enum', []), lambda facts: CantMeet('identity differs'),
        example={'reply': before}, witness=terminal), roles=('planner', 'maker'))
    violations = evaluate((row,), {'reply': before}, role='planner')
    assert permits(violations[0], before, terminal, role='planner')
    assert permits(row, before, terminal, role='maker')
    assert not permits(violations, before, {'kind': 'leaf', 'edited': True}, role='planner')
    for role in ('planner', 'maker'):
        assert terminal_for(role).phrase in violations[0].render(role=role)
    error = ValueError('refused')
    error.rule_violations, error.rule_role = violations, 'planner'

    def check(reply):
        if reply['kind'] != 'cant_meet':
            raise error
        return reply

    replies, history, control = iter((before, terminal)), [], TurnControl()
    assert repaired(control, history, lambda: next(replies), check) == terminal
    assert control.repair_attempts == 1 and len(history) == 1


def test_source_edit_never_enters_a_tool_free_reply_repair():
    from tl3_harness.rules import Allowed, Name, Rule, SourceEdit, evaluate
    from tl3_workers.turns import repaired
    row = Rule('proof.source', lambda facts: ({},),
               lambda facts: (Name('path', 'tests/test_observer.py'),),
               Allowed('const', 'observer test body'),
               SourceEdit('tests/test_observer.py', 'Move the call into the test body.'),
               roles=frozenset({'maker'}))
    error = ValueError('source repair required')
    error.rule_violations = evaluate((row,), {}, role='maker')
    error.rule_role = 'maker'
    asks = []

    def check(text):
        raise error

    with pytest.raises(ValueError, match='source repair required') as caught:
        repaired(object(), [], lambda: asks.append(True) or '{}', check)
    assert caught.value is error
    assert asks == [True]
