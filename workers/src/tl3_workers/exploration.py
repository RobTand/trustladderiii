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
          "schema. State uncertainty and unfinished work in the reply. Never claim a goal "
          "was met without supporting evidence.")


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
    for index, message in enumerate(messages[first + 1:], first + 1):
        if isinstance(message, HumanMessage):
            continue
        if isinstance(message, AIMessage):
            groups.append([(index, message)])
        elif groups:
            groups[-1].append((index, message))
    kept, used, clipped = {}, 0, False
    for group in reversed(groups):
        indexes, group = zip(*group)
        available = limit - used
        overhead = sum(len(json.dumps(m.model_copy(update={"content": ""}).model_dump(),
                                      ensure_ascii=False).encode()) for m in group)
        if overhead > available:
            break
        size = sum(len(json.dumps(m.model_dump(), ensure_ascii=False).encode()) for m in group)
        if size > available:
            clipped = True
            each = max(0, (available - overhead) // len(group) - 32)
            original = group
            while size > available:
                group = [m.model_copy(update={"content": m.text.encode("utf-8")[:each].decode("utf-8", errors="ignore")})
                         for m in original]
                size = sum(len(json.dumps(m.model_dump(), ensure_ascii=False).encode()) for m in group)
                each //= 2
        kept.update(zip(indexes, group))
        used += size
    if clipped or len(kept) < sum(len(group) for group in groups):
        prefix = prefix + [HumanMessage(content="Earlier exploration text was removed or clipped to bound context. "
                                       "Use the retained evidence.")]
    # Resumed session inputs and no-change feedback are instructions, not stale evidence.
    return prefix + [kept.get(index, message)
                     for index, message in enumerate(messages[first + 1:], first + 1)
                     if index in kept or isinstance(message, HumanMessage)]


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
        # Reserve the original input plus maximum retained evidence before exploring.
        original = [m for m in messages if isinstance(m, HumanMessage)]
        final_reserve = input_bound(request.override(messages=original, tools=[]))
        final_reserve += self.budget.history_bytes * 2 + len(FINISH) + 1024
        final = (self.turns >= self.budget.tool_turns
                 or self.spent + bound + final_reserve > self.budget.input_tokens)
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
        if final:
            for message in response.result:
                if isinstance(message, AIMessage):
                    message.response_metadata["tl3_budget_exhausted"] = True
        reported = sum((m.usage_metadata or {}).get("input_tokens", 0)
                       for m in response.result if isinstance(m, AIMessage))
        self.spent += max(bound, reported)
        self.turns += 1
        return response
