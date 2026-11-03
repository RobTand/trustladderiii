"""#687: judge causes survive the safe trace and durable run-failure boundaries."""

import base64
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
from tl3_workers.roles import postmortem
from test_postmortem import FakeTraces, KEY, request, run
from test_postmortem import judge_environment as judge_environment
from test_postmortem import release as release


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


@pytest.mark.parametrize("stage", ["grade", "postmortem"])
@pytest.mark.parametrize("mode", ["raised", "feedback", "ensemble"])
@pytest.mark.parametrize("category,code", CASES, ids=[item[0] for item in CASES])
def test_each_judge_cause_reaches_failed_span_and_run_record(
        tmp_path, monkeypatch, stage, mode, category, code):
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
    records = [json.loads(line) for line in log.read_text().splitlines()]
    packet = ExportTraceServiceRequest.FromString(base64.b64decode(
        next(item["otlp"] for item in records if item["kind"] == "final")))
    spans = packet.resource_spans[0].scope_spans[0].spans
    root = next(span for span in spans if not span.parent_span_id)
    assert root.status.code == 2
    attrs = {item.key: item.value.string_value for item in root.attributes}
    for key, value in reason.items():
        assert attrs["mlflow.traceTag." + key] == value
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
