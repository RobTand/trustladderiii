"""#687: judge causes survive the safe trace and durable run-failure boundaries."""

import base64
import copy
import json
import os
from pathlib import Path

from mlflow.entities import AssessmentError, Feedback
from mlflow.exceptions import MlflowException
from mlflow.genai.judges.adapters import gateway_adapter
from mlflow.genai.judges.adapters.utils import ChatCompletionError
from mlflow.protos.databricks_pb2 import INTERNAL_ERROR, REQUEST_LIMIT_EXCEEDED
from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import ExportTraceServiceRequest
import pytest
import requests

from tl3_workers import judges, tracing
from tl3_workers.contracts import validate
from tl3_workers.roles import postmortem
from postmortem_support import FakeTraces, KEY, request, run
from postmortem_support import judge_environment as judge_environment
from postmortem_support import release as release


PRIVATE = f"request-fragment-687 {KEY}"
CASES = [
    ("quota", "RATE_LIMIT_EXCEEDED"),
    ("refusal", "PERMISSION_DENIED"),
    ("http", "ChatCompletionError"),
    ("deadline", "TimeoutError"),
    ("transport", "ConnectionError"),
    ("schema", "JSONDecodeError"),
    ("contract", "KeyError"),
    ("configuration", "INVALID_PARAMETER_VALUE"),
    ("iteration_limit", "REQUEST_LIMIT_EXCEEDED"),
    ("mlflow", "MlflowException"),
    ("other", PRIVATE),
]


def raised_failure(category):
    if category in {"quota", "refusal", "http"}:
        return ChatCompletionError({"quota": 429, "refusal": 403, "http": 502}[category], PRIVATE)
    if category == "deadline":
        return requests.Timeout(PRIVATE)
    if category == "transport":
        return requests.ConnectionError(PRIVATE)
    if category == "schema":
        return json.JSONDecodeError(PRIVATE, PRIVATE, 0)
    if category == "contract":
        return KeyError(PRIVATE)
    if category == "configuration":
        return ValueError("judges are not configured")
    if category == "iteration_limit":
        return MlflowException(PRIVATE, error_code=REQUEST_LIMIT_EXCEEDED)
    if category == "mlflow":
        return MlflowException(PRIVATE, error_code=INTERNAL_ERROR)
    # Even the type name is untrusted and must never be exported.
    return type(PRIVATE, (Exception,), {})(PRIVATE)


MODES = ["raised", "feedback", "ensemble"]
# A malformed grade reply grades null instead of failing the run (#728).
FATAL = [pytest.param(stage, category, code, id=f"{stage}-{category}")
         for stage in ("grade", "postmortem") for category, code in CASES
         if (stage, category) != ("grade", "schema")]


def instrumented(tmp_path, monkeypatch):
    document = request(tmp_path)
    log, summary = tmp_path / "spans.jsonl", tmp_path / "memory.json"
    document["mlflow"] = {"tracking_uri": "http://unused.invalid", "experiment": "test"}
    document["trace"] = {"log": str(log)}
    document["memory"] = {"summary_file": str(summary), "repo": "RobTand/tl3-sandbox",
                          "issue": 42, "request": "3", "epoch": 1}
    # Keep the real durable OTLP export; no network shipper is needed to read it.
    monkeypatch.setattr(tracing.SpanLog, "start_shipper", lambda self: None)
    monkeypatch.setattr(tracing.SpanLog, "finish", lambda self: os.close(self.fd))
    monkeypatch.setattr(postmortem, "SOURCE", lambda settings: FakeTraces([]))
    return document, log, summary


def exported_spans(log):
    records = [json.loads(line) for line in log.read_text().splitlines()]
    packet = ExportTraceServiceRequest.FromString(base64.b64decode(
        next(item["otlp"] for item in records if item["kind"] == "final")))
    return packet, packet.resource_spans[0].scope_spans[0].spans


@pytest.mark.parametrize("mode", MODES)
@pytest.mark.parametrize("stage,category,code", FATAL)
def test_each_judge_cause_reaches_failed_span_and_run_record(
        tmp_path, monkeypatch, stage, mode, category, code):
    document, log, summary = instrumented(tmp_path, monkeypatch)

    def fail(**kwargs):
        if mode == "raised":
            raise raised_failure(category)
        if mode == "ensemble":
            return Feedback(value="no consensus", metadata={"mlflow.ensemble.sub_feedbacks":
                json.dumps([{"feedback": {"error": {"error_code": code,
                                                    "error_message": PRIVATE}}}])})
        return Feedback(error=AssessmentError(error_code=code, error_message=PRIVATE))

    if mode == "raised":
        # Exercise MLflow's actual wrappers, including their chained exceptions.
        monkeypatch.setattr(gateway_adapter, "send_chat_request", lambda **kwargs: fail())
    elif stage == "grade":
        monkeypatch.setattr(judges, "grade_judges", lambda *args: {"correctness": fail})
    else:
        monkeypatch.setattr(judges, "postmortem_judge", lambda *args: fail)
    if stage == "postmortem":
        monkeypatch.setattr(judges, "grade_judges", lambda *args: {
            "correctness": lambda **kwargs: Feedback(value=4, rationale="fixture")})

    with pytest.raises(tracing.WorkerStageError):
        run(document)
    reason = json.loads(summary.read_text())["reason"]
    assert reason["failure_class"] == category
    assert reason["failure_stage"] == f"{stage} judge failure"
    assert reason["failure_message"].startswith(f"{stage} judge failure: {category}")
    if mode == "raised" and category in {"quota", "refusal", "http"}:
        status = str({"quota": 429, "refusal": 403, "http": 502}[category])
        assert status in reason["failure_message"]
        assert reason["failure_http_status"] == status
    judge_runs = json.loads(summary.read_text()).get("judge_runs", [])
    measured = [item for item in judge_runs if item["stage"] == stage]
    assert len(measured) == 1
    assert type(measured[0]["turn_count"]) is int
    assert measured[0]["turn_count"] == (1 if mode == "raised" else 0)
    # Every judge keeps its last turn for a reply repair (#728).
    assert measured[0]["iteration_limit"] == 47
    packet, spans = exported_spans(log)
    root = next(span for span in spans if not span.parent_span_id)
    assert root.status.code == 2
    attrs = {item.key: item.value.string_value for item in root.attributes}
    for key, value in reason.items():
        assert attrs["mlflow.traceTag." + key] == value
    judge_span = next(span for span in spans if span.name == f"{stage} judge")
    metrics = {item.key: item.value for item in judge_span.attributes}
    for field in ("turn_count", "iteration_limit"):
        value = metrics["tl3.judge." + field]
        assert value.WhichOneof("value") == "int_value"
        assert value.int_value == measured[0][field]
    assert all(not span.events and not span.status.message for span in spans)
    exported = packet.SerializeToString() + summary.read_bytes() + log.read_bytes()
    assert PRIVATE.encode() not in exported
    assert KEY.encode() not in exported
    assert b"request-fragment-687" not in exported
    assert not (Path(document["worktree"]) / "judged.json").exists()


@pytest.mark.parametrize("feedback", [None, Feedback(value=4, metadata={
    "mlflow.ensemble.sub_feedbacks": "broken JSON"})])
def test_missing_or_malformed_feedback_has_a_contract_cause(feedback):
    with pytest.raises(tracing.WorkerStageError) as caught:
        postmortem._judge(lambda: feedback, "grade judge failure")
    assert tracing.failure_tags(caught.value)["failure_class"] == "contract"


# #728: a grade judge's malformed final reply gets one tool-free repair turn,
# and a grade still malformed after it is null instead of failing the run.
WRITTEN = {
    "summary": "The maker needed a fix round.", "cause": "tl3",
    "went_wrong": "The maker skipped a test.",
    "timeline": [{"at": "Making", "event": "The maker ran."}],
    "process_failures": [{"phase": "maker", "kind": "prompt or skill defect",
                          "what": "It skipped a test.", "evidence": "tr-1"}],
    "recommendations": [], "notes": "", "lessons": ["Sandbox tests need PrismaBuild."],
}


def graded(value):
    return json.dumps({"rationale": "tr-1 shows it", "result": value})


class GradeModel:
    """CLIProxyAPI for the grade judges: one tool turn, then each final reply
    from `finals`, and each repair reply from `repairs`, in order."""

    def __init__(self, finals, repairs=()):
        self.finals, self.repairs = list(finals), list(repairs)
        self.payloads, self.repaired = [], []

    def __call__(self, endpoint, headers, payload, num_retries):
        self.payloads.append(copy.deepcopy(payload))
        messages = payload["messages"]
        if payload.get("tool_choice") == "none":
            self.repaired.append(copy.deepcopy(payload))
            return self.answer(self.repairs.pop(0))
        if not any(message.get("role") == "tool" for message in messages):
            call = {"id": f"call-{len(self.payloads)}", "type": "function",
                    "function": {"name": "search_trace_regex", "arguments": json.dumps(
                        {"pattern": "case", "max_matches": 5,
                         "surrounding_content_length": 100})}}
            return {"id": "r", "object": "chat.completion", "model": "m", "created": 0,
                    "choices": [{"index": 0, "finish_reason": "tool_calls",
                                 "message": {"role": "assistant", "content": None,
                                             "tool_calls": [call]}}],
                    "usage": {"prompt_tokens": 10, "completion_tokens": 1, "total_tokens": 11}}
        return self.answer(self.finals.pop(0))

    @staticmethod
    def answer(content):
        return {"id": "r", "object": "chat.completion", "model": "m", "created": 0,
                "choices": [{"index": 0, "finish_reason": "stop",
                             "message": {"role": "assistant", "content": content}}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 1, "total_tokens": 11}}


@pytest.fixture
def graders(tmp_path, monkeypatch):
    def install(finals, repairs=(), directory=tmp_path):
        model = GradeModel(finals, repairs)
        monkeypatch.setattr(gateway_adapter, "send_chat_request", model)
        monkeypatch.setattr(judges, "postmortem_judge", lambda *args: (
            lambda **kwargs: Feedback(value=json.dumps(WRITTEN), rationale="fixture")))
        document, log, summary = instrumented(directory, monkeypatch)
        return model, document, log, summary
    return install


def repairs(log):
    """The repair outcome and failure class on each repaired grade judge span."""
    _, spans = exported_spans(log)
    found = []
    for span in spans:
        attrs = {item.key: item.value for item in span.attributes}
        if span.name == "grade judge" and "tl3.judge.repair" in attrs:
            found.append((attrs["tl3.judge.repair"].string_value,
                          attrs["tl3.judge.failure_class"].string_value
                          if "tl3.judge.failure_class" in attrs else None))
    return found


def test_a_malformed_grade_reply_is_repaired_once_and_accepted(graders):
    # Sol at low effort ended a grade judge's tool loop with an empty reply.
    model, document, log, summary = graders(finals=["", graded(5)], repairs=[graded(3)])
    reply, _ = run(document)
    validate("postmortem-reply", reply)
    assert {grade["dimension"]: grade["grade"] for grade in reply["grades"]} == {
        "correctness": 3, "fix round": 5}
    [repair] = model.repaired
    assert repair["messages"][-2] == {"role": "assistant", "content": ""}
    assert repair["messages"][-1]["role"] == "user"
    assert "failed validation" in repair["messages"][-1]["content"]
    assert repair.get("response_format") == model.payloads[0].get("response_format")
    recorded = json.loads(summary.read_text())
    assert recorded["outcome"] == "succeeded"
    # The repair is one more turn of the same judge, inside its 48.
    assert [(item["turn_count"], item["iteration_limit"]) for item in recorded["judge_runs"]
            if item["stage"] == "grade"] == [(3, 47), (2, 47)]
    assert repairs(log) == [("accepted", None)]


def test_a_grade_still_malformed_after_repair_is_null_and_the_postmortem_completes(graders):
    model, document, log, summary = graders(finals=["", graded(5)],
                                            repairs=["The grade is 3."])
    reply, _ = run(document)
    validate("postmortem-reply", reply)
    grades = {grade["dimension"]: grade for grade in reply["grades"]}
    assert grades["correctness"]["grade"] is None
    assert "schema" in grades["correctness"]["rationale"]
    assert grades["fix round"]["grade"] == 5
    assert len(model.repaired) == 1, "one bounded repair turn"
    recorded = json.loads(summary.read_text())
    assert recorded["outcome"] == "succeeded" and recorded["reason"] is None
    assert repairs(log) == [("failed", "schema")]


@pytest.mark.parametrize("mode", MODES)
def test_a_schema_grade_failure_without_a_usable_reply_is_null(tmp_path, monkeypatch, mode):
    document, log, summary = instrumented(tmp_path, monkeypatch)

    def fail(**kwargs):
        if mode == "raised":
            raise raised_failure("schema")
        if mode == "ensemble":
            return Feedback(value="no consensus", metadata={"mlflow.ensemble.sub_feedbacks":
                json.dumps([{"feedback": {"error": {"error_code": "JSONDecodeError",
                                                    "error_message": PRIVATE}}}])})
        return Feedback(error=AssessmentError(error_code="JSONDecodeError",
                                              error_message=PRIVATE))

    if mode == "raised":
        monkeypatch.setattr(gateway_adapter, "send_chat_request", lambda **kwargs: fail())
    else:
        monkeypatch.setattr(judges, "grade_judges", lambda *args: {"correctness": fail})
    monkeypatch.setattr(judges, "postmortem_judge", lambda *args: (
        lambda **kwargs: Feedback(value=json.dumps(WRITTEN), rationale="fixture")))
    reply, _ = run(document)
    assert {grade["grade"] for grade in reply["grades"]} == {None}
    recorded = json.loads(summary.read_text())
    assert recorded["outcome"] == "succeeded"
    exported = summary.read_bytes() + log.read_bytes() + json.dumps(reply).encode()
    assert PRIVATE.encode() not in exported and KEY.encode() not in exported


def test_three_malformed_grades_in_a_row_no_longer_fail_postmortems(graders, tmp_path):
    # The controller pauses postmortems after three failed runs in a row; each
    # of these runs must now succeed instead.
    for attempt in range(3):
        directory = tmp_path / f"run-{attempt}"
        directory.mkdir()
        model, document, log, summary = graders(finals=["", "not JSON"],
                                                repairs=["", "{\"result\": "],
                                                directory=directory)
        reply, _ = run(document)
        validate("postmortem-reply", reply)
        assert [grade["grade"] for grade in reply["grades"]] == [None, None]
        recorded = json.loads(summary.read_text())
        assert recorded["outcome"] == "succeeded" and recorded["reason"] is None
        assert len(model.repaired) == 2
