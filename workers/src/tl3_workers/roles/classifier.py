"""Classify one comment against the request and its open question."""

from tl3_workers.roles.base import Role


def check_reply(inputs, reply):
    if 'reconcile_issue' in inputs or reply['kind'] == 'reconcile_decisions':
        if 'reconcile_issue' not in inputs or reply['kind'] != 'reconcile_decisions':
            raise ValueError('restart must reconcile the current issue')
        active = {d['id'] for d in inputs['decision_context']['decisions']}
        ids = [a['id'] for a in reply['assessments']]
        if len(ids) != len(set(ids)) or set(ids) != active:
            raise ValueError('reconciliation must assess every applicable decision exactly once')
        return
    if any(reply.get(key) != value for key, value in inputs.get("selected_answer", {}).items()):
        raise ValueError("classifier changed a selected console answer")
    active = {decision["id"] for decision in inputs.get("decision_context", {}).get("decisions", [])
              if decision["superseded_by"] is None}
    if (reply["kind"] == "change_request" or reply.get("answer") == "change_request") and active and "supersedes" not in reply:
        raise ValueError("replacement must assess applicable decisions")
    if not set(reply.get("supersedes", [])) <= active:
        raise ValueError("supersedes names a decision that is not applicable")
    state = inputs["state"]
    if reply["kind"] == "restart":
        if state.get("restartable") is not True:
            raise ValueError("restart is not offered for this request")
    elif state["closed"] and reply["kind"] not in {"complaint", "remark", "unsure"}:
        raise ValueError("closed requests allow only complaint, remark, unsure or restart")
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
