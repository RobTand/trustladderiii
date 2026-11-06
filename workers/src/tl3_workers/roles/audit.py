"""Independently audit an ingest reply against the supplied issue."""

from tl3_workers.roles.base import Role
from tl3_workers.evidence import check_contract, preflight
from tl3_workers.ingest_retention import preserve_verbatim
from tl3_workers.generated_scope import audit_generation_scope
from tl3_workers.exploration import ExplorationBudget
from tl3_workers.contracts import schema


def ingest_reply_contract():
    """#863: what an ingest reply can express, generated from its reply contract."""
    definitions = schema("ingest-reply")["$defs"]
    return ("Ingest reply contract, generated from ingest-reply.json. A proof_obligation has only these fields: "
            + ", ".join(sorted(definitions["proof_obligation"]["properties"]))
            + ". An evidence_contract entry has only these fields: "
            + ", ".join(sorted(definitions["evidence_requirement"]["properties"])) + ".")


def check_reply(inputs, reply):
    check_contract(inputs, reply, role='audit')
    preflight(inputs, reply)
    if ("tool_capabilities" in inputs and inputs["ingest"]["kind"] == "brief"
            and reply["evidence_errors"]):
        reply["verdict"] = "disagree"
        reply["objections"] = list(dict.fromkeys([*reply["objections"], *reply["evidence_errors"]]))
    audit_generation_scope(inputs, reply)
    preserve_verbatim('audit', inputs, reply)


ROLE = Role(
    name="audit",
    check_reply=check_reply,
    code_only_fields=("evidence_errors",),
    prompt="audit.md",
    input_schema="audit-input",
    reply_schema="audit-reply",
    environment_errors_end_run=True,
    exploration_budget=ExplorationBudget(),
    tools={
        "list": "List repository directories to locate evidence for the request.",
        "read": "Read repository text to check the request against its implementation.",
        "glob": "Find repository paths using relative glob patterns. The path must be a directory.",
        "grep": "Find references in repository text to check the request. The pattern is an RE2 regular expression, with no backreferences or lookarounds, matched per line; set literal to true to search text as written. The path may be a file or a directory.",
    },
)
