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


def input_estimate(request):
    # Match the session runners' fallback: three UTF-8 bytes per token, rounded up.
    # Include tool schemas and framing when provider usage is unavailable.
    messages = ([request.system_message] if request.system_message else []) + request.messages
    payload = {"messages": [m.model_dump(include={"type", "content", "tool_calls", "tool_call_id", "name"})
                            for m in messages],
               "tools": [convert_to_openai_tool(tool) for tool in request.tools]}
    size = len(json.dumps(payload, ensure_ascii=False).encode("utf-8")) + 32 * len(messages)
    return (size + 2) // 3


def _excerpt(text, allowance):
    """Clip once by the escaped UTF-8 size the JSON request actually sends."""
    used = 0
    for index, char in enumerate(text):
        cost = (2 if char in '\"\\\b\f\n\r\t' else
                6 if ord(char) < 32 else len(char.encode("utf-8")))
        if used + cost > allowance:
            return text[:index]
        used += cost
    return text


def _clip_group(group, available, arguments=False):
    def resize(message, allowance):
        changes = {"content": _excerpt(message.text, allowance)}
        if isinstance(message, AIMessage) and arguments:
            changes["tool_calls"] = [
                {**call, "args": {"tl3_clipped_arguments": _excerpt(
                    json.dumps(call["args"], ensure_ascii=False), allowance)}}
                for call in message.tool_calls]
            # Some providers also preserve the unparsed call in additional_kwargs.
            changes["additional_kwargs"] = {key: value for key, value in message.additional_kwargs.items()
                                            if key != "tool_calls"}
        return message.model_copy(update=changes)

    overhead = sum(len(json.dumps(resize(m, 0).model_dump(), ensure_ascii=False).encode()) for m in group)
    fields = len(group) + (sum(len(m.tool_calls) for m in group if isinstance(m, AIMessage))
                           if arguments else 0)
    each = max(0, available - overhead) // fields
    return [resize(m, each) for m in group]


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
        if overhead > available and kept:
            break
        size = sum(len(json.dumps(m.model_dump(), ensure_ascii=False).encode()) for m in group)
        if size > available:
            clipped = True
            # The newest edit must survive even when its arguments fill the window.
            group = _clip_group(group, available, arguments=overhead > available)
            size = sum(len(json.dumps(m.model_dump(), ensure_ascii=False).encode()) for m in group)
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
    def __init__(self, budget, role):
        self.budget = budget
        self.role = role
        self.last_input = 0
        self.turns = 0
        self.spent = 0
        self.final = False

    def wrap_model_call(self, request, handler):
        messages = retained(request.messages, self.budget.history_bytes)
        request = request.override(messages=messages)
        bound = input_estimate(request)
        # Reserve the original input plus maximum retained evidence before exploring.
        original = [m for m in messages if isinstance(m, HumanMessage)]
        final_reserve = input_estimate(request.override(messages=original, tools=[]))
        final_reserve += (self.budget.history_bytes * 2 + len(FINISH) + 1024 + 2) // 3
        # A provider may tokenize above the fallback estimate. Carry that evidence
        # into the next-call and final-answer reservations as well as the charge.
        final_reserve = max(final_reserve, self.last_input)
        final = (self.turns >= self.budget.tool_turns
                 or self.spent + max(bound, self.last_input) + final_reserve > self.budget.input_tokens)
        if self.final:
            raise ValueError("missing final model reply")
        if final:
            self.final = True
            request = request.override(tools=[], tool_choice=None,
                                       messages=messages + [HumanMessage(content=FINISH)])
            bound = input_estimate(request)
        if self.spent + max(bound, self.last_input) > self.budget.input_tokens:
            raise ValueError(f"{self.role} input exceeds exploration budget")
        response = handler(request)
        if final:
            for message in response.result:
                if isinstance(message, AIMessage):
                    message.response_metadata["tl3_budget_exhausted"] = True
        reported = [(m.usage_metadata or {}).get("input_tokens")
                    for m in response.result if isinstance(m, AIMessage)]
        self.last_input = (sum(reported) if reported and all(type(n) is int and n >= 0 for n in reported)
                           else bound)
        self.spent += self.last_input
        self.turns += 1
        return response
