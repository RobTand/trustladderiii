"""Draft questions, course-change notes and closing summaries for Rob."""

from tl3_workers.roles.base import Role
from tl3_workers import tracing


def redact_findings(inputs, request):
    """Worker findings reach the model only through the MLflow redaction (#302, #322)."""
    facts = inputs.get("facts", {})
    if not any(field in facts for field in ("findings", "evidence")):
        return inputs
    redactor = tracing.Redactor(request.get("key_file"))
    if key := tracing.model_key.get():
        # The key create_model loaded for this run, whether or not tracing is on.
        redactor.secrets.add(key)
    return {**inputs, "facts": {
        field: redactor.payload(value) if field in {"findings", "evidence"} else value
        for field, value in facts.items()
    }}


ROLE = Role(
    name="writer",
    prompt="writer.md",
    input_schema="writer-input",
    reply_schema="writer-reply",
    tools={},
    prepare_inputs=redact_findings,
)
