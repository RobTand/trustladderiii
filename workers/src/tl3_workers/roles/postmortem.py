"""Judge a finished request: grade it by TL3's rubric, then write its postmortem (#403).

The judges read the request's own MLflow traces through MLflow's trace tools.
Grades are logged as assessments on the request's newest trace. The run caches
each step under its run key, so a run restarted after a crash pays for no judge
twice and logs no assessment twice.
"""

import base64
from contextlib import contextmanager
import hashlib
import json
from pathlib import Path
import secrets
import signal
import sys
import time

from jsonschema import ValidationError
from langchain_core.messages import AIMessage, HumanMessage, SystemMessage

from tl3_workers import judges, tracing
from tl3_workers.contracts import read_json, read_reply
from tl3_workers.roles.base import Role
from tl3_workers.turns import TurnControl, repaired


# The newest runs are judged, and their tokens counted; the reply says how many.
MAX_TRACES = 24
MAX_RECOMMENDATIONS = 3
ASSESSMENT_SETTLE_SECS = 0.1
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
    """Why a recommendation can't apply to the release, or None. TL1's rule: its
    target is a file the release ships, and its before text appears there
    exactly once, so slice C's curator never sees an edit that can't apply."""
    target = item["target"]
    root = Path(root).resolve()
    path = (root / target).resolve()
    if Path(target).is_absolute() or not path.is_relative_to(root) or not path.is_file():
        return "not a file in the release"
    if item["operation"] == "add":
        return None
    count = path.read_text(encoding="utf-8", errors="replace").count(item["before"])
    return None if count == 1 else f"before appears {count} times"


# The reply schema's cap on a postmortem's notes.
NOTES_CHARS = 2000


def applicable(recommendations, root):
    """The recommendations that apply to the release, and a sentence for each
    dropped one, which is also logged."""
    kept, dropped = [], []
    for rank, item in enumerate(recommendations, 1):
        if (reason := unusable(item, root)) is None:
            kept.append(item)
        else:
            dropped.append(f"Dropped recommendation {rank}, {item['target'][:200]}: {reason}.")
            print(f"tl3_workers: postmortem: {dropped[-1]}", file=sys.stderr)
    return kept, dropped


def with_drops(notes, dropped):
    """The judge's notes followed by the drop reasons, within the notes' cap.
    The reasons are kept whole; the judge's notes give way."""
    if not dropped:
        return notes
    reasons = " ".join(dropped)[:NOTES_CHARS]
    room = NOTES_CHARS - len(reasons) - 1
    notes = notes.strip()[:room] if room > 0 else ""
    return f"{notes} {reasons}" if notes else reasons


@contextmanager
def deadline(seconds):
    """Raise PostmortemDeadline in this thread once `seconds` of wall clock pass."""
    def expired(signum, frame):
        raise PostmortemDeadline()

    previous = signal.signal(signal.SIGALRM, expired)
    signal.setitimer(signal.ITIMER_REAL, seconds)
    try:
        yield
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        signal.signal(signal.SIGALRM, previous)


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

    def assessment(self, trace_id, key):
        for item in self.client.get_trace_info(trace_id).assessments:
            if (item.metadata or {}).get("idempotency_key") == key:
                return item.assessment_id
        return None

    def log(self, trace_id, name, value, rationale, judge_model, metadata):
        from mlflow.entities import AssessmentSource, Feedback
        from mlflow.environment_variables import MLFLOW_HTTP_REQUEST_MAX_RETRIES

        feedback = Feedback(name=name, value=value, rationale=rationale,
                            source=AssessmentSource(source_type="LLM_JUDGE", source_id=judge_model),
                            metadata=metadata)
        # MLflow preserves caller-supplied IDs and enforces their uniqueness.
        # Metadata lookup handles lag; the ID also prevents a late duplicate POST.
        feedback.assessment_id = metadata["idempotency_key"]
        # Keep one POST per attempt; checkpoint reconciliation owns retries.
        previous = MLFLOW_HTTP_REQUEST_MAX_RETRIES.get_raw()
        MLFLOW_HTTP_REQUEST_MAX_RETRIES.set(0)
        try:
            return self.client.log_assessment(trace_id, feedback).assessment_id
        finally:
            if previous is None:
                MLFLOW_HTTP_REQUEST_MAX_RETRIES.unset()
            else:
                MLFLOW_HTTP_REQUEST_MAX_RETRIES.set(previous)


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
    return read_json(path.read_text()) if path.exists() else None


def _save(path, value):
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(json.dumps(value, ensure_ascii=False))
    temporary.replace(path)


def _judge(run, stage):
    try:
        feedback = run()
        failure = judges.feedback_failure(feedback)
    except Exception as error:
        raise tracing.WorkerStageError(stage, judge_failure=judges.exception_failure(error)) from None
    if failure is not None:
        raise tracing.WorkerStageError(stage, judge_failure=failure) from None
    return feedback


def assessment_key(run, dimension, model, release):
    identity = json.dumps([run, dimension, model, release], separators=(",", ":"))
    # Match MLflow's a-<32 hex> ID shape and its 50-character database bound.
    return "a-" + hashlib.sha256(identity.encode()).hexdigest()[:32]


def _checked(inputs, reply):
    from tl3_workers.runtime import _checked as checked

    try:
        return checked(ROLE, inputs, reply)
    except ValidationError as error:
        if "recommendations" in error.absolute_schema_path:
            raise tracing.WorkerStageError(
                "postmortem recommendation contract failure", error) from None
        raise
    except ValueError:
        raise tracing.WorkerStageError("postmortem recommendation contract failure") from None


def _cached(path, inputs, envelope=None):
    try:
        cached = _load(path)
        if cached is None:
            return None
        # Older judged checkpoints held only the postmortem and grades.
        if envelope is not None and set(cached) == {"postmortem", "grades"}:
            cached = envelope(cached["postmortem"], cached["grades"])
        return _checked(inputs, cached)
    except (ValueError, ValidationError, KeyError, TypeError) as error:
        stage = "postmortem cached reply failure" if path.name == "reply.json" else (
            "postmortem cached judged failure")
        raise tracing.WorkerStageError(stage, error) from None


def run(model, request, worktree, emit):
    del worktree
    workdir = Path(request["worktree"])
    # Cached artifacts and all external calls share the whole-run deadline.
    with deadline(request["inputs"]["deadline_secs"]):
        if (cached := _cached(workdir / "reply.json", request["inputs"])) is not None:
            return cached
        return _judged_run(model, request, workdir, emit)


def _judged_run(model, request, workdir, emit):
    inputs = request["inputs"]
    redactor = redactor_for(request)
    source = SOURCE(tracing.mlflow_settings(request))
    traces = source.find(inputs["trace_tags"], MAX_TRACES)
    postmortem_model = request["model"]["name"]
    grade_model = inputs["grade_model"]

    def envelope(written, grades):
        return {"postmortem": written,
                "grades": [{"assessment_id": None, **grade} for grade in grades],
                "assessment_ids": [], "usage": usage(traces),
                "models": {"postmortem": postmortem_model, "grades": grade_model}}

    judged = _cached(workdir / "judged.json", inputs, envelope)
    if judged is None:
        judges.configure(request["model"]["base_url"], tracing.model_key.get())
        trace = case_trace(traces, redactor, inputs["record"]["id"])
        case = inputs["case"]

        def progress():
            emit({"event": "progress", "unit": "model_turn"})

        grades = []
        with judges.guarded_tools(redactor, progress):
            for dimension, grader in judges.grade_judges(inputs["rubric"], grade_model).items():
                feedback = _judge(lambda: grader(inputs=case, trace=trace), "grade judge failure")
                value = feedback.value
                grade = value if type(value) is int and 1 <= value <= 5 else None
                grades.append({"dimension": dimension, "grade": grade,
                               "rationale": redactor.text(feedback.rationale or "", 4000)[:4000],
                               "judge_model": grade_model})
            prompt = run_prompt()
            with judges.postmortem_turns():
                feedback = _judge(lambda: judges.postmortem_judge(prompt, postmortem_model)(
                    inputs=case, trace=trace), "postmortem judge failure")

        raw = feedback.value
        history = [SystemMessage(content=judges.postmortem_contract(prompt)),
                   HumanMessage(content=json.dumps(case, ensure_ascii=False)),
                   AIMessage(content=json.dumps(scrub(raw, redactor))
                             if isinstance(raw, dict) else redactor.text(str(raw)))]
        control = TurnControl()

        def ask():
            if not control.repairing:
                return raw
            try:
                # No tools or new judge loop: one turn reserved from MAX_TURNS.
                message = model.invoke(history)
            except Exception:
                raise tracing.WorkerStageError("postmortem repair failure") from None
            progress()
            return message.text

        def check(value):
            try:
                written = value if isinstance(value, dict) else read_reply(value)
            except (ValueError, TypeError):
                raise tracing.WorkerStageError("postmortem JSON parse failure") from None
            written = scrub(written, redactor)
            result = _checked(inputs, envelope(written, grades))
            try:
                written["recommendations"], dropped = applicable(
                    written["recommendations"], RELEASE_ROOT)
            except Exception:
                raise tracing.WorkerStageError(
                    "postmortem recommendation contract failure") from None
            written["notes"] = with_drops(written["notes"], dropped)
            return _checked(inputs, result)

        judged = repaired(control, history, ask, check)
        _save(workdir / "judged.json", judged)

    # Every success checkpoint and assessment follows full schema and role validation.
    _checked(inputs, judged)
    try:
        logged = _load(workdir / "assessments.json") or {}
        pending = _load(workdir / "assessment-pending.json") or {}
        if not isinstance(pending, dict) or not all(
                isinstance(key, str) and isinstance(value, str) for key, value in pending.items()):
            raise ValueError()
        for grade in judged["grades"]:
            grade["assessment_id"] = logged.get(grade["dimension"])
        judged["assessment_ids"] = [logged[key] for key in sorted(logged)]
        _checked(inputs, judged)
    except (ValueError, ValidationError, AttributeError, TypeError):
        raise tracing.WorkerStageError("postmortem assessment cache failure") from None
    target = traces[0].info.trace_id if traces else None
    tags = inputs["trace_tags"]
    run_key = hashlib.sha256(str(workdir).encode()).hexdigest()
    for grade in judged["grades"]:
        dimension = grade["dimension"]
        key = assessment_key(run_key, dimension, grade["judge_model"], inputs["release"])
        assessment_target = pending.get(dimension, target)
        metadata = {**{key: str(tags[key]) for key in ("node", "request", "repo", "issue")},
                    "release": inputs["release"], "rubric_version": str(inputs["rubric"]["version"]),
                    "dimension": dimension, "judge_model": grade["judge_model"],
                    "postmortem_run": run_key, "idempotency_key": key}
        if dimension not in logged and dimension in pending:
            # The intent may precede the POST, or the POST's response may be lost.
            # Query twice, allowing a bounded read-after-write delay, before retrying
            # the same server-enforced ID on the intent's original trace.
            try:
                recovered = source.assessment(assessment_target, key)
                if recovered is None:
                    time.sleep(ASSESSMENT_SETTLE_SECS)
                    recovered = source.assessment(assessment_target, key)
            except Exception:
                raise tracing.WorkerStageError("postmortem assessment lookup failure") from None
            if recovered is not None:
                logged[dimension] = recovered
        if assessment_target and grade["grade"] is not None and dimension not in logged:
            pending[dimension] = assessment_target
            _save(workdir / "assessment-pending.json", pending)
            try:
                logged[dimension] = source.log(
                    assessment_target, dimension, grade["grade"], grade["rationale"],
                    grade["judge_model"], metadata)
            except Exception:
                raise tracing.WorkerStageError("postmortem assessment write failure") from None
        grade["assessment_id"] = logged.get(dimension)
        judged["assessment_ids"] = [logged[key] for key in sorted(logged)]
        _checked(inputs, judged)
        _save(workdir / "assessments.json", logged)
        pending.pop(dimension, None)
        _save(workdir / "assessment-pending.json", pending)
    _checked(inputs, judged)
    _save(workdir / "reply.json", judged)
    return judged


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
    validates_reply=True,
    prepare_inputs=redact_case,
)
