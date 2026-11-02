"""Draft questions, course-change notes and closing summaries for Rob."""

from tl3_workers.roles.base import Role
from tl3_workers import tracing


def redact_findings(inputs, request):
    """Worker findings reach the model only through the MLflow redaction (#302, #322)."""
    facts = inputs.get("facts", {})
    if "findings" not in facts:
        return inputs
    redactor = tracing.Redactor(request.get("key_file"))
    if key := tracing.model_key.get():
        # The key create_model loaded for this run, whether or not tracing is on.
        redactor.secrets.add(key)
    return {**inputs, "facts": {**facts, "findings": redactor.payload(facts["findings"])}}


ROLE = Role(
    name="writer",
    prompt="writer.md",
    input_schema="writer-input",
    reply_schema="writer-reply",
    tools={},
    prepare_inputs=redact_findings,
)
