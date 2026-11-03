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


def agent_calls(calls):
    """Recorded (messages, ...) model calls without the maker's retro turn (#648).

    The retro follows the accepted reply and is tested on its own."""
    from tl3_workers.retro import PROMPT

    return [call for call in calls if not (call[0] and getattr(call[0][-1], "content", None) == PROMPT)]
