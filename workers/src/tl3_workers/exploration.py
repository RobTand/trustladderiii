"""Per-run exploration limits, including room for a final answer."""

from dataclasses import dataclass
import json
import math
import os

from langchain.agents.middleware import AgentMiddleware
from langchain_core.messages import AIMessage, HumanMessage, messages_from_dict, messages_to_dict
from langchain_core.utils.function_calling import convert_to_openai_tool

from tl3_workers.turns import RepairUnavailable


@dataclass(frozen=True)
class ExplorationBudget:
    tool_turns: int = 4
    input_tokens: int = 300_000
    history_bytes: int = 32_768


@dataclass(frozen=True)
class CacheWeights:
    uncached: float = 1.0
    creation: float = 1.25
    read: float = 0.1

    @classmethod
    def configured(cls):
        values = {name: float(os.environ.get("TL3_CACHE_" + name.upper() + "_WEIGHT", default))
                  for name, default in (("uncached", 1), ("creation", 1.25), ("read", 0.1))}
        if any(not math.isfinite(value) or value < 0 for value in values.values()):
            raise ValueError("cache weights must be finite and nonnegative")
        return cls(**values)

    def charge(self, tokens, details=None):
        details = details or {}
        counts = [details.get(name, 0) for name in ("cache_read", "cache_creation")]
        read, creation = [count if type(count) is int and count >= 0 else 0 for count in counts]
        if read + creation > tokens:
            read = creation = 0
        return math.ceil((tokens - read - creation) * self.uncached
                         + creation * self.creation + read * self.read)


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
    def __init__(self, budget, role, context_size=150_000):
        self.budget = budget
        self.role = role
        self.context_size = context_size
        self.weights = CacheWeights.configured()
        self.last_input = 0
        self.last_estimate = None
        self.cache_active = False
        self.turns = 0
        self.spent = 0
        self.in_flight = 0
        self.in_flight_reserve = 0
        self.repairing = False
        self.final = False
        self.compacted = False
        # Only a compaction that freed room has a later threshold to approach again.
        self.compacted_below = False
        self.compaction_prefix = []
        self.compaction_offset = 0

    def _context(self, estimate):
        # Calibrate byte estimates to the latest provider count, then add new text.
        return (estimate if self.last_estimate is None else
                max(0, self.last_input + estimate - self.last_estimate))

    def wrap_model_call(self, request, handler):
        source = request.messages
        messages = (self.compaction_prefix + source[self.compaction_offset:]
                    if self.compacted else list(source))
        request = request.override(messages=messages)
        bound = input_estimate(request)
        context = self._context(bound)
        compact_now = (not self.compacted and context >= self.context_size * 0.7
                       and any(isinstance(message, AIMessage) for message in messages))
        if compact_now:
            messages = retained(messages, self.budget.history_bytes)
            self.compaction_prefix = messages
            self.compaction_offset = len(source)
            self.compacted = True
            request = request.override(messages=messages)
            bound = input_estimate(request)
            context = self._context(bound)
            self.compacted_below = context < self.context_size * 0.7

        cold_weight = (max(self.weights.uncached, self.weights.creation)
                       if self.cache_active else self.weights.uncached)
        if self.cache_active and not compact_now:
            cached = min(context, self.last_input)
            predicted = math.ceil(cached * self.weights.read + (context - cached) * cold_weight)
        else:
            predicted = math.ceil(context * cold_weight)
        self.in_flight, self.in_flight_reserve = predicted, 0
        if self.repairing:
            # A repair turn spends the same allowance, without the final-answer reserve.
            if self.spent + predicted > self.budget.input_tokens:
                raise RepairUnavailable()
            return self._account(handler(request), bound)
        original = [m for m in messages if isinstance(m, HumanMessage)]
        reserve = input_estimate(request.override(messages=original, tools=[]))
        reserve += (self.budget.history_bytes * 2 + len(FINISH) + 1024 + 2) // 3
        # A compaction may invalidate cached evidence. Reserve a cold final call.
        reserve = math.ceil(max(reserve, context) * cold_weight)
        context_full = (self.compacted_below and not compact_now and len(source) > self.compaction_offset
                        and context >= self.context_size * 0.7)
        final = (self.turns >= self.budget.tool_turns or context_full
                 or self.spent + predicted + reserve > self.budget.input_tokens)
        self.in_flight_reserve = 0 if final else reserve
        if self.final:
            raise ValueError("missing final model reply")
        if final:
            self.final = True
            request = request.override(tools=[], tool_choice=None,
                                       messages=messages + [HumanMessage(content=FINISH)])
            bound = input_estimate(request)
        if self.spent + predicted > self.budget.input_tokens:
            raise ValueError(f"{self.role} input exceeds exploration budget")
        response = handler(request)
        if final:
            for message in response.result:
                if isinstance(message, AIMessage):
                    message.response_metadata["tl3_budget_exhausted"] = True
        return self._account(response, bound)

    def charge_dropped(self):
        """Charge a dropped call, then report whether its retry still fits with its reserve."""
        self.spent += self.in_flight
        return self.spent + self.in_flight + self.in_flight_reserve <= self.budget.input_tokens

    # A resumed visit continues with the same allowance, compaction and cache pricing.
    # The final-answer flag is not kept: a resumed run recomputes it from this state.
    def state(self):
        return {"spent": self.spent, "turns": self.turns, "last_input": self.last_input,
                "last_estimate": self.last_estimate, "cache_active": self.cache_active,
                "compacted": self.compacted, "compacted_below": self.compacted_below,
                "compaction_offset": self.compaction_offset,
                "compaction_prefix": messages_to_dict(self.compaction_prefix)}

    def restore(self, state):
        prefix = messages_from_dict(state["compaction_prefix"])
        numbers = [state[name] for name in ("spent", "turns", "last_input", "compaction_offset")]
        flags = [state[name] for name in ("cache_active", "compacted", "compacted_below")]
        if (any(type(n) is not int or n < 0 for n in numbers) or any(type(f) is not bool for f in flags)
                or not (state["last_estimate"] is None or type(state["last_estimate"]) is int)):
            raise ValueError("invalid allowance state")
        (self.spent, self.turns, self.last_input, self.compaction_offset) = numbers
        (self.cache_active, self.compacted, self.compacted_below) = flags
        self.last_estimate = state["last_estimate"]
        self.compaction_prefix = prefix

    def _account(self, response, bound):
        usage = [message.usage_metadata for message in response.result if isinstance(message, AIMessage)]
        reported = bool(usage) and all(item and type(item.get("input_tokens")) is int
                                       and item["input_tokens"] >= 0 for item in usage)
        self.last_input = sum(item["input_tokens"] for item in usage) if reported else bound
        self.spent += (sum(self.weights.charge(item["input_tokens"], item.get("input_token_details"))
                           for item in usage) if reported else self.weights.charge(bound))
        self.cache_active = reported and any(
            (item.get("input_token_details") or {}).get("cache_read", 0)
            or (item.get("input_token_details") or {}).get("cache_creation", 0) for item in usage)
        self.last_estimate = bound
        self.turns += 1
        return response
