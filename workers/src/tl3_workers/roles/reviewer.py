"""Review an exact head against goals and repository standing rules."""

import json
from pathlib import Path

from langchain_core.messages import AIMessage, HumanMessage, ToolMessage
from langchain_core.messages import messages_from_dict, messages_to_dict

from tl3_workers.contracts import read_json, validate
from tl3_workers.roles.base import Role
from tl3_workers.roles.maker import _save


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


def run(model, request, worktree, emit):
    from tl3_workers.runtime import build_agent, _run_agent

    agent = build_agent(model, ROLE, worktree)
    if "session_file" not in request:
        return _run_agent(agent, request, emit)
    session = Path(request["session_file"])
    saved = read_json(session.read_text()) if session.exists() else None
    retry = saved is not None and saved["generation"] > request.get("last_accepted_generation", -1)
    if retry and saved["inputs"] != request["inputs"]:
        raise ValueError("review retry evidence changed before acceptance")
    history = messages_from_dict(saved["messages"]) if saved and (
        retry or request.get("resume_session")) else []
    if retry and "reply" in saved:
        validate("reviewer-reply", saved["reply"])
        check_reply(request["inputs"], saved["reply"])
        return saved["reply"]
    if not retry:
        history.append(HumanMessage(content=json.dumps(request["inputs"], ensure_ascii=False)))
    state = {"generation": saved["generation"] if retry else request["operation"]["generation"],
             "inputs": request["inputs"], "messages": messages_to_dict(history)}
    _save(session, state)
    stream = agent.stream({"messages": history}, stream_mode="updates")
    reply = None
    try:
        for update in stream:
            messages = [message for result in update.values() if isinstance(result, dict)
                        for message in result.get("messages", [])]
            history.extend(messages)
            for message in messages:
                if isinstance(message, AIMessage) and not message.tool_calls:
                    if message.invalid_tool_calls:
                        raise ValueError("invalid reviewer tool calls")
                    reply = read_json(message.text)
                    validate("reviewer-reply", reply)
                    check_reply(request["inputs"], reply)
                    state["reply"] = reply
            # Only complete tool exchanges can be sent back to the model.
            pending = set()
            for message in history:
                if isinstance(message, AIMessage):
                    pending.update(call["id"] for call in message.tool_calls)
                elif isinstance(message, ToolMessage):
                    pending.discard(message.tool_call_id)
            if not pending:
                state["messages"] = messages_to_dict(history)
                _save(session, state)
            for message in messages:
                if isinstance(message, AIMessage):
                    emit({"event": "progress", "unit": "model_turn"})
                elif isinstance(message, ToolMessage):
                    emit({"event": "progress", "unit": "tool_result"})
    finally:
        stream.close()
    if reply is None:
        raise ValueError("missing final reviewer reply")
    return reply


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
    run=run,
)
