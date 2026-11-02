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


# The newest runs are judged; older ones still count toward usage.
MAX_TRACES = 24
MAX_RECOMMENDATIONS = 3
# Trace attributes that hold structure rather than content.
STRUCTURAL = {"mlflow.spanType", "mlflow.traceRequestId", "mlflow.chat.tokenUsage",
              "mlflow.llm.model", "tl3.role", "tl3.proxy.usage_chunks",
              "tl3.proxy.usage_chunks_truncated", "tl3.trace.truncated",
              "tl3.trace.omitted_spans", "tl3.source_trace"}
USAGE = ("input_tokens", "output_tokens", "cache_read_input_tokens",
         "cache_creation_input_tokens")


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
    raise NotImplementedError("#403")


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
    raise NotImplementedError("#403")


def usage(traces):
    """Tokens from the runs' trace tags, which tracing records per run."""
    raise NotImplementedError("#403")


def check_reply(inputs, reply):
    raise NotImplementedError("#403")


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
    raise NotImplementedError("#403")


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
