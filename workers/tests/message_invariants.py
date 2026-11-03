"""The proxy requires complete, ordered tool-call/result pairs."""
from langchain_core.messages import AIMessage, ToolMessage


def assert_tool_pairs(messages):
    pending = set()
    for message in messages:
        if isinstance(message, AIMessage):
            assert not pending, f"missing tool results: {pending}"
            pending = {call["id"] for call in message.tool_calls}
            assert len(pending) == len(message.tool_calls), "duplicate tool call IDs"
        elif isinstance(message, ToolMessage):
            assert message.tool_call_id in pending, "tool result without preceding call"
            pending.remove(message.tool_call_id)
        else:
            assert not pending, "instruction interrupts pending tool results"
    assert not pending, f"missing tool results: {pending}"



def is_retro(call):
    """Whether a recorded model call is the maker's retro turn (#648).

    A call is a message list, a (messages, ...) tuple or a Chat Completions payload."""
    from tl3_workers.retro import PROMPT

    messages = call["messages"] if isinstance(call, dict) else call[0] if isinstance(call, tuple) else call
    last = messages[-1] if messages else None
    return (last.get("content") if isinstance(last, dict) else getattr(last, "content", None)) == PROMPT


def agent_calls(calls):
    """Recorded model calls without the retro, which follows the accepted reply and has its own tests."""
    return [call for call in calls if not is_retro(call)]
