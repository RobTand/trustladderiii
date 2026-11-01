"""Independently audit an ingest reply against the supplied issue."""

from tl3_workers.roles.base import Role
from tl3_workers.exploration import ExplorationBudget


ROLE = Role(
    name="audit",
    prompt="audit.md",
    input_schema="audit-input",
    reply_schema="audit-reply",
    exploration_budget=ExplorationBudget(),
    tools={
        "list": "List repository directories to locate evidence for the request.",
        "read": "Read repository text to check the request against its implementation.",
        "glob": "Find repository paths using relative glob patterns.",
        "grep": "Find literal references in repository text to check the request.",
    },
)
