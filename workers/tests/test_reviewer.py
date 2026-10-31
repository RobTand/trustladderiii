import copy
import json

from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage
import pytest

from tl3_workers import runtime
from tl3_workers.roles import ROLES
from tl3_workers.tools import READ_TOOLS
import worker_process
from worker_process import start_worker


BASE = "a" * 40
HEAD = "b" * 40
NOTE = {"problem": "The helper name could be clearer.", "evidence": "greeting.py:1 uses greet."}
APPROVE = {"verdict": "approve", "head_sha": HEAD, "blocking_findings": [], "findings": []}


@pytest.fixture(autouse=True)
def tool_capable_process_model(monkeypatch):
    launcher = worker_process.LAUNCH.replace(
        "model = FakeMessagesListChatModel(",
        """class ReviewerModel(FakeMessagesListChatModel):
    def bind_tools(self, tools, **kwargs):
        from tl3_workers.roles import ROLES
        assert {tool.name: tool.description for tool in tools} == dict(ROLES['reviewer'].tools)
        return self
model = ReviewerModel(""",
    )
    monkeypatch.setattr(worker_process, "LAUNCH", launcher)


def blocker(kind="goal", identifier="g1"):
    return {"citation": {"kind": kind, "id": identifier},
            "problem": "The greeting is wrong.", "evidence": "greeting.py:2 returns goodbye."}


def changes(*blocking_findings, findings=None):
    return {"verdict": "changes", "blocking_findings": list(blocking_findings),
            "findings": [] if findings is None else findings}


@pytest.fixture
def review_request(tmp_path):
    tree = tmp_path / "head"
    tree.mkdir()
    (tree / "greeting.py").write_text('def greet():\n    return "goodbye"\n')
    return {
        "protocol": 1,
        "operation": {"node": 35, "generation": 0, "kind": "review"},
        "role": "reviewer",
        "model": {"base_url": "http://model.invalid/v1", "name": "controller-picked-reviewer",
                  "effort": "high", "budget_tokens": 32000},
        "key_file": str(tmp_path / "unused-fixture-key"),
        "worktree": str(tree),
        "inputs": {
            "goals": [{"id": "g1", "description": "Return hello.", "verification": "test"},
                      {"id": "g2", "description": "Keep the interface clear.", "verification": "review"}],
            "base_sha": BASE, "head_sha": HEAD,
            "diff": '-    return "hello"\n+    return "goodbye"\n',
            "checks": [{"name": "existing greeting", "kind": "regression", "verdict": "fail",
                        "tests": [{"test": "tests/test_greeting.py::test_greet", "base": "passed",
                                   "head": "assertion", "pre_existing": False, "passed": False}],
                        "custody": "in-process"}],
            "standing_rules": [{"id": "r1", "description": "Keep the existing tests passing."}],
        },
    }


@pytest.mark.parametrize("reply", [
    APPROVE, {**APPROVE, "findings": [NOTE]}, changes(blocker()),
    changes(blocker("standing_rule", "r1"), findings=[NOTE]),
    changes(blocker(), blocker("standing_rule", "r1")),
])
def test_review_replies_through_protocol(tmp_path, review_request, reply):
    result = start_worker(tmp_path, review_request, [{"content": json.dumps(reply)}])
    assert result.exit_code == 0, result.stderr
    assert result.events == [
        {"event": "progress", "unit": "model_turn"},
        {"event": "reply", "role": "reviewer", "reply": reply},
    ]


@pytest.mark.parametrize("reply", [
    {**APPROVE, "head_sha": BASE},
    changes(blocker(identifier="unknown")),
    changes(blocker("standing_rule", "unknown")),
    changes(blocker("goal", "r1")),
    changes(blocker("standing_rule", "g1")),
    changes(), changes(findings=[NOTE]),
    {**APPROVE, "blocking_findings": [blocker()]},
    {**APPROVE, "head_sha": "main"},
    {key: value for key, value in APPROVE.items() if key != "head_sha"},
    changes({"problem": "No citation", "evidence": "greeting.py:2"}),
    changes({**blocker(), "evidence": ""}),
    {**APPROVE, "findings": [{"problem": "No evidence"}]},
    {**APPROVE, "extra": True},
])
def test_invalid_review_never_leaves_worker(tmp_path, review_request, reply):
    result = start_worker(tmp_path, review_request, [{"content": json.dumps(reply)}])
    assert result.exit_code == 1
    assert result.events == [{"event": "progress", "unit": "model_turn"}]
    assert result.stderr.strip() == "tl3_workers: run failed"


@pytest.mark.parametrize("field,value", [
    ("maker_reasoning", "Trust the maker."), ("goals", []), ("head_sha", "branch-name"),
    ("standing_rules", [{"id": "r1"}]),
    ("checks", [{"name": "invented", "verdict": "pass"}]),
])
def test_invalid_review_input_fails_before_model(tmp_path, review_request, field, value):
    review_request["inputs"][field] = value
    result = start_worker(tmp_path, review_request, [{"content": json.dumps(APPROVE)}])
    assert result.exit_code == 1
    assert result.events == []
    assert result.stderr.strip() == "tl3_workers: request failed"


def test_review_accepts_harness_failure_messages(tmp_path, review_request):
    review_request["inputs"]["checks"][0]["tests"][0]["message"] = "assert 'goodbye' == 'hello'"
    result = start_worker(tmp_path, review_request, [{"content": json.dumps(changes(blocker()))}])
    assert result.exit_code == 0, result.stderr
    assert result.events[-1]["reply"] == changes(blocker())


def test_review_accepts_empty_diff_checks_and_rules(tmp_path, review_request):
    review_request["inputs"].update(diff="", checks=[], standing_rules=[])
    result = start_worker(tmp_path, review_request, [{"content": json.dumps(APPROVE)}])
    assert result.exit_code == 0, result.stderr
    assert result.events[-1]["reply"] == APPROVE


@pytest.mark.parametrize("name,args", [
    ("list", {}), ("read", {"path": "greeting.py"}),
    ("glob", {"pattern": "*.py"}), ("grep", {"pattern": "goodbye"}),
    ("write", {"path": "greeting.py", "content": "forged"}),
    ("edit", {"path": "greeting.py", "old": "goodbye", "new": "hello"}),
    ("execute", {"command": "touch forbidden"}),
    ("read", {"path": "../outside.txt"}),
])
def test_review_tools_through_protocol(tmp_path, review_request, name, args):
    (tmp_path / "outside.txt").write_text("outside material")
    target = tmp_path / "head/greeting.py"
    before = target.read_text()
    reply = changes(blocker())
    result = start_worker(tmp_path, review_request, [
        {"content": "", "tool_calls": [{"name": name, "args": args, "id": "evidence", "type": "tool_call"}]},
        {"content": json.dumps(reply)},
    ])
    assert result.exit_code == 0, result.stderr
    assert result.events == [
        {"event": "progress", "unit": "model_turn"},
        {"event": "progress", "unit": "tool_result"},
        {"event": "progress", "unit": "model_turn"},
        {"event": "reply", "role": "reviewer", "reply": reply},
    ]
    assert target.read_text() == before
    assert not (tmp_path / "head/forbidden").exists()


def test_model_receives_review_material_and_confined_read_results(review_request, monkeypatch):
    observed = []
    role = ROLES["reviewer"]

    class InspectModel(FakeMessagesListChatModel):
        def bind_tools(self, tools, **kwargs):
            assert {tool.name: tool.description for tool in tools} == dict(role.tools)
            assert set(role.tools) == READ_TOOLS
            return self

        def _generate(self, messages, **kwargs):
            observed.append(copy.deepcopy(messages))
            return super()._generate(messages, **kwargs)

    calls = [
        {"name": name, "args": args, "id": name, "type": "tool_call"}
        for name, args in [
            ("list", {}), ("read", {"path": "greeting.py"}),
            ("glob", {"pattern": "*.py"}), ("grep", {"pattern": "goodbye"}),
            ("write", {"path": "greeting.py", "content": "forged"}),
        ]
    ]
    model = InspectModel(responses=[AIMessage(content="", tool_calls=calls),
                                    AIMessage(content=json.dumps(changes(blocker())))])

    def create_model(request):
        assert request["model"]["name"] == "controller-picked-reviewer"
        return model

    monkeypatch.setattr(runtime, "create_model", create_model)
    events = []
    runtime.run(review_request, events.append)
    human = [message for message in observed[0] if message.type == "human"]
    assert len(human) == 1
    assert json.loads(human[0].content) == review_request["inputs"]
    results = {message.name: message for message in observed[1] if message.type == "tool"}
    for name in READ_TOOLS:
        assert results[name].status == "success"
        assert "greeting.py" in results[name].content or "goodbye" in results[name].content
    assert results["write"].status == "error"
    assert events[-1]["reply"] == changes(blocker())


def test_prompt_blocks_test_report_tampering():
    from importlib.resources import files
    prompt = files("tl3_workers").joinpath("prompts", "reviewer.md").read_text()
    assert "breaks the standing\nrule that protects test reports" in prompt
    assert "is not\ntampering" in prompt


@pytest.fixture
def session_model(monkeypatch):
    observed = []
    responses = []

    class Model(FakeMessagesListChatModel):
        def bind_tools(self, tools, **kwargs):
            return self

        def _generate(self, messages, **kwargs):
            observed.append(copy.deepcopy(messages))
            return super()._generate(messages, **kwargs)

    monkeypatch.setattr(runtime, "create_model", lambda request: Model(responses=responses))
    return observed, responses


def start_review_session(request, session_model, tmp_path):
    observed, responses = session_model
    request["session_file"] = str(tmp_path / "sessions/35/plan-0/reviewer-attempt-1.json")
    request["resume_session"] = False
    responses[:] = [AIMessage(content=json.dumps(changes(blocker())),
                              additional_kwargs={"reasoning_content": "The implementation contradicts g1."})]
    runtime.run(request, lambda event: None)
    request["last_accepted_generation"] = request["operation"]["generation"]
    request["operation"]["generation"] += 2
    request["resume_session"] = True
    request["inputs"].update(base_sha=HEAD, head_sha="c" * 40, diff="fix greeting",
                              checks=[])
    observed.clear()
    responses[:] = [AIMessage(content=json.dumps({**APPROVE, "head_sha": "c" * 40}))]


def test_round_one_sees_round_zero_findings_and_reasoning(review_request, session_model, tmp_path):
    start_review_session(review_request, session_model, tmp_path)
    observed, _ = session_model
    runtime.run(review_request, lambda event: None)
    history = observed[0]
    humans = [json.loads(m.content) for m in history if m.type == "human"]
    assert len(humans) == 2
    assert humans[0]["head_sha"] == HEAD
    assert humans[1] == review_request["inputs"]
    previous = next(m for m in history if m.type == "ai")
    assert json.loads(previous.content) == changes(blocker())
    assert previous.additional_kwargs["reasoning_content"] == "The implementation contradicts g1."


def test_new_attempt_starts_fresh_reviewer_session(review_request, session_model, tmp_path):
    start_review_session(review_request, session_model, tmp_path)
    observed, _ = session_model
    review_request["session_file"] = str(tmp_path / "sessions/35/plan-0/reviewer-attempt-2.json")
    review_request["resume_session"] = False
    review_request.pop("last_accepted_generation")
    runtime.run(review_request, lambda event: None)
    assert len([m for m in observed[0] if m.type == "human"]) == 1
    assert not any(m.type == "ai" for m in observed[0])
    from pathlib import Path
    assert Path(review_request["session_file"]).is_file()


@pytest.mark.parametrize("crash_event", ["model_turn", "reply"])
@pytest.mark.parametrize("new_generation", [False, True])
def test_round_one_crash_and_retry_do_not_duplicate_turns(
        review_request, session_model, tmp_path, crash_event, new_generation):
    start_review_session(review_request, session_model, tmp_path)
    observed, _ = session_model

    def crash(event):
        if event.get("unit", event["event"]) == crash_event:
            raise RuntimeError("injected crash after durable completion")

    with pytest.raises(RuntimeError, match="injected crash"):
        runtime.run(review_request, crash)
    if new_generation:
        review_request["operation"]["generation"] += 1
    events = []
    runtime.run(review_request, events.append)
    assert len(observed) == 1, "retry must recover the completed model turn"
    assert events[-1]["reply"]["head_sha"] == "c" * 40
    from pathlib import Path
    checkpoint = Path(review_request["session_file"]).read_bytes()
    runtime.run(review_request, events.append)
    assert len(observed) == 1
    assert Path(review_request["session_file"]).read_bytes() == checkpoint


@pytest.mark.parametrize("crash_unit", ["model_turn", "tool_result"])
def test_round_one_retry_keeps_completed_tool_exchange(review_request, session_model, tmp_path, crash_unit):
    start_review_session(review_request, session_model, tmp_path)
    observed, responses = session_model
    responses.insert(0, AIMessage(content="", tool_calls=[{
        "name": "read", "args": {"path": "greeting.py"}, "id": "inspect", "type": "tool_call"}]))

    def crash(event):
        if event.get("unit") == crash_unit:
            raise RuntimeError("injected crash during tool exchange")

    with pytest.raises(RuntimeError, match="during tool exchange"):
        runtime.run(review_request, crash)
    responses.pop(0)
    review_request["operation"]["generation"] += 1
    runtime.run(review_request, lambda event: None)
    assert len(observed) == 2
    assert len([m for m in observed[-1] if m.type == "human"]) == 2
    assert len([m for m in observed[-1] if m.type == "tool"]) == 1
    assert len([m for m in observed[-1] if m.type == "ai" and m.tool_calls]) == 1


def test_accepted_round_one_recheck_is_a_new_visit(review_request, session_model, tmp_path):
    start_review_session(review_request, session_model, tmp_path)
    observed, _ = session_model
    runtime.run(review_request, lambda event: None)
    review_request["last_accepted_generation"] = review_request["operation"]["generation"]
    review_request["operation"]["generation"] += 2
    runtime.run(review_request, lambda event: None)
    assert len(observed) == 2
    assert len([m for m in observed[-1] if m.type == "human"]) == 3


@pytest.mark.parametrize("with_session", [False, True])
@pytest.mark.parametrize("usage", [None, {"input_tokens": 127_499, "output_tokens": 1,
                                        "total_tokens": 127_500}])
def test_reviewer_stops_before_context_exhaustion(review_request, session_model, tmp_path,
                                                 with_session, usage):
    observed, responses = session_model
    review_request["inputs"]["context_size"] = 150_000 if usage else 1_000
    if with_session:
        review_request["session_file"] = str(tmp_path / "reviewer.json")
    responses[:] = [AIMessage(content=json.dumps(APPROVE), usage_metadata=usage)]
    events = []
    runtime.run(review_request, events.append)
    assert events == [{"event": "reply", "role": "reviewer", "reply": {"verdict": "out_of_room"}}]
    if with_session:
        runtime.run(review_request, events.append)
        assert events[-1] == events[0]
        assert len(observed) == 1, "retry must not repeat an exhausted model turn"


@pytest.mark.parametrize("field,value", [("diff", "different fix"), ("checks", [])])
def test_review_retry_rejects_changed_evidence(review_request, session_model, tmp_path, field, value):
    observed, responses = session_model
    review_request["session_file"] = str(tmp_path / "reviewer.json")
    responses[:] = [AIMessage(content=json.dumps(APPROVE))]
    runtime.run(review_request, lambda event: None)
    review_request["inputs"][field] = value
    with pytest.raises(ValueError, match="review retry evidence changed before acceptance"):
        runtime.run(review_request, lambda event: None)
    assert len(observed) == 1


def test_exhausted_reviewer_does_not_execute_pending_reads(review_request, session_model, tmp_path):
    observed, responses = session_model
    review_request["session_file"] = str(tmp_path / "reviewer.json")
    review_request["inputs"]["context_size"] = 1_000
    responses[:] = [AIMessage(content="", tool_calls=[{
        "name": "read", "args": {"path": "greeting.py"}, "id": "inspect", "type": "tool_call"}])]
    events = []
    for _ in range(2):
        runtime.run(review_request, events.append)
    assert events == [{"event": "reply", "role": "reviewer", "reply": {"verdict": "out_of_room"}}] * 2
    assert len(observed) == 1


@pytest.mark.parametrize("length,valid", [(1000, True), (1001, False)])
def test_review_check_message_bound(review_request, length, valid):
    from jsonschema import Draft202012Validator
    from tl3_workers.contracts import schema

    review_request["inputs"]["checks"][0]["tests"][0]["message"] = "é" * length
    assert Draft202012Validator(schema("reviewer-input")).is_valid(review_request["inputs"]) == valid


@pytest.mark.parametrize('count,valid', [(20, True), (21, False)])
def test_blocking_findings_are_bounded(count, valid):
    from jsonschema import Draft202012Validator
    from tl3_workers.contracts import schema
    assert Draft202012Validator(schema('reviewer-reply')).is_valid(changes(*[blocker()] * count)) == valid


@pytest.mark.parametrize('recovered', [False, True])
def test_reviewer_reply_is_validated_exactly_once(
        review_request, session_model, tmp_path, monkeypatch, recovered):
    from dataclasses import replace
    from tl3_workers.roles import reviewer

    _, responses = session_model
    responses[:] = [AIMessage(content=json.dumps(APPROVE))]
    review_request['session_file'] = str(tmp_path / 'reviewer.json')
    if recovered:
        runtime.run(review_request, lambda event: None)
        review_request['operation']['generation'] += 1
    schema_checks, evidence_checks = [], []
    validate = runtime.validate
    check_reply = reviewer.check_reply

    def checked_schema(name, reply):
        schema_checks.append(name)
        validate(name, reply)

    def checked_evidence(inputs, reply):
        evidence_checks.append(copy.deepcopy(reply))
        check_reply(inputs, reply)

    monkeypatch.setattr(runtime, 'validate', checked_schema)
    monkeypatch.setattr(reviewer, 'validate', checked_schema)
    monkeypatch.setattr(reviewer, 'check_reply', checked_evidence)
    monkeypatch.setitem(ROLES, 'reviewer', replace(ROLES['reviewer'], check_reply=checked_evidence))
    events = []
    runtime.run(review_request, events.append)
    assert events[-1]['reply'] == APPROVE
    assert schema_checks == ['reviewer-reply']
    assert evidence_checks == [APPROVE]


def test_q44_reviewer_reports_exhaustion_through_process_protocol(tmp_path, review_request):
    review_request["inputs"]["context_size"] = 1_000
    result = start_worker(tmp_path, review_request, [{"content": json.dumps(APPROVE)}])
    assert result.exit_code == 0, result.stderr
    assert result.events == [{"event": "reply", "role": "reviewer", "reply": {"verdict": "out_of_room"}}]


@pytest.mark.parametrize("with_session", [False, True])
def test_q44_model_cannot_request_a_free_restart(review_request, session_model, tmp_path, with_session):
    _, responses = session_model
    responses[:] = [AIMessage(content=json.dumps({"verdict": "out_of_room"}))]
    if with_session:
        review_request["session_file"] = str(tmp_path / "reviewer.json")
    events = []
    with pytest.raises(ValueError, match="model cannot report reviewer exhaustion"):
        runtime.run(review_request, events.append)
    assert not any(event["event"] == "reply" for event in events)
