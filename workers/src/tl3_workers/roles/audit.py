"""Independently audit an ingest reply against the supplied issue."""

from tl3_workers.roles.base import Role


ROLE = Role(
    name="audit",
    prompt="audit.md",
    input_schema="audit-input",
    reply_schema="audit-reply",
    tools={},
)
