"""Turn supplied issue material into a brief or an early end."""

from tl3_workers.roles.base import Role


def check_memory_ids(inputs, reply):
    if reply["kind"] == "brief":
        supplied = {memory["id"] for memory in inputs.get("memories", [])}
        if not set(reply["memory_ids"]) <= supplied:
            raise ValueError("reply cites a memory it was not given")


ROLE = Role(
    name="ingest",
    prompt="ingest.md",
    input_schema="ingest-input",
    reply_schema="ingest-reply",
    tools={},
    check_reply=check_memory_ids,
)
