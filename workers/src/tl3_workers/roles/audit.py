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
        "glob": "Find repository paths using relative glob patterns. The path must be a directory.",
        "grep": "Find references in repository text to check the request. The pattern is an RE2 regular expression, with no backreferences or lookarounds, matched per line; set literal to true to search text as written. The path may be a file or a directory.",
    },
)
