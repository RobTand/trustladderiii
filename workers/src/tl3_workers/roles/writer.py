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
        raise ValueError("this message requires text")
    elif inputs["kind"] == "clarification":
        text = reply["text"]
        if (len(text) > 300 or any(c in text for c in "\r\n\u0085\u2028\u2029")
                or not text.rstrip().endswith("?") or text.count("?") != 1):
            raise ValueError("write one short question on one line")
        if '"' + inputs["facts"]["comment_excerpt"] + '"' not in text:
            raise ValueError("quote the supplied comment excerpt exactly")


ROLE = Role(
    name="writer",
    prompt="writer.md",
    input_schema="writer-input",
    reply_schema="writer-reply",
    tools={},
    check_reply=check_reply,
)
