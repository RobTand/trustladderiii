"""Turn supplied issue material into a brief or an early end."""

from tl3_workers.roles.base import Role
from tl3_workers.prerequisites import check_prerequisites
from tl3_workers.evidence import preflight
from tl3_workers.contracts import schema
from tl3_workers.exploration import ExplorationBudget
from tl3_workers.generated_scope import check_ingest_generation_scope
from tl3_workers.recall import check_citations

# The brief's maxLength, which the controller validates.
BRIEF_CHARACTERS = next(
    variant for variant in schema("ingest-reply")["oneOf"]
    if variant["properties"]["kind"].get("const") == "brief"
)["properties"]["brief"]["maxLength"]


def check_memory_ids(inputs, reply):
    if "forecast" in reply:
        raise ValueError("only code may supply a forecast")
    preflight(inputs, reply)
    if reply["kind"] == "brief":
        check_prerequisites(inputs, reply)
        check_citations(inputs, reply)
        check_ingest_generation_scope(inputs, reply)

        if "forecast" in inputs:
            from tl3_workers.forecast import Forecaster
            if "files" not in reply:
                raise ValueError("sized briefs must declare their root files")
            forecaster = Forecaster(inputs["forecast"])
            file_tokens = forecaster.files(reply["files"])
            # Unusable files are named in the brief rather than failing the run
            # (#321): planning reports them as violations if they're still declared.
            for note in forecaster.notes():
                # The controller validates the brief against the schema's limit.
                if len(reply["brief"]) + 2 + len(note) > BRIEF_CHARACTERS:
                    break
                reply["brief"] += "\n\n" + note
            reply["forecast"] = forecaster.forecast(reply["brief"], file_tokens)


ROLE = Role(
    name="ingest",
    prompt="ingest.md",
    input_schema="ingest-input",
    reply_schema="ingest-reply",
    code_only_fields=("forecast", "evidence_errors"),
    environment_errors_end_run=True,
    exploration_budget=ExplorationBudget(),
    tools={
        "list": "List repository directories to locate evidence for the request.",
        "read": "Read repository text to check the request against its implementation.",
        "glob": "Find repository paths using relative glob patterns. The path must be a directory.",
        "grep": "Find references in repository text to check the request. The pattern is an RE2 regular expression, with no backreferences or lookarounds, matched per line; set literal to true to search text as written. The path may be a file or a directory.",
    },
    check_reply=check_memory_ids,
)
