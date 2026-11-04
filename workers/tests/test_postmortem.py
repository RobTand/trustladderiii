"""#403: the postmortem role's judges, with a fake model and fake MLflow."""

import base64
import copy
import hashlib
import json
from pathlib import Path
import secrets

from jsonschema import ValidationError
import pytest

from tl3_workers import judges, tracing
from tl3_workers.contracts import validate
from tl3_workers.roles import postmortem
from postmortem_support import FakeTraces, KEY, MAKER_PROMPT, SECRET, TOKEN, request, run
from postmortem_support import judge_environment as judge_environment
from postmortem_support import release as release


PLANTED = (KEY, "assigned-password-403", TOKEN)
MARKER = "SPANMARKER"


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


class FakeModel:
    """CLIProxyAPI as MLflow's gateway adapter sees it. It records every request."""

    def __init__(self, postmortem=None, needs_rob="no", tool_turns=1):
        self.payloads = []
        self.tool_turns = tool_turns
        self.postmortem = postmortem or written(2, extra=f"It read {TOKEN}.")
        self.needs_rob = needs_rob

    def __call__(self, endpoint, headers, payload, num_retries):
        self.payloads.append(copy.deepcopy(payload))
        messages = payload["messages"]
        if sum(message.get("role") == "tool" for message in messages) < self.tool_turns:
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


@pytest.fixture
def model(monkeypatch):
    from mlflow.genai.judges.adapters import gateway_adapter

    fake = FakeModel()
    monkeypatch.setattr(gateway_adapter, "send_chat_request", fake)
    return fake


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
        metadata = dict(entry["metadata"])
        assert metadata.pop("idempotency_key").startswith("a-")
        assert metadata == {
            "node": "3", "request": "3", "repo": "RobTand/tl3-sandbox", "issue": "42",
            "release": "b" * 40, "rubric_version": "1", "dimension": entry["name"],
            "judge_model": "claude-sonnet-5-5",
            "postmortem_run": hashlib.sha256(document["worktree"].encode()).hexdigest()}
        assert TOKEN not in entry["rationale"]

    # A run restarted under the same run key reuses its reply: no judge runs
    # again and no assessment is logged twice.
    calls, logged = len(model.payloads), len(traces.logged)
    again, _ = run(request(tmp_path))
    assert again == reply
    assert (len(model.payloads), len(traces.logged)) == (calls, logged)


@pytest.mark.durability
def test_a_restart_after_judging_logs_only_the_missing_assessments(tmp_path, model,
                                                                   monkeypatch):
    traces = FakeTraces([source_trace("maker", 1_000_000_000, {})])
    monkeypatch.setattr(postmortem, "SOURCE", lambda settings: traces)
    first = traces.log
    calls = []

    def crash_after_one(*args):
        calls.append(args)
        result = first(*args)
        if len(calls) == 1:
            raise KeyboardInterrupt("the worker died after the external commit")
        return result

    traces.log = crash_after_one
    with pytest.raises(KeyboardInterrupt):
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
    with pytest.raises(ValueError, match="grade judge failure") as error:
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


def test_judges_have_forty_eight_turns_with_one_reserved_for_repair():
    from mlflow.environment_variables import MLFLOW_JUDGE_MAX_ITERATIONS

    judges.configure("http://proxy.invalid:8317/v1", KEY)
    assert MLFLOW_JUDGE_MAX_ITERATIONS.get() == 48
    with judges.postmortem_turns():
        assert MLFLOW_JUDGE_MAX_ITERATIONS.get() == 47
    assert MLFLOW_JUDGE_MAX_ITERATIONS.get() == 48


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
                                                                       release):  # noqa: F811
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
    assert reply["postmortem"]["notes"] == (
        "Dropped recommendation 1, workers/src/tl3_workers/prompts/planner.md: "
        "before appears 2 times. Dropped recommendation 2, "
        "crates/controller/src/lib.rs: not a file in the release.")
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
def test_unusable_names_why_a_recommendation_cannot_apply(release, item, reason):  # noqa: F811
    (release.parent / "outside.md").write_text("Run the tests.\n")
    assert postmortem.unusable(item, release) == reason


def test_drop_reasons_follow_the_notes_within_their_cap(tmp_path, model, monkeypatch):
    model.postmortem = written(0)
    model.postmortem["notes"] = "The judge's note. " * 110
    model.postmortem["recommendations"] = [
        dict(RECOMMENDATION, before="Not in the prompt.")]
    monkeypatch.setattr(postmortem, "SOURCE", lambda settings: FakeTraces([]))
    reply, _ = run(request(tmp_path))
    notes = reply["postmortem"]["notes"]
    assert len(notes) == 2000
    assert notes.startswith("The judge's note. ")
    assert notes.endswith(f"Dropped recommendation 1, {MAKER_PROMPT}: before appears 0 times.")
    validate("postmortem-reply", reply)


@pytest.fixture
def contract_boundary(tmp_path, monkeypatch):
    """Failure-shaped judge outputs, without reading production trace content."""
    from mlflow.entities.assessment import Feedback

    traces = FakeTraces([source_trace("maker", 1_000_000_000, {})])
    monkeypatch.setattr(postmortem, "SOURCE", lambda settings: traces)
    monkeypatch.setattr(judges, "grade_judges", lambda *args: {
        "correctness": lambda **kwargs: Feedback(value=4, rationale="fixture")})
    values, calls = [], []

    def judge(prompt, model):
        def invoke(**kwargs):
            calls.append(prompt)
            value = values[min(len(calls) - 1, len(values) - 1)]
            return value if isinstance(value, Feedback) else Feedback(value=value)
        return invoke

    monkeypatch.setattr(judges, "postmortem_judge", judge)
    from langchain_core.messages import AIMessage
    from tl3_workers.runtime import ProxyChatOpenAI

    def repair(self, history):
        calls.append(history[-1].text)
        value = values[min(len(calls) - 1, len(values) - 1)]
        return AIMessage(content=json.dumps(value) if isinstance(value, dict) else value)

    monkeypatch.setattr(ProxyChatOpenAI, "invoke", repair)
    return request(tmp_path), traces, values, calls


@pytest.mark.parametrize("bad, diagnostic", [
    (dict(written(0), went_wrong=["The maker skipped a test."]), "schema validation failed: type"),
    ("{broken JSON", "JSON parse failure"),
    (dict(written(0), recommendations=[dict(RECOMMENDATION, target="bad\x00path")]),
     "recommendation contract failure"),
])
def test_invalid_output_never_becomes_successful_work(contract_boundary, bad, diagnostic):
    document, traces, values, calls = contract_boundary
    values.append(bad)
    with pytest.raises((ValueError, ValidationError)) as error:
        run(document)
    workdir = Path(document["worktree"])
    assert not (workdir / "judged.json").exists()
    assert not traces.logged
    message = tracing.failure_tags(error.value)["failure_message"]
    assert diagnostic in message
    assert not any(planted in message for planted in PLANTED)
    assert len(calls) == 2, "one bounded correction, shared with ordinary roles"
    assert not (workdir / "reply.json").exists()
    assert not (workdir / "assessments.json").exists()


@pytest.mark.durability
def test_wrong_type_is_repaired_before_checkpoint_and_assessment(contract_boundary):
    document, traces, values, calls = contract_boundary
    values.extend([dict(written(0), went_wrong=["The maker skipped a test."]), written(0)])
    result, _ = run(document)
    validate("postmortem-reply", result)
    assert result["postmortem"]["went_wrong"] == "The maker skipped a test."
    assert len(calls) == 2
    assert len(traces.logged) == 1
    assert run(document)[0] == result
    assert len(traces.logged) == 1


def test_feedback_failure_names_stage_without_repair_or_provider_text(contract_boundary):
    from mlflow.entities.assessment import Feedback

    document, traces, values, calls = contract_boundary
    values.append(Feedback(value=None, error=RuntimeError(f"provider {KEY}")))
    with pytest.raises(ValueError) as error:
        run(document)
    tags = tracing.failure_tags(error.value)
    assert tags["failure_message"] == "postmortem judge failure: other"
    assert KEY not in json.dumps(tags)
    assert len(calls) == 1
    assert not traces.logged


@pytest.mark.parametrize("artifact", ["reply.json", "judged.json"])
@pytest.mark.parametrize("malformed", [False, True])
@pytest.mark.durability
def test_invalid_cached_work_is_rejected_before_judging_or_logging(
        contract_boundary, artifact, malformed):
    document, traces, values, calls = contract_boundary
    invalid = reply(0)
    invalid["postmortem"]["went_wrong"] = ["Failure-shaped fixture."]
    (Path(document["worktree"]) / artifact).write_text(
        "{invalid" if malformed else json.dumps(invalid))
    with pytest.raises(ValueError) as error:
        run(document)
    assert "cached " + ("reply" if artifact == "reply.json" else "judged") in (
        tracing.failure_tags(error.value)["failure_message"])
    assert not calls
    assert not traces.logged


@pytest.mark.parametrize("artifact", ["judged.json", "assessments.json", "reply.json"])
@pytest.mark.durability
def test_restart_after_each_valid_checkpoint_never_repeats_assessments(
        contract_boundary, monkeypatch, artifact):
    document, traces, values, calls = contract_boundary
    values.append(written(0))
    save = postmortem._save
    interrupted = []

    def crash(path, value):
        save(path, value)
        if path.name == artifact and not interrupted:
            interrupted.append(True)
            raise KeyboardInterrupt("checkpoint committed")

    monkeypatch.setattr(postmortem, "_save", crash)
    with pytest.raises(KeyboardInterrupt):
        run(document)
    result, _ = run(document)
    validate("postmortem-reply", result)
    assert len(calls) == 1
    assert len(traces.logged) == 1
    assert result["assessment_ids"] == ["a-1"]


@pytest.mark.durability
def test_ambiguous_assessment_response_is_reconciled_without_resubmit(contract_boundary):
    document, traces, values, calls = contract_boundary
    values.append(written(0))
    submitted = []
    log = traces.log

    def uncertain(*args):
        submitted.append(args)
        log(*args)
        raise OSError(f"transport {KEY}")

    traces.log = uncertain
    with pytest.raises(ValueError, match="assessment write failure"):
        run(document)
    result, _ = run(document)
    assert len(submitted) == 1
    assert len(calls) == 1
    assert len(traces.logged) == 1
    assert KEY not in json.dumps(result)
    assert result["assessment_ids"] == ["a-1"]
    assert json.loads((Path(document["worktree"]) / "assessment-pending.json").read_text()) == {}


def test_grade_feedback_error_is_terminal_and_names_the_stage(contract_boundary, monkeypatch):
    from mlflow.entities.assessment import Feedback

    document, traces, values, calls = contract_boundary
    monkeypatch.setattr(judges, "grade_judges", lambda *args: {
        "correctness": lambda **kwargs: Feedback(error=RuntimeError(KEY))})
    with pytest.raises(ValueError) as error:
        run(document)
    assert tracing.failure_tags(error.value)["failure_message"] == "grade judge failure: other"
    assert not calls
    assert not traces.logged


def test_judge_receives_authoritative_nested_contract(model, monkeypatch, tmp_path):
    from tl3_workers.contracts import schema

    monkeypatch.setattr(postmortem, "SOURCE", lambda settings: FakeTraces([]))
    result, _ = run(request(tmp_path))
    validate("postmortem-reply", result)
    instructions = model.text()
    assert "Authoritative result JSON Schema" in instructions
    contract = judges.postmortem_contract("custom controller prompt")
    nested = json.loads(contract.split("Authoritative result JSON Schema:\n")[1])
    assert nested["$defs"] == schema("postmortem-reply")["$defs"]
    assert nested["$ref"] == "#/$defs/postmortem"


def test_repair_is_one_tool_free_turn_within_the_writer_budget(contract_boundary, monkeypatch):
    from langchain_core.messages import AIMessage
    from mlflow.environment_variables import MLFLOW_JUDGE_MAX_ITERATIONS
    from tl3_workers.runtime import ProxyChatOpenAI

    document, traces, values, calls = contract_boundary
    values.append(dict(written(0), went_wrong=["wrong shape", KEY]))
    inspected = []

    def repair(self, history):
        inspected.append(history)
        assert MLFLOW_JUDGE_MAX_ITERATIONS.get() == judges.MAX_TURNS
        assert "Your reply failed validation" in history[-1].text
        assert "went_wrong" in history[-1].text
        assert KEY not in json.dumps([item.text for item in history])
        return AIMessage(content=json.dumps(written(0)))

    original = judges.postmortem_judge

    def judge(prompt, model):
        assert MLFLOW_JUDGE_MAX_ITERATIONS.get() == judges.MAX_TURNS - 1
        return original(prompt, model)

    monkeypatch.setattr(judges, "postmortem_judge", judge)
    monkeypatch.setattr(ProxyChatOpenAI, "invoke", repair)
    result, _ = run(document)
    validate("postmortem-reply", result)
    assert len(inspected) == 1
    assert len(traces.logged) == 1


def test_deadline_covers_the_repair_turn(contract_boundary, monkeypatch):
    import time
    from tl3_workers.runtime import ProxyChatOpenAI

    document, traces, values, calls = contract_boundary
    document["inputs"]["deadline_secs"] = 1
    values.append(dict(written(0), went_wrong=["wrong shape"]))

    def slow(self, history):
        time.sleep(5)

    monkeypatch.setattr(ProxyChatOpenAI, "invoke", slow)
    with pytest.raises(postmortem.PostmortemDeadline):
        run(document)
    assert not traces.logged
    assert not (Path(document["worktree"]) / "judged.json").exists()


@pytest.mark.parametrize("fails", [False, True])
def test_assessment_posts_disable_implicit_http_retries(monkeypatch, fails):
    from types import SimpleNamespace
    from mlflow.environment_variables import MLFLOW_HTTP_REQUEST_MAX_RETRIES

    monkeypatch.setenv("MLFLOW_HTTP_REQUEST_MAX_RETRIES", "7")
    source = postmortem.MlflowTraces(None)

    def log(trace_id, feedback):
        assert feedback.assessment_id == "a-fixture"
        assert feedback.to_proto().assessment_id == "a-fixture"
        assert feedback.metadata["idempotency_key"] == "a-fixture"
        assert MLFLOW_HTTP_REQUEST_MAX_RETRIES.get() == 0
        if fails:
            raise OSError("response lost")
        return SimpleNamespace(assessment_id="a-1")

    source.client = SimpleNamespace(log_assessment=log)
    if fails:
        with pytest.raises(OSError):
            source.log("tr-1", "correctness", 4, "fixture", "judge", {"idempotency_key": "a-fixture"})
    else:
        assert source.log("tr-1", "correctness", 4, "fixture", "judge", {"idempotency_key": "a-fixture"}) == "a-1"
    assert MLFLOW_HTTP_REQUEST_MAX_RETRIES.get() == 7


def test_representative_repaired_postmortem_reaches_valid_final_envelope_once(
        tmp_path, model, monkeypatch):
    from langchain_core.messages import AIMessage
    from tl3_workers.runtime import ProxyChatOpenAI

    model.postmortem = dict(written(1), went_wrong=["The maker skipped a test."])
    source = FakeTraces([source_trace("maker", 1_000_000_000, {})])
    monkeypatch.setattr(postmortem, "SOURCE", lambda settings: source)
    repairs = []

    def repair(self, history):
        repairs.append(history)
        return AIMessage(content=json.dumps(written(1)))

    monkeypatch.setattr(ProxyChatOpenAI, "invoke", repair)
    document = request(tmp_path)
    result, _ = run(document)
    validate("postmortem-reply", result)
    assert len(repairs) == 1
    assert len(source.logged) == 2
    assert result["assessment_ids"] == ["a-1", "a-2"]
    assert run(document)[0] == result
    assert len(source.logged) == 2
    assert (Path(document["worktree"]) / "reply.json").exists()


@pytest.mark.parametrize("artifact, content", [
    ("assessments.json", "{invalid"),
    ("assessments.json", '["not a receipt mapping"]'),
    ("assessments.json", '{"correctness": 42}'),
    ("assessment-pending.json", '{"correctness": {"unexpected": "object"}}'),
])
@pytest.mark.durability
def test_invalid_assessment_cache_fails_before_external_writes(contract_boundary, artifact, content):
    document, traces, values, calls = contract_boundary
    (Path(document["worktree"]) / "judged.json").write_text(json.dumps(reply(0)))
    (Path(document["worktree"]) / artifact).write_text(content)
    with pytest.raises(ValueError, match="assessment cache failure"):
        run(document)
    assert not calls
    assert not traces.logged


@pytest.mark.durability
def test_restart_after_intent_before_post_resolves_to_one_assessment(contract_boundary, monkeypatch):
    document, traces, values, calls = contract_boundary
    values.append(written(0))
    save = postmortem._save
    crashed = []

    def crash(path, value):
        save(path, value)
        if path.name == "assessment-pending.json" and not crashed:
            crashed.append(True)
            raise KeyboardInterrupt("intent committed, POST not started")

    monkeypatch.setattr(postmortem, "_save", crash)
    with pytest.raises(KeyboardInterrupt):
        run(document)
    assert not traces.logged
    result, _ = run(document)
    validate("postmortem-reply", result)
    assert len(traces.logged) == 1
    assert result["assessment_ids"] == ["a-1"]
    assert len(calls) == 1
    assert run(document)[0] == result
    assert len(traces.logged) == 1


def test_assessment_key_is_stable_and_separates_each_identity_field():
    fields = ["run", "dimension", "model", "release"]
    key = postmortem.assessment_key(*fields)
    assert key == postmortem.assessment_key(*fields)
    assert key.startswith("a-") and len(key) == 34
    for index in range(len(fields)):
        changed = fields.copy()
        changed[index] += "-changed"
        assert postmortem.assessment_key(*changed) != key


@pytest.mark.durability
def test_pending_lookup_rechecks_after_bounded_wait_before_any_post(contract_boundary, monkeypatch):
    document, traces, values, calls = contract_boundary
    values.append(written(0))
    save = postmortem._save
    original_target = traces.traces[0].info.trace_id
    workdir = Path(document["worktree"])
    interrupted = []

    def crash(path, value):
        save(path, value)
        if path.name == "assessment-pending.json" and not interrupted:
            interrupted.append(True)
            raise KeyboardInterrupt()

    monkeypatch.setattr(postmortem, "_save", crash)
    with pytest.raises(KeyboardInterrupt):
        run(document)
    traces.traces.insert(0, source_trace("maker", 2_000_000_000, {}))
    queries, waits = [], []

    def absent(trace_id, key):
        queries.append((trace_id, key))
        return None

    monkeypatch.setattr(traces, "assessment", absent)
    monkeypatch.setattr(postmortem.time, "sleep", waits.append)
    result, _ = run(document)
    assert len(queries) == 2 and queries[0] == queries[1]
    assert queries[0][0] == original_target
    assert waits == [postmortem.ASSESSMENT_SETTLE_SECS]
    assert 0 < waits[0] <= 1
    assert len(traces.logged) == 1
    assert traces.logged[0]["trace_id"] == original_target
    assert traces.logged[0]["metadata"]["idempotency_key"] == queries[0][1]
    assert result["assessment_ids"] == ["a-1"]
    assert json.loads((workdir / "assessment-pending.json").read_text()) == {}


@pytest.mark.durability
def test_stale_lookup_then_visible_assessment_resolves_without_another_post(
        contract_boundary, monkeypatch):
    document, traces, values, calls = contract_boundary
    values.append(written(0))
    log, lookup = traces.log, traces.assessment
    posts, queries, waits = [], [], []

    def lost_response(*args):
        posts.append(args)
        log(*args)
        raise KeyboardInterrupt("POST committed, response lost")

    traces.log = lost_response
    with pytest.raises(KeyboardInterrupt):
        run(document)

    def stale_once(trace_id, key):
        queries.append(key)
        return None if len(queries) == 1 else lookup(trace_id, key)

    monkeypatch.setattr(traces, "assessment", stale_once)
    monkeypatch.setattr(postmortem.time, "sleep", waits.append)
    result, _ = run(document)
    assert len(posts) == 1 and len(traces.logged) == 1
    assert len(queries) == 2 and queries[0] == queries[1]
    assert waits == [postmortem.ASSESSMENT_SETTLE_SECS]
    assert result["assessment_ids"] == ["a-1"]


@pytest.mark.durability
def test_server_enforced_assessment_id_prevents_duplicate_when_both_lookups_are_stale(
        contract_boundary, monkeypatch):
    from types import SimpleNamespace

    document, traces, values, calls = contract_boundary
    values.append(written(0))
    source = postmortem.MlflowTraces(None)
    source.find = traces.find
    committed, posts, reads = {}, [], []

    def log(trace_id, feedback):
        wire = feedback.to_proto()
        key = feedback.metadata["idempotency_key"]
        assert wire.assessment_id == key
        posts.append(wire.assessment_id)
        if wire.assessment_id in committed:
            raise ValueError("existing assessment primary key")
        committed[wire.assessment_id] = feedback
        raise OSError("commit succeeded, response lost")

    def info(trace_id):
        reads.append(trace_id)
        return SimpleNamespace(assessments=[] if len(reads) <= 2 else list(committed.values()))

    source.client = SimpleNamespace(log_assessment=log, get_trace_info=info)
    monkeypatch.setattr(postmortem, "SOURCE", lambda settings: source)
    monkeypatch.setattr(postmortem.time, "sleep", lambda seconds: None)
    with pytest.raises(ValueError, match="assessment write failure"):
        run(document)
    with pytest.raises(ValueError, match="assessment write failure"):
        run(document)
    result, _ = run(document)
    validate("postmortem-reply", result)
    assert len(committed) == 1
    assert len(posts) == 2 and posts[0] == posts[1]
    assert result["assessment_ids"] == [posts[0]]
    assert len(calls) == 1


@pytest.mark.parametrize("tool_turns", [1, 20])
def test_judge_turn_counts_and_limits_reach_spans_and_summary(tmp_path, monkeypatch, tool_turns):
    import os
    from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import ExportTraceServiceRequest
    from mlflow.genai.judges.adapters import gateway_adapter

    model = FakeModel(tool_turns=tool_turns)
    monkeypatch.setattr(gateway_adapter, "send_chat_request", model)
    monkeypatch.setattr(postmortem, "SOURCE", lambda settings: FakeTraces([
        source_trace("maker", 1_000_000_000, {})]))
    document = request(tmp_path)
    log, summary = tmp_path / "spans.jsonl", tmp_path / "memory.json"
    document["mlflow"] = {"tracking_uri": "http://unused.invalid", "experiment": "test"}
    document["trace"] = {"log": str(log)}
    document["memory"] = {"summary_file": str(summary)}
    monkeypatch.setattr(tracing.SpanLog, "start_shipper", lambda self: None)
    monkeypatch.setattr(tracing.SpanLog, "finish", lambda self: os.close(self.fd))

    result, _ = run(document)
    validate("postmortem-reply", result)
    # Every judge keeps its last turn for a reply repair (#728).
    expected = [{"stage": stage, "turn_count": tool_turns + 1, "iteration_limit": 47}
                for stage in ["grade", "grade", "postmortem"]]
    recorded = json.loads(summary.read_text())
    assert recorded.get("judge_runs") == expected
    assert recorded["outcome"] == "succeeded"
    records = [json.loads(line) for line in log.read_text().splitlines()]
    packet = ExportTraceServiceRequest.FromString(base64.b64decode(
        next(item["otlp"] for item in records if item["kind"] == "final")))
    spans = packet.resource_spans[0].scope_spans[0].spans
    measured = [span for span in spans if span.name in {"grade judge", "postmortem judge"}]
    assert len(measured) == 3
    for span, metrics in zip(measured, expected, strict=True):
        attrs = {item.key: item.value for item in span.attributes}
        for field in ("turn_count", "iteration_limit"):
            value = attrs["tl3.judge." + field]
            assert value.WhichOneof("value") == "int_value"
            assert value.int_value == metrics[field]


@pytest.fixture
def fake_reply_checker(tmp_path, monkeypatch):
    import sys

    def install(body):
        checker = tmp_path / 'fake-tl3'
        checker.write_text(f'#!{sys.executable}\n' + body)
        checker.chmod(0o755)
        monkeypatch.setenv('TL3_BINARY', str(checker))
        return checker
    return install


@pytest.mark.parametrize('violations', [[], ["a recommendation's before and after must fit its operation"]])
def test_postmortem_shim_transports_exact_reply_and_repair(fake_reply_checker, violations):
    from tl3_workers.turns import InvalidReply, TurnControl, repaired

    inputs, output = {'case': 'café'}, reply(0)
    fake_reply_checker(
        'import json, sys\n'
        "assert sys.argv[1:] == ['check-reply', 'postmortem']\n"
        f'assert json.load(sys.stdin) == {dict(inputs=inputs, reply=output)!r}\n'
        f'print({json.dumps(dict(violations=violations))!r})\n')
    if not violations:
        assert postmortem.check_reply(inputs, output) is None
        return
    history = []
    with pytest.raises(InvalidReply) as caught:
        repaired(TurnControl(), history, lambda: output,
                 lambda value: postmortem._checked(inputs, value))
    first = violations[0]
    assert all(str(error) == first for error in caught.value.errors)
    assert tracing.failure_tags(caught.value.errors[0]) == {
        'failure_class': 'ReplyViolation', 'failure_message': first}
    assert history[0].text == (
        f'Your reply failed validation: {first}\nTools are unavailable for this turn. Return one '
        'corrected JSON object matching your reply schema and nothing else.')


@pytest.mark.parametrize('body,message', [
    ("print('null')\n", 'reply validator returned invalid output'),
    ("print('{\"violations\": false}')\n", 'reply validator returned invalid output'),
    ("print('{\"violations\": [null]}')\n", 'reply validator returned invalid output'),
    ("print('private-validator-canary')\n", 'reply validator returned invalid output'),
    ("import sys\nprint('private-validator-canary', file=sys.stderr)\nsys.exit(7)\n",
     'reply validator process failed'),
    ("import time\ntime.sleep(0.3)\n", 'reply validator timed out'),
])
def test_postmortem_validator_failure_bypasses_repair(fake_reply_checker, monkeypatch, body, message):
    from tl3_workers import reply_check
    from tl3_workers.turns import TurnControl, repaired

    fake_reply_checker(body)
    if body.startswith('import time'):
        monkeypatch.setattr(reply_check, 'TIMEOUT_SECONDS', 0.05)
    history, asked = [], []

    def ask():
        asked.append(True)
        return reply(0)

    with pytest.raises(reply_check.ReplyCheckInfrastructureError) as caught:
        repaired(TurnControl(), history, ask, lambda value: postmortem.check_reply({}, value))
    assert history == [] and asked == [True]
    assert tracing.failure_tags(caught.value) == {
        'failure_class': 'ReplyCheckInfrastructureError', 'failure_message': message}



def test_validator_infrastructure_failure_precedes_checkpoints_and_assessments(
        contract_boundary, fake_reply_checker):
    from tl3_workers.reply_check import ReplyCheckInfrastructureError

    document, traces, values, calls = contract_boundary
    values.append(written(0))
    fake_reply_checker("print('private-validator-canary')\n")
    with pytest.raises(ReplyCheckInfrastructureError) as caught:
        run(document)
    assert tracing.failure_tags(caught.value)['failure_message'] == (
        'reply validator returned invalid output')
    assert len(calls) == 1
    assert not traces.logged
    for artifact in ['judged.json', 'assessment-pending.json', 'assessments.json', 'reply.json']:
        assert not (Path(document['worktree']) / artifact).exists()
