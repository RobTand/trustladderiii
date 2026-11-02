"""Judge a finished request: grade it by TL3's rubric, then write its postmortem (#403).

The judges read the request's own MLflow traces through MLflow's trace tools.
Grades are logged as assessments on the request's newest trace. The run caches
each step under its run key, so a run restarted after a crash pays for no judge
twice and logs no assessment twice.
"""

import base64
import json
from pathlib import Path
import secrets
import time

from tl3_workers import judges, tracing
from tl3_workers.contracts import read_reply
from tl3_workers.roles.base import Role


# The newest runs are judged, and their tokens counted; the reply says how many.
MAX_TRACES = 24
MAX_RECOMMENDATIONS = 3
# Trace attributes that hold structure rather than content.
STRUCTURAL = {"mlflow.spanType", "mlflow.traceRequestId", "mlflow.chat.tokenUsage",
              "mlflow.llm.model", "tl3.role", "tl3.proxy.usage_chunks",
              "tl3.proxy.usage_chunks_truncated", "tl3.trace.truncated",
              "tl3.trace.omitted_spans", "tl3.source_trace"}
USAGE = ("input_tokens", "output_tokens", "cache_read_input_tokens",
         "cache_creation_input_tokens")
# The release's own files, which a recommendation's target names from the
# repository root: the worker release ships workers/ and harness/ (#372).
RELEASE_ROOT = Path(__file__).resolve().parents[4]


class PostmortemDeadline(BaseException):
    """The run passed its whole wall-clock allowance (#486). A BaseException, so
    no judge's error handling turns it into a judge failure."""


def unusable(item, root):
    """Why a recommendation can't apply to the release, or None."""
    del item, root
    return None


class MlflowTraces:
    """The node's traces in TL3's experiment, and assessments logged on them."""

    def __init__(self, settings):
        self.client = self.experiment = None
        if not settings:
            # Without MLflow the judges read the case file alone and log nothing.
            return
        from mlflow import MlflowClient
        from mlflow.tracing.client import TracingClient

        self.client = TracingClient(tracking_uri=settings["tracking_uri"])
        experiment = MlflowClient(tracking_uri=settings["tracking_uri"]).get_experiment_by_name(
            settings["experiment"])
        self.experiment = experiment.experiment_id if experiment else None

    def find(self, tags, limit):
        if self.experiment is None:
            return []
        clauses = " AND ".join(f"tags.{key} = '{value}'" for key, value in sorted(tags.items()))
        found = self.client.search_traces(locations=[self.experiment], filter_string=clauses,
                                          max_results=limit * 2,
                                          order_by=["timestamp_ms DESC"], include_spans=True)
        return [trace for trace in found if trace.info.tags.get("role") != "postmortem"][:limit]

    def log(self, trace_id, name, value, rationale, judge_model, metadata):
        from mlflow.entities import AssessmentSource, Feedback

        feedback = Feedback(name=name, value=value, rationale=rationale,
                            source=AssessmentSource(source_type="LLM_JUDGE", source_id=judge_model),
                            metadata=metadata)
        return self.client.log_assessment(trace_id, feedback).assessment_id


# Tests replace this with a fake; production reads MLflow.
SOURCE = MlflowTraces


def scrub(value, redactor):
    """Redact every string in a JSON value."""
    if isinstance(value, str):
        return redactor.text(value)
    if isinstance(value, list):
        return [scrub(item, redactor) for item in value]
    if isinstance(value, dict):
        return {key: scrub(item, redactor) for key, item in value.items()}
    return value


def redactor_for(request):
    redactor = tracing.Redactor(request.get("key_file"))
    if key := tracing.model_key.get():
        # The key create_model loaded for this run.
        redactor.secrets.add(key)
    return redactor


def redact_case(inputs, request):
    """The case file reaches a judge only through #302's redaction."""
    return {**inputs, "case": scrub(inputs["case"], redactor_for(request))}


def _b64(nbytes):
    return base64.b64encode(secrets.token_bytes(nbytes)).decode()


def _redacted_attributes(attributes, request_id, source, redactor):
    clean = {}
    for key, raw in attributes.items():
        if key in STRUCTURAL:
            clean[key] = raw
            continue
        try:
            value = json.loads(raw) if isinstance(raw, str) else raw
        except ValueError:
            value = raw
        clean[key] = json.dumps(scrub(value, redactor), ensure_ascii=False)
    clean["mlflow.traceRequestId"] = json.dumps(request_id)
    clean["tl3.source_trace"] = json.dumps(source)
    return clean


def case_trace(traces, redactor, case):
    """One in-memory trace holding every source run under a case root span.

    Span content is redacted again here; events and status messages, which can
    hold raw exception text, are dropped."""
    from mlflow.entities import Trace, TraceData, TraceInfo, TraceLocation, TraceState
    from mlflow.entities.span import Span

    raw = secrets.token_bytes(16)
    otel_trace = base64.b64encode(raw).decode()
    request_id = "tr-" + raw.hex()
    root_id = _b64(8)
    spans, starts, ends, runs = [], [], [], []
    for trace in reversed(traces):
        source = trace.info.trace_id
        tags = trace.info.tags
        for span in trace.data.spans:
            data = span.to_dict()
            data["trace_id"] = otel_trace
            data["events"] = []
            data["status"] = {"code": data["status"]["code"], "message": ""}
            data["links"] = []
            data["name"] = redactor.text(data["name"])[:256]
            data["attributes"] = _redacted_attributes(data["attributes"], request_id, source,
                                                      redactor)
            if not data.get("parent_span_id"):
                data["parent_span_id"] = root_id
                data["name"] = (f"{data['name']} [{source} role={tags.get('role')} "
                                f"model={tags.get('model')} outcome={tags.get('outcome')}]")
            starts.append(int(data["start_time_unix_nano"]))
            ends.append(int(data["end_time_unix_nano"]))
            spans.append(data)
        runs.append({"trace_id": source, **{key: tags.get(key) for key in
                                             ("role", "model", "outcome", "attempt", "round")},
                     "spans": len(trace.data.spans)})
    now = time.time_ns()
    start, end = min(starts, default=now), max(ends, default=now)
    root = {
        "trace_id": otel_trace, "span_id": root_id, "parent_span_id": None,
        "name": f"case {case}", "start_time_unix_nano": start, "end_time_unix_nano": end,
        "events": [], "status": {"code": "STATUS_CODE_OK", "message": ""}, "links": [],
        "attributes": {
            "mlflow.traceRequestId": json.dumps(request_id),
            "mlflow.spanType": json.dumps("CHAIN"),
            "mlflow.spanInputs": json.dumps({"case": case}),
            "mlflow.spanOutputs": json.dumps(
                {"runs": runs} if runs else {"runs": [], "note": "no traces were found"}),
        },
    }
    info = TraceInfo(trace_id=request_id, trace_location=TraceLocation.from_experiment_id("0"),
                     request_time=start // 1_000_000, state=TraceState.OK,
                     execution_duration=(end - start) // 1_000_000, tags={})
    return Trace(info=info, data=TraceData(spans=[Span.from_dict(root)]
                                           + [Span.from_dict(span) for span in spans]))


def usage(traces):
    """Tokens from the runs' trace tags, which tracing records per run."""
    totals = {"traces": len(traces), **{key: 0 for key in USAGE}}
    for trace in traces:
        for key in USAGE:
            value = trace.info.tags.get(key)
            if isinstance(value, str) and value.isdigit():
                totals[key] += int(value)
    return totals


def check_reply(inputs, reply):
    recommendations = reply["postmortem"]["recommendations"]
    if len(recommendations) > MAX_RECOMMENDATIONS:
        raise ValueError("at most three recommendations")
    for item in recommendations:
        before, after = item["before"], item["after"]
        if (item["operation"] == "add" and (before or not after)) or (
                item["operation"] == "remove" and (after or not before)) or (
                item["operation"] == "replace" and not (before and after)):
            raise ValueError("a recommendation's before and after must fit its operation")


def _load(path):
    return json.loads(path.read_text()) if path.exists() else None


def _save(path, value):
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(json.dumps(value, ensure_ascii=False))
    temporary.replace(path)


def _judge(run):
    try:
        feedback = run()
    except Exception as error:
        raise ValueError("judge failed") from error
    if judges.failed(feedback):
        raise ValueError("judge failed")
    return feedback


def run(model, request, worktree, emit):
    del model, worktree  # The judges call the proxy through MLflow, not LangChain.
    workdir = Path(request["worktree"])
    if (cached := _load(workdir / "reply.json")) is not None:
        return cached
    inputs = request["inputs"]
    redactor = redactor_for(request)
    settings = tracing.mlflow_settings(request)
    source = SOURCE(settings)
    traces = source.find(inputs["trace_tags"], MAX_TRACES)
    postmortem_model = request["model"]["name"]
    grade_model = inputs["grade_model"]
    judged = _load(workdir / "judged.json")
    if judged is None:
        judges.configure(request["model"]["base_url"], tracing.model_key.get())
        trace = case_trace(traces, redactor, inputs["record"]["id"])
        case = inputs["case"]

        def progress():
            emit({"event": "progress", "unit": "model_turn"})

        grades = []
        with judges.guarded_tools(redactor, progress):
            for dimension, grader in judges.grade_judges(inputs["rubric"], grade_model).items():
                feedback = _judge(lambda: grader(inputs=case, trace=trace))
                value = feedback.value
                grade = value if type(value) is int and 1 <= value <= 5 else None
                grades.append({"dimension": dimension, "grade": grade,
                               "rationale": redactor.text(feedback.rationale or "", 4000)[:4000],
                               "judge_model": grade_model})
            prompt = run_prompt()
            feedback = _judge(lambda: judges.postmortem_judge(prompt, postmortem_model)(
                inputs=case, trace=trace))
        written = feedback.value if isinstance(feedback.value, dict) else read_reply(
            str(feedback.value))
        judged = {"postmortem": scrub(written, redactor), "grades": grades}
        _save(workdir / "judged.json", judged)
    logged = _load(workdir / "assessments.json") or {}
    target = traces[0].info.trace_id if traces else None
    tags = inputs["trace_tags"]
    for grade in judged["grades"]:
        dimension = grade["dimension"]
        if target and grade["grade"] is not None and dimension not in logged:
            logged[dimension] = source.log(target, dimension, grade["grade"], grade["rationale"],
                                           grade["judge_model"], {
                **{key: str(tags[key]) for key in ("node", "request", "repo", "issue")},
                "release": inputs["release"],
                "rubric_version": str(inputs["rubric"]["version"]),
                "dimension": dimension, "judge_model": grade["judge_model"]})
            _save(workdir / "assessments.json", logged)
        grade["assessment_id"] = logged.get(dimension)
    reply = {"postmortem": judged["postmortem"], "grades": judged["grades"],
             "assessment_ids": [logged[key] for key in sorted(logged)],
             "usage": usage(traces),
             "models": {"postmortem": postmortem_model, "grades": grade_model}}
    _save(workdir / "reply.json", reply)
    return reply


def run_prompt():
    # The runtime imports the roles, so it is imported when a run needs it.
    from tl3_workers.runtime import role_prompt
    return role_prompt(ROLE)


ROLE = Role(
    name="postmortem",
    prompt="postmortem.md",
    input_schema="postmortem-input",
    reply_schema="postmortem-reply",
    tools={},
    check_reply=check_reply,
    run=run,
    prepare_inputs=redact_case,
)
