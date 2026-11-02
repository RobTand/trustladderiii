"""Turn supplied issue material into a brief or an early end."""

from tl3_workers.roles.base import Role
from tl3_workers.contracts import schema
from tl3_workers.exploration import ExplorationBudget

# The brief's maxLength, which the controller validates.
BRIEF_CHARACTERS = next(
    variant for variant in schema("ingest-reply")["oneOf"]
    if variant["properties"]["kind"].get("const") == "brief"
)["properties"]["brief"]["maxLength"]


def check_memory_ids(inputs, reply):
    if "forecast" in reply:
        raise ValueError("only code may supply a forecast")
    if reply["kind"] == "brief":
        supplied = {memory["id"] for memory in inputs.get("memories", [])}
        if not set(reply["memory_ids"]) <= supplied:
            raise ValueError("reply cites a memory it was not given")

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
    code_only_fields=("forecast",),
    exploration_budget=ExplorationBudget(),
    tools={
        "list": "List repository directories to locate evidence for the request.",
        "read": "Read repository text to check the request against its implementation.",
        "glob": "Find repository paths using relative glob patterns. The path must be a directory.",
        "grep": "Find literal references in repository text to check the request. The path may be a file or a directory, and the pattern is literal text, not a regular expression.",
    },
    check_reply=check_memory_ids,
)
