"""Review an exact head against goals and repository standing rules."""

import json
from pathlib import Path

from langchain_core.messages import AIMessage, HumanMessage, ToolMessage
from langchain_core.messages import messages_from_dict, messages_to_dict
from langgraph.graph import END, START, MessagesState, StateGraph
from langgraph.prebuilt import ToolNode
from jsonschema import ValidationError
from langchain_openai import ChatOpenAI

from tl3_workers.contracts import read_json, read_reply, validate
from tl3_workers.roles.base import Role
from tl3_workers.session import ContextCharacters, is_unaccepted_session, out_of_room, save
from tl3_workers.tools import build_tools
from tl3_workers.turns import TurnControl, repaired


def check_reply(inputs, reply):
    if reply["verdict"] == "approve" and reply["head_sha"] != inputs["head_sha"]:
        raise ValueError("approval names a head other than the reviewed head")
    if reply["verdict"] == "out_of_room":
        return
    known = {
        "goal": {goal["id"] for goal in inputs["goals"]},
        "standing_rule": {rule["id"] for rule in inputs["standing_rules"]},
    }
    for finding in reply["blocking_findings"]:
        citation = finding["citation"]
        if citation["id"] not in known[citation["kind"]]:
            raise ValueError("blocking finding cites an unknown goal or standing rule")
        if citation == {"kind": "standing_rule", "id": "request-scope"}:
            changed = (line for line in inputs.get("scope_diff", inputs["diff"]).splitlines()
                       if line.startswith(("+", "-"))
                       and not line.startswith(("+++", "---"))
                       and sum(not char.isspace() for char in line[1:]) >= 8)
            quoted = {line.strip() for line in finding["evidence"].splitlines()}
            if not any(line in quoted for line in changed):
                raise ValueError("scope blocker must quote a changed hunk")


def _pending_calls(history):
    pending = {}
    for message in history:
        if isinstance(message, AIMessage):
            pending.update((call["id"], call) for call in message.tool_calls)
        elif isinstance(message, ToolMessage):
            pending.pop(message.tool_call_id, None)
    return list(pending.values())


def run(model, request, worktree, emit):
    from tl3_workers.runtime import build_agent

    if isinstance(model, ChatOpenAI):
        model.stream_usage = True
    control = TurnControl()
    agent = build_agent(model, ROLE, worktree, control=control)
    characters = ContextCharacters()
    session = Path(request["session_file"]) if "session_file" in request else None
    saved = read_json(session.read_text()) if session and session.exists() else None
    retry = is_unaccepted_session(request, saved)
    if retry and saved["inputs"] != request["inputs"]:
        raise ValueError("review retry evidence changed before acceptance")
    if retry and saved.get("out_of_room"):
        return {"verdict": "out_of_room"}
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
    if session:
        save(session, state)
    if calls := _pending_calls(history):
        # Resume only the pending reads, without asking the model for another turn.
        graph = StateGraph(MessagesState)
        graph.add_node("tools", ToolNode(build_tools(worktree, ROLE.tools)))
        graph.add_edge(START, "tools")
        graph.add_edge("tools", END)
        result = graph.compile().invoke({"messages": [AIMessage(content="", tool_calls=calls)]})
        results = [message for message in result["messages"] if isinstance(message, ToolMessage)]
        history.extend(results)
        state["messages"] = messages_to_dict(history)
        if session:
            save(session, state)
        for message in results:
            emit({"event": "progress", "unit": "tool_result"})
    def ask():
        stream = agent.stream({"messages": list(history)}, config={"callbacks": [characters]},
                              stream_mode="updates")
        last_message = None
        try:
            for update in stream:
                messages = [message for result in update.values() if isinstance(result, dict)
                            for message in result.get("messages", [])]
                history.extend(messages)
                for message in messages:
                    if isinstance(message, AIMessage) and out_of_room(
                            message, request["inputs"].get("context_size", 150_000), characters.prompt):
                        state["out_of_room"] = True
                        state["messages"] = messages_to_dict(history)
                        if session:
                            save(session, state)
                        raise _OutOfRoom()
                    if isinstance(message, AIMessage) and session is None:
                        emit({"event": "progress", "unit": "model_turn"})
                    if isinstance(message, AIMessage) and not message.tool_calls:
                        if message.invalid_tool_calls:
                            raise ValueError("invalid reviewer tool calls")
                        last_message = message
                        # Save a valid reply with its turn, so a crash after it costs no model call.
                        try:
                            state["reply"] = checked[message.text] = check(message.text)
                        except (ValueError, ValidationError):
                            pass
                state["messages"] = messages_to_dict(history)
                if session:
                    save(session, state)
                for message in messages:
                    if isinstance(message, AIMessage) and session is not None:
                        emit({"event": "progress", "unit": "model_turn"})
                    elif isinstance(message, ToolMessage):
                        emit({"event": "progress", "unit": "tool_result"})
        finally:
            stream.close()
        if last_message is None:
            raise ValueError("missing final reviewer reply")
        return last_message.text

    def check(text):
        reply = read_reply(text)
        validate("reviewer-reply", reply)
        if reply["verdict"] == "out_of_room":
            raise ValueError("model cannot report reviewer exhaustion")
        check_reply(request["inputs"], reply)
        return reply

    checked = {}
    try:
        return repaired(control, history, ask, lambda text: checked.get(text) or check(text))
    except _OutOfRoom:
        return {"verdict": "out_of_room"}


class _OutOfRoom(Exception):
    pass


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
    validates_reply=True,
)
