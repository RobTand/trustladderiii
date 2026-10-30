"""Draft questions, course-change notes and closing summaries for Rob."""

from tl3_workers.roles.base import Role


def check_reply(inputs, reply):
    if inputs["kind"] == "question":
        if "text" in reply:
            raise ValueError("a question requires a draft")
        if reply["recommended_option"] >= len(reply["options"]):
            raise ValueError("recommended option is out of range")
        if reply["request_quote"] != inputs["facts"]["issue_title"]:
            raise ValueError("request quote must match the issue title")
    elif "text" not in reply:
        raise ValueError("a note or summary requires text")


ROLE = Role(
    name="writer",
    prompt="writer.md",
    input_schema="writer-input",
    reply_schema="writer-reply",
    tools={},
    check_reply=check_reply,
)
