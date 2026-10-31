"""Turn supplied issue material into a brief or an early end."""

from tl3_workers.roles.base import Role


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
            reply["forecast"] = Forecaster(inputs["forecast"]).measure(reply["brief"], reply["files"])


ROLE = Role(
    name="ingest",
    prompt="ingest.md",
    input_schema="ingest-input",
    reply_schema="ingest-reply",
    code_only_fields=("forecast",),
    tools={},
    check_reply=check_memory_ids,
)
