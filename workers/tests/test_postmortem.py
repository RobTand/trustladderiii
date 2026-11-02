"""#403: the postmortem role's judges, with a fake model and fake MLflow."""

import base64
import copy
import json
import secrets

from jsonschema import ValidationError
import pytest

from tl3_workers import judges, tracing
from tl3_workers.contracts import validate
from tl3_workers.roles import ROLES
from tl3_workers.roles import postmortem


KEY = "tl3-planted-model-key-7c1e09"
SECRET = "password=assigned-password-403"
TOKEN = "sk-test403abcdefghijklmnopqrst"
PLANTED = (KEY, "assigned-password-403", TOKEN)
MARKER = "SPANMARKER"

RUBRIC = {"version": 1, "dimensions": {
    "reliability": {"grader": "code", "fact": "fix rounds"},
    "correctness": {"grader": "judge", "grades": {"5": "Right.", "1": "Wrong."}},
    "fix round": {"grader": "judge", "grades": {"5": "Fixed all.", "1": "Fixed nothing."}},
}}

MAKER_PROMPT = "workers/src/tl3_workers/prompts/maker.md"
RECOMMENDATION = {"phase": "maker", "kind": "prompt", "target": MAKER_PROMPT,
                  "operation": "replace", "before": "Run the tests.",
                  "after": "Run the tests through PrismaBuild.",
                  "evidence": "tr-1 shows a local test run.", "effect": "No local runs."}


def written(recommendations=1, extra=""):
    return {
        "summary": "The maker needed a fix round. " + extra,
        "cause": "tl3",
        "went_wrong": "The maker skipped a test.",
        "timeline": [{"at": "Making", "event": "The maker ran."}],
        "process_failures": [{"phase": "maker", "kind": "prompt or skill defect",
                              "what": "It skipped a test.", "evidence": "tr-1"}],
        "recommendations": [dict(RECOMMENDATION, after=f"Run test {n}.")
                            for n in range(recommendations)],
        "notes": "",
        "lessons": ["Sandbox tests need PrismaBuild. " + extra],
    }


def reply(recommendations=1):
    return {"postmortem": written(recommendations),
            "grades": [{"dimension": "correctness", "grade": 4, "rationale": "tr-1",
                        "judge_model": "claude-sonnet-5-5", "assessment_id": "a-1"}],
            "assessment_ids": ["a-1"],
            "usage": {"traces": 1, "input_tokens": 100, "output_tokens": 10,
                      "cache_read_input_tokens": 60, "cache_creation_input_tokens": 0},
            "models": {"postmortem": "claude-opus-5-5", "grades": "claude-sonnet-5-5"}}


def test_the_schema_and_code_each_reject_a_fourth_recommendation():
    validate("postmortem-reply", reply(3))
    with pytest.raises(ValidationError):
        validate("postmortem-reply", reply(4))
    with pytest.raises(ValueError, match="at most three"):
        ROLES["postmortem"].check_reply({}, reply(4))
    add = reply(1)
    add["postmortem"]["recommendations"][0].update(operation="add")
    with pytest.raises(ValidationError):
        validate("postmortem-reply", add)


def test_a_tool_result_is_redacted_and_capped():
    redactor = tracing.Redactor(None)
    redactor.secrets.add(KEY)
    text = (KEY + " " + "x" * 100) * 2000
    shown = judges.bound(text, redactor)
    assert len(text) > judges.TOOL_RESULT_CAP
    assert len(shown) <= judges.TOOL_RESULT_CAP + 200
    assert KEY not in shown
    # A secret the cap cuts in two is still redacted.
    cut = "y" * (judges.TOOL_RESULT_CAP - 5) + KEY
    assert KEY[:5] not in judges.bound(cut, redactor)[judges.TOOL_RESULT_CAP - 10:]


def test_guarded_tools_wrap_every_tool_result(monkeypatch):
    from mlflow.genai.judges.adapters import gateway_adapter
    from mlflow.types.llm import ChatMessage

    def huge(tool_calls, trace):
        return [ChatMessage(role="tool", content=TOKEN + "z" * 300_000, tool_call_id="1",
                            name="get_root_span")]

    monkeypatch.setattr(gateway_adapter, "_process_tool_calls", huge)
    redactor = tracing.Redactor(None)
    turns = []
    with judges.guarded_tools(redactor, lambda: turns.append(1)):
        [message] = gateway_adapter._process_tool_calls(tool_calls=[], trace=None)
    assert len(message.content) <= judges.TOOL_RESULT_CAP + 200
    assert TOKEN not in message.content
    assert turns == [1]
    assert gateway_adapter._process_tool_calls is huge, "the wrapper is removed afterwards"


def span(trace_id, name, inputs, outputs, start):
    from mlflow.entities.span import Span

    return Span.from_dict({
        "trace_id": base64.b64encode(trace_id).decode(),
        "span_id": base64.b64encode(secrets.token_bytes(8)).decode(),
        "parent_span_id": None, "name": name,
        "start_time_unix_nano": start, "end_time_unix_nano": start + 1_000_000,
        "events": [], "links": [], "status": {"code": "STATUS_CODE_OK", "message": ""},
        "attributes": {"mlflow.traceRequestId": json.dumps("tr-" + trace_id.hex()),
                       "mlflow.spanType": json.dumps("AGENT"),
                       "mlflow.spanInputs": json.dumps(inputs),
                       "mlflow.spanOutputs": json.dumps(outputs)},
    })


def source_trace(role, start, tags):
    from mlflow.entities import Trace, TraceData, TraceInfo, TraceLocation, TraceState

    raw = secrets.token_bytes(16)
    content = f"{MARKER} the maker read {KEY} and {SECRET} and {TOKEN} " * 400
    return Trace(info=TraceInfo(trace_id="tr-" + raw.hex(),
                                trace_location=TraceLocation.from_experiment_id("2"),
                                request_time=start // 1_000_000, state=TraceState.OK,
                                execution_duration=1, tags={"role": role, **tags}),
                 data=TraceData(spans=[span(raw, f"tl3_{role}", {"brief": content}, content,
                                            start)]))


class FakeTraces:
    def __init__(self, traces):
        self.traces, self.asked, self.logged = traces, [], []

    def find(self, tags, limit):
        self.asked.append(tags)
        return self.traces[:limit]

    def log(self, trace_id, name, value, rationale, judge_model, metadata):
        self.logged.append({"trace_id": trace_id, "name": name, "value": value,
                            "rationale": rationale, "judge_model": judge_model,
                            "metadata": metadata})
        return f"a-{len(self.logged)}"


class FakeModel:
    """CLIProxyAPI as MLflow's gateway adapter sees it. It records every request."""

    def __init__(self, postmortem=None, needs_rob="no"):
        self.payloads = []
        self.postmortem = postmortem or written(2, extra=f"It read {TOKEN}.")
        self.needs_rob = needs_rob

    def __call__(self, endpoint, headers, payload, num_retries):
        self.payloads.append(copy.deepcopy(payload))
        messages = payload["messages"]
        if not any(message.get("role") == "tool" for message in messages):
            call = {"id": f"call-{len(self.payloads)}", "type": "function",
                    "function": {"name": "search_trace_regex", "arguments": json.dumps(
                        {"pattern": MARKER, "max_matches": 400,
                         "surrounding_content_length": 1000})}}
            return {"id": "r", "object": "chat.completion", "model": "m", "created": 0,
                    "choices": [{"index": 0, "finish_reason": "tool_calls",
                                 "message": {"role": "assistant", "content": None,
                                             "tool_calls": [call]}}],
                    "usage": {"prompt_tokens": 10, "completion_tokens": 1, "total_tokens": 11}}
        prompt = json.dumps(messages)
        if "write the postmortem" in prompt:
            result = json.dumps(self.postmortem)
        elif "needed Rob" in prompt:
            result = self.needs_rob
        else:
            result = 4
        return {"id": "r", "object": "chat.completion", "model": "m", "created": 0,
                "choices": [{"index": 0, "finish_reason": "stop", "message": {
                    "role": "assistant",
                    "content": json.dumps({"rationale": f"tr-1 shows it; {TOKEN}",
                                           "result": result})}}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 1, "total_tokens": 11}}

    def text(self):
        return json.dumps(self.payloads)


@pytest.fixture(autouse=True)
def judge_environment(monkeypatch):
    # judges.configure sets these for its process; the test restores them.
    for name, value in [("OPENAI_API_KEY", "placeholder"), ("MLFLOW_DISABLE_TELEMETRY", "true"),
                        ("DO_NOT_TRACK", "true"), ("MLFLOW_JUDGE_MAX_ITERATIONS", "30")]:
        monkeypatch.setenv(name, value)


@pytest.fixture(autouse=True)
def release(tmp_path, monkeypatch):
    """The files of the release the postmortem judges, as its recommendations target."""
    root = tmp_path / "release"
    prompt = root / MAKER_PROMPT
    prompt.parent.mkdir(parents=True)
    prompt.write_text("You are the maker.\nRun the tests.\nReply when done.\n")
    monkeypatch.setattr(postmortem, "RELEASE_ROOT", root)
    return root


@pytest.fixture
def model(monkeypatch):
    from mlflow.genai.judges.adapters import gateway_adapter

    fake = FakeModel()
    monkeypatch.setattr(gateway_adapter, "send_chat_request", fake)
    return fake


def request(tmp_path, *, release="b" * 40):
    key_file = tmp_path / "model-key"
    key_file.write_text(KEY + "\n")
    worktree = tmp_path / "worktree"
    worktree.mkdir(exist_ok=True)
    case = {
        "request": {"repo": "RobTand/tl3-sandbox", "issue": 42, "title": "Fix the build",
                    "text": f"Fix it. The deploy key is {KEY}; {SECRET}; token {TOKEN}."},
        "outcome": "Merged", "grades": [{"dimension": "reliability", "grade": 4}],
        "phases": [{"phase": "Making", "ms": 90}],
        "questions": [{"kind": "cant_meet", "findings": {"decision": f"See {TOKEN}"}}],
        "incidents": [], "plan": {"pieces": []}, "brief": {"goals": [SECRET]},
        "audit": {"verdict": "agree"}, "release": release,
    }
    return {
        "protocol": 1, "operation": {"node": 3, "generation": 7, "kind": "postmortem"},
        "role": "postmortem",
        "model": {"base_url": "http://proxy.invalid:8317/v1", "name": "claude-opus-5-5",
                  "effort": "high", "budget_tokens": 32000},
        "key_file": str(key_file), "worktree": str(worktree),
        "inputs": {"record": {"id": 9, "trigger": "close", "reason": "needed_fix"},
                   "case": case, "rubric": RUBRIC, "release": release,
                   "grade_model": "claude-sonnet-5-5", "deadline_secs": 1200,
                   "trace_tags": {"node": "3", "request": "3", "repo": "RobTand/tl3-sandbox",
                                  "issue": "42"}},
    }


def run(document):
    from tl3_workers.runtime import run as run_worker

    events = []
    run_worker(document, events.append)
    [reply] = [event["reply"] for event in events if event["event"] == "reply"]
    return reply, events


def test_judges_see_only_redacted_capped_material_and_grades_are_logged(tmp_path, model,
                                                                         monkeypatch):
    tags = {"input_tokens": "1000", "output_tokens": "50", "cache_read_input_tokens": "600",
            "cache_creation_input_tokens": "100", "model": "claude-sonnet-5-5",
            "outcome": "succeeded"}
    traces = FakeTraces([source_trace("reviewer", 2_000_000_000, tags),
                         source_trace("maker", 1_000_000_000, tags)])
    monkeypatch.setattr(postmortem, "SOURCE", lambda settings: traces)
    document = request(tmp_path)
    reply, events = run(document)

    assert traces.asked == [document["inputs"]["trace_tags"]]
    assert model.payloads, "the judges called the model"
    seen = model.text()
    assert MARKER in seen, "the judges read the traces through their tools"
    for planted in PLANTED:
        assert planted not in seen, f"{planted} reached a judge"
    for payload in model.payloads:
        for message in payload["messages"]:
            if message.get("role") == "tool":
                assert len(message["content"]) <= judges.TOOL_RESULT_CAP + 200
    assert any(len(message["content"]) > judges.TOOL_RESULT_CAP
               for payload in model.payloads for message in payload["messages"]
               if message.get("role") == "tool"), "the cap was exercised"
    assert any(event["event"] == "progress" for event in events)

    stored = json.dumps(reply)
    for planted in PLANTED:
        assert planted not in stored, f"{planted} reached the reply"
    validate("postmortem-reply", reply)
    assert reply["postmortem"]["cause"] == "tl3"
    assert len(reply["postmortem"]["recommendations"]) == 2
    assert {grade["dimension"]: grade["grade"] for grade in reply["grades"]} == {
        "correctness": 4, "fix round": 4}
    assert reply["models"] == {"postmortem": "claude-opus-5-5", "grades": "claude-sonnet-5-5"}
    assert reply["usage"] == {"traces": 2, "input_tokens": 2000, "output_tokens": 100,
                              "cache_read_input_tokens": 1200,
                              "cache_creation_input_tokens": 200}

    newest = traces.traces[0].info.trace_id
    assert [entry["trace_id"] for entry in traces.logged] == [newest, newest]
    assert sorted(reply["assessment_ids"]) == ["a-1", "a-2"]
    for entry in traces.logged:
        assert entry["metadata"] == {
            "node": "3", "request": "3", "repo": "RobTand/tl3-sandbox", "issue": "42",
            "release": "b" * 40, "rubric_version": "1", "dimension": entry["name"],
            "judge_model": "claude-sonnet-5-5"}
        assert TOKEN not in entry["rationale"]

    # A run restarted under the same run key reuses its reply: no judge runs
    # again and no assessment is logged twice.
    calls, logged = len(model.payloads), len(traces.logged)
    again, _ = run(request(tmp_path))
    assert again == reply
    assert (len(model.payloads), len(traces.logged)) == (calls, logged)


def test_a_restart_after_judging_logs_only_the_missing_assessments(tmp_path, model,
                                                                   monkeypatch):
    traces = FakeTraces([source_trace("maker", 1_000_000_000, {})])
    monkeypatch.setattr(postmortem, "SOURCE", lambda settings: traces)
    first = traces.log
    calls = []

    def crash_after_one(*args):
        calls.append(args)
        if len(calls) == 2:
            raise OSError("the worker died")
        return first(*args)

    traces.log = crash_after_one
    with pytest.raises(OSError):
        run(request(tmp_path))
    judged = len(model.payloads)
    traces.log = first
    reply, _ = run(request(tmp_path))
    assert len(model.payloads) == judged, "judging is not repeated"
    assert [entry["name"] for entry in traces.logged] == ["correctness", "fix round"]
    assert len(reply["assessment_ids"]) == 2


def test_a_failed_judge_fails_the_run(tmp_path, monkeypatch):
    from mlflow.genai.judges.adapters import gateway_adapter

    def broken(endpoint, headers, payload, num_retries):
        raise RuntimeError(f"provider said {KEY}")

    monkeypatch.setattr(gateway_adapter, "send_chat_request", broken)
    monkeypatch.setattr(postmortem, "SOURCE", lambda settings: FakeTraces([]))
    with pytest.raises(ValueError, match="judge failed") as error:
        run(request(tmp_path))
    assert KEY not in str(error.value)


def test_the_escalation_seam_returns_feedback_and_logs_nothing(model, monkeypatch):
    judges.configure("http://proxy.invalid:8317/v1", KEY)
    trace = postmortem.case_trace([], tracing.Redactor(None), 9)
    sonnet, opus = judges.needs_rob_judge("claude-sonnet-5-5"), judges.needs_rob_judge(
        "claude-opus-5-5")
    feedback = sonnet(inputs={"case": "Q8"}, trace=trace)
    assert feedback.value == "no" and feedback.error is None
    agreed = judges.unanimous(sonnet, opus, name="needs_rob")(inputs={"case": "Q8"}, trace=trace)
    assert agreed.value == "no"
    assert not judges.failed(agreed)

    from mlflow.entities.assessment import Feedback
    split = judges.unanimity([Feedback(value="yes"), Feedback(value="no")])
    assert split.value == "no consensus"
    errored = Feedback(value="yes", error=RuntimeError("boom"))
    assert judges.unanimity([Feedback(value="yes"), errored]).value == "no consensus"
    assert judges.failed(errored)


def test_judges_take_at_most_twelve_turns():
    from mlflow.environment_variables import MLFLOW_JUDGE_MAX_ITERATIONS

    judges.configure("http://proxy.invalid:8317/v1", KEY)
    assert MLFLOW_JUDGE_MAX_ITERATIONS.get() == 12


def test_a_run_past_its_deadline_fails_with_the_deadline_class(tmp_path, monkeypatch):
    import time

    from mlflow.genai.judges.adapters import gateway_adapter

    def slow(endpoint, headers, payload, num_retries):
        time.sleep(5)
        raise RuntimeError("the proxy never answered")

    monkeypatch.setattr(gateway_adapter, "send_chat_request", slow)
    monkeypatch.setattr(postmortem, "SOURCE", lambda settings: FakeTraces([]))
    document = request(tmp_path)
    document["inputs"]["deadline_secs"] = 1
    started = time.monotonic()
    with pytest.raises(postmortem.PostmortemDeadline) as error:
        run(document)
    assert time.monotonic() - started < 4
    assert tracing.failure_tags(error.value)["failure_class"] == "PostmortemDeadline"
    # The alarm is gone with the run.
    import signal
    assert signal.getitimer(signal.ITIMER_REAL) == (0.0, 0.0)


def test_a_recommendation_must_edit_text_its_target_holds_exactly_once(tmp_path, model,
                                                                       monkeypatch, capsys,
                                                                       release):
    planner = release / "workers/src/tl3_workers/prompts/planner.md"
    planner.write_text("Plan the work. Plan the work.\n")
    model.postmortem = written(0)
    model.postmortem["recommendations"] = [
        dict(RECOMMENDATION, target="workers/src/tl3_workers/prompts/planner.md",
             before="Plan the work."),
        dict(RECOMMENDATION, target="crates/controller/src/lib.rs"),
        RECOMMENDATION,
    ]
    monkeypatch.setattr(postmortem, "SOURCE", lambda settings: FakeTraces([]))
    reply, _ = run(request(tmp_path))
    assert reply["postmortem"]["recommendations"] == [RECOMMENDATION]
    logged = capsys.readouterr().err
    assert "workers/src/tl3_workers/prompts/planner.md: before appears 2 times" in logged
    assert "crates/controller/src/lib.rs: not a file in the release" in logged


@pytest.mark.parametrize("item, reason", [
    (dict(RECOMMENDATION), None),
    (dict(RECOMMENDATION, before="Not in the prompt."), "before appears 0 times"),
    (dict(RECOMMENDATION, target="../outside.md"), "not a file in the release"),
    (dict(RECOMMENDATION, target="/etc/hostname"), "not a file in the release"),
    (dict(RECOMMENDATION, operation="add", before="", after="Check twice."), None),
    (dict(RECOMMENDATION, target="workers/missing.md", operation="add", before="",
          after="New."), "not a file in the release"),
])
def test_unusable_names_why_a_recommendation_cannot_apply(release, item, reason):
    (release.parent / "outside.md").write_text("Run the tests.\n")
    assert postmortem.unusable(item, release) == reason
