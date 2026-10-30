"""Classify one comment against the request and its open question."""

from tl3_workers.roles.base import Role


def check_reply(inputs, reply):
    state = inputs["state"]
    if state["closed"] and reply["kind"] not in {"complaint", "remark", "unsure"}:
        raise ValueError("closed requests allow only complaint, remark or unsure")
    if reply["kind"] == "answer":
        question = state.get("open_question")
        if question is None or reply["question_id"] != question["id"]:
            raise ValueError("reply answers a question that is not open")
        if reply["answer"] != "unclear" and reply["answer"] not in question["answers"]:
            raise ValueError("reply selects an answer the question does not offer")
        # JSON Schema treats 1.0 as an integer; credit arithmetic requires an int.
        if reply["answer"] == "add_credit" and type(reply["amount"]) is not int:
            raise ValueError("credit amount must be an integer")
    if reply["kind"] == "complaint" and not state["closed"]:
        raise ValueError("complaints require a closed request")
    if reply["kind"] == "change_request" or (
        reply["kind"] == "answer" and reply["answer"] == "change_request"
    ):
        if not reply["new_text"].strip():
            raise ValueError("a change requires new request text")


ROLE = Role(
    name="classifier",
    prompt="classifier.md",
    input_schema="classifier-input",
    reply_schema="classifier-reply",
    tools={},
    check_reply=check_reply,
)
