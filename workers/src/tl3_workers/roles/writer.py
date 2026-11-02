"""Draft questions, course-change notes and closing summaries for Rob."""

from tl3_workers.roles.base import Role
from tl3_workers import tracing


def redact_findings(inputs, request):
    """Worker findings reach the model only through the MLflow redaction (#302, #322)."""
    facts = inputs.get("facts", {})
    if "findings" not in facts:
        return inputs
    redactor = tracing.Redactor(request.get("key_file"))
    if (capture := tracing.active.get()) is not None:
        # Includes the model key that create_model loaded for this run.
        redactor.secrets |= capture.redactor.secrets
    return {**inputs, "facts": {**facts, "findings": redactor.payload(facts["findings"])}}


ROLE = Role(
    name="writer",
    prompt="writer.md",
    input_schema="writer-input",
    reply_schema="writer-reply",
    tools={},
    prepare_inputs=redact_findings,
)
