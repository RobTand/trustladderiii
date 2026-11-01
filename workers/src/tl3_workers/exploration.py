"""Per-run exploration limits, including room for a final answer."""

from dataclasses import dataclass
import json

from langchain.agents.middleware import AgentMiddleware
from langchain_core.messages import AIMessage, HumanMessage
from langchain_core.utils.function_calling import convert_to_openai_tool


@dataclass(frozen=True)
class ExplorationBudget:
    tool_turns: int = 4
    input_tokens: int = 300_000
    history_bytes: int = 32_768


FINISH = ("The exploration budget is exhausted. Tools are unavailable. Finish now from the "
          "supplied request and retained evidence. Return the final JSON matching your reply "
          "schema. State any uncertainty in the brief or audit objections.")


def input_bound(request):
    # A byte can encode at most one text token. Include tool schemas and message
    # framing rather than relying on a tokenizer from a different model family.
    messages = ([request.system_message] if request.system_message else []) + request.messages
    payload = {"messages": [m.model_dump(include={"type", "content", "tool_calls", "tool_call_id", "name"})
                            for m in messages],
               "tools": [convert_to_openai_tool(tool) for tool in request.tools]}
    return len(json.dumps(payload, ensure_ascii=False).encode("utf-8")) + 32 * len(messages)


def retained(messages, limit):
    """Keep complete recent call/result groups, with a bounded evidence payload."""
    first = next(i for i, message in enumerate(messages) if isinstance(message, HumanMessage))
    prefix = messages[:first + 1]
    groups = []
    for message in messages[first + 1:]:
        if isinstance(message, AIMessage):
            groups.append([message])
        elif groups:
            groups[-1].append(message)
    kept, used = [], 0
    for group in reversed(groups):
        size = sum(len(json.dumps(m.model_dump(), ensure_ascii=False).encode()) for m in group)
        if used + size > limit:
            break
        kept = group + kept
        used += size
    if len(kept) < len(messages) - len(prefix):
        prefix = prefix + [HumanMessage(content="Earlier exploration turns were removed to bound context. "
                                       "Use the retained evidence and finish within the budget.")]
    return prefix + kept


class ExplorationMiddleware(AgentMiddleware):
    def __init__(self, budget):
        self.budget = budget
        self.turns = 0
        self.spent = 0
        self.final = False

    def wrap_model_call(self, request, handler):
        messages = retained(request.messages, self.budget.history_bytes)
        request = request.override(messages=messages)
        bound = input_bound(request)
        # Reserve another context of this size for finalization before exploring.
        final = (self.turns >= self.budget.tool_turns
                 or self.spent + 2 * (bound + len(FINISH) + 128) > self.budget.input_tokens)
        if self.final:
            raise ValueError("missing final model reply")
        if final:
            self.final = True
            request = request.override(tools=[], tool_choice=None,
                                       messages=messages + [HumanMessage(content=FINISH)])
            bound = input_bound(request)
        if self.spent + bound > self.budget.input_tokens:
            raise ValueError("intake input exceeds exploration budget")
        response = handler(request)
        reported = sum((m.usage_metadata or {}).get("input_tokens", 0)
                       for m in response.result if isinstance(m, AIMessage))
        self.spent += max(bound, reported)
        self.turns += 1
        return response
