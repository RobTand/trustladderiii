"""Draft questions, course-change notes and closing summaries for Rob."""

from tl3_workers.roles.base import Role


ROLE = Role(
    name="writer",
    prompt="writer.md",
    input_schema="writer-input",
    reply_schema="writer-reply",
    tools={},
)
