"""Review an exact head against goals and repository standing rules."""

from tl3_workers.roles.base import Role


def check_reply(inputs, reply):
    if reply["verdict"] == "approve" and reply["head_sha"] != inputs["head_sha"]:
        raise ValueError("approval names a head other than the reviewed head")
    known = {
        "goal": {goal["id"] for goal in inputs["goals"]},
        "standing_rule": {rule["id"] for rule in inputs["standing_rules"]},
    }
    for finding in reply["blocking_findings"]:
        citation = finding["citation"]
        if citation["id"] not in known[citation["kind"]]:
            raise ValueError("blocking finding cites an unknown goal or standing rule")


ROLE = Role(
    name="reviewer",
    prompt="reviewer.md",
    input_schema="reviewer-input",
    reply_schema="reviewer-reply",
    tools={
        "list": "List reviewed worktree directories to locate evidence for a goal or rule.",
        "read": "Read reviewed head files to check goals and standing rules. Follow page continuations.",
        "glob": "Find reviewed head paths using relative glob patterns.",
        "grep": "Find literal text in reviewed head files to investigate a goal or rule.",
    },
    check_reply=check_reply,
)
