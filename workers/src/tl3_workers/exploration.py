"""Per-run exploration limits, including room for a final answer."""

from dataclasses import dataclass
import hashlib
import json
import math
import os
from threading import Lock

from langchain.agents.middleware import AgentMiddleware
from langchain_core.messages import AIMessage, HumanMessage, ToolMessage, messages_from_dict, messages_to_dict
from langchain_core.utils.function_calling import convert_to_openai_tool

from tl3_workers.budget_check import check as budget_check
from tl3_workers.turns import RepairUnavailable


@dataclass(frozen=True)
class ExplorationBudget:
    tool_turns: int = 4
    input_tokens: int = 300_000
    history_bytes: int = 32_768
    finalization_turns: int = 0
    no_progress_window: int = 0


FINALIZATION_TOOLS = frozenset({"list", "read", "glob", "grep", "diff", "run_tests"})
BUDGET_PREFIX = "TL3 execution budget: "


class InvalidAllowanceState(ValueError):
    """A visit checkpoint has unusable progress accounting."""


class BudgetUnavailable(Exception):
    """No further model call fits the existing allowance."""


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
        return budget_check("charge", tokens=tokens, read=read, creation=creation,
                            weights=[self.uncached, self.creation, self.read])


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
    size = len(json.dumps(payload, ensure_ascii=False).encode("utf-8"))
    return budget_check("estimate", bytes=size, messages=len(messages))


def retained(messages, limit):
    """Apply Rust's retention and clipping instructions to LangChain objects."""
    def size(message):
        return len(json.dumps(message.model_dump(), ensure_ascii=False).encode())

    facts = []
    for message in messages:
        # These are immutable LangChain serialization facts, with no external
        # reads or clocks. Python preserves providers' argument serialization.
        empty = message.model_copy(update={"content": ""})
        arguments = []
        clipped = empty
        if isinstance(message, AIMessage):
            arguments = [json.dumps(call["args"], ensure_ascii=False) for call in message.tool_calls]
            clipped = empty.model_copy(update={
                "tool_calls": [{**call, "args": {"tl3_clipped_arguments": ""}}
                               for call in message.tool_calls],
                "additional_kwargs": {key: value for key, value in message.additional_kwargs.items()
                                      if key != "tool_calls"}})
        facts.append({"kind": "human" if isinstance(message, HumanMessage) else
                      "ai" if isinstance(message, AIMessage) else "other",
                      "size": size(message), "overhead": size(empty),
                      "arguments_overhead": size(clipped), "text": message.text,
                      "arguments": arguments})
    decision = budget_check("retained", messages=facts, limit=limit)
    result = list(messages[:decision["prefix_end"]])
    if decision["notice"]:
        result.append(HumanMessage(content="Earlier exploration text was removed or clipped to bound context. "
                                   "Use the retained evidence."))
    for instruction in decision["messages"]:
        message = messages[instruction["index"]]
        clip = instruction["clip"]
        if clip is not None:
            changes = {"content": clip["content"]}
            if clip["arguments"] is not None:
                changes["tool_calls"] = [
                    {**call, "args": {"tl3_clipped_arguments": argument}}
                    for call, argument in zip(message.tool_calls, clip["arguments"], strict=True)]
                changes["additional_kwargs"] = {key: value for key, value in message.additional_kwargs.items()
                                                if key != "tool_calls"}
            message = message.model_copy(update=changes)
        result.append(message)
    return result


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
        self.no_progress_turns = 0
        self.last_progress_kind = None
        self.seen_calls = set()
        self.seen_content = set()
        self.seen_tests = set()
        self._progress_lock = Lock()
        self.spent = 0
        self.in_flight = 0
        self.in_flight_reserve = 0
        self.repairing = False
        self.final = False
        self.exhaustion = None
        self.finalizing = False
        self.implementation_files = frozenset()
        self.implementation_started = False
        self.compacted = False
        self.compaction_count = 0
        self.context_fraction = 0.0
        # Only a compaction that freed room has a later threshold to approach again.
        self.compacted_below = False
        self.compaction_prefix = []
        self.compaction_offset = 0
        self._tool_policy_key = None
        self._tool_policy_value = None

    def _context(self, estimate):
        # Calibrate byte estimates to the latest provider count, then add new text.
        return budget_check("context", estimate=estimate, last_input=self.last_input,
                            last_estimate=self.last_estimate)

    def _tool_rule(self, name):
        """#780: no subprocess per tool for permission or progress.

        Rust batches both decisions for every result shape once per flag change.
        The model boundary prepares the policy before dispatch. Each subsequent
        tool event only applies it; the maker's durable checkpoint uses at most
        one history subprocess per streamed update or PB-submission callback.
        """
        key = (self.role == "maker", self.budget.no_progress_window,
               self.finalizing, self.final, self.repairing)
        if key != self._tool_policy_key:
            maker, window, finalizing, final, repairing = key
            policy = budget_check("tools", maker=maker, window=window, finalizing=finalizing,
                                  final=final, repairing=repairing)
            self._tool_policy_key, self._tool_policy_value = key, policy
        return self._tool_policy_value["tools"].get(name, self._tool_policy_value["other"])

    def begin_tool(self, name, arguments):
        """Call identity excludes provider ids and survives visit recovery."""
        signature = self._fingerprint([name, arguments])
        with self._progress_lock:
            fresh = signature not in self.seen_calls
            self.seen_calls.add(signature)
        return fresh

    @staticmethod
    def _fingerprint(value):
        return hashlib.sha256(json.dumps(value, sort_keys=True, ensure_ascii=False).encode()).hexdigest()

    def observe_tool(self, name, arguments, result, *, fresh, changed=False):
        if self.role != "maker" or not self.budget.no_progress_window:
            return
        evidence = None
        if name in {"read", "grep", "glob"}:
            content = result if name == "read" else result.get("matches" if name == "grep" else "entries", [])
            if content:
                evidence = (self.seen_content, self._fingerprint(content))
        elif name == "run_tests" and isinstance(result, dict):
            # A changed verdict is new even when rerunning the same tests. Receipts,
            # timings, tree ids and log text alone do not establish progress.
            def verdict(run):
                return {"status": run.get("status"), "totals": run.get("totals"),
                        "failures": sorted((test.get("test"), test.get("outcome"))
                                           for test in run.get("tests", [])
                                           if test.get("outcome") in {"failed", "error"}),
                        "groups": [verdict(group) for group in run.get("groups", [])]}
            evidence = (self.seen_tests, self._fingerprint({
                "selection": sorted(set(arguments.get("tests") or [])),
                "base": bool(arguments.get("base")), "result": verdict(result)}))
        with self._progress_lock:
            evidence_new = None
            if evidence is not None:
                seen, signature = evidence
                evidence_new = signature not in seen
                seen.add(signature)
            progress = self._tool_rule(name)["progress"][fresh][changed][
                {None: 0, False: 1, True: 2}[evidence_new]]
            if progress:
                self.no_progress_turns = 0
                self.last_progress_kind = name

    def remaining(self, phase=None):
        return budget_check("remaining", maker=self.role == "maker", turns=self.turns, spent=self.spent,
                            tool_turns=self.budget.tool_turns, input_tokens=self.budget.input_tokens,
                            finalization_turns=self.budget.finalization_turns,
                            no_progress_window=self.budget.no_progress_window,
                            no_progress_turns=self.no_progress_turns, last_progress_kind=self.last_progress_kind,
                            implementation_files=bool(self.implementation_files),
                            implementation_started=self.implementation_started, phase=phase,
                            exhaustion=self.exhaustion, finalizing=self.finalizing, final=self.final)

    def wrap_tool_call(self, request, handler):
        action = self._tool_rule(request.tool_call["name"])["permission"]
        if action == "unavailable":
            return ToolMessage(content="Tools are unavailable for this turn.",
                               tool_call_id=request.tool_call["id"], status="error")
        if action == "reserve":
            return ToolMessage(content="The finalization reserve permits verification and readback only. "
                               "New edits and generation are unavailable. Report unfinished work precisely.",
                               tool_call_id=request.tool_call["id"], status="error")
        response = handler(request)
        if (request.tool_call["name"] in {"write", "edit"}
                and request.tool_call["args"].get("path") in self.implementation_files
                and isinstance(response, ToolMessage) and response.status != "error"):
            self.implementation_started = True
        return response

    def wrap_model_call(self, request, handler):
        # Recovery rechecks limits, including context stops from older workers.
        if not self.repairing and not self.final:
            self.exhaustion = None
        source = request.messages
        messages = (self.compaction_prefix + source[self.compaction_offset:]
                    if self.compacted else list(source))
        self.finalizing = budget_check("finalizing", maker=self.role == "maker", turns=self.turns,
                                       no_progress=self.no_progress_turns, allowed=self.budget.tool_turns,
                                       window=self.budget.no_progress_window, reserve=self.budget.finalization_turns)
        notice = None
        if self.role == "maker":
            # Proxy providers may hoist system messages into the cached prefix.
            notice = HumanMessage(content=BUDGET_PREFIX + json.dumps(self.remaining(), sort_keys=True))
            messages.append(notice)
        request = request.override(messages=messages)
        bound = input_estimate(request)
        context = self._context(bound)
        compact_now = budget_check("compact_now", maker=self.role == "maker", compacted=self.compacted,
                                   context=context, context_size=self.context_size,
                                   has_ai=any(isinstance(message, AIMessage) for message in messages))
        if compact_now:
            context_before = context
            messages = retained(messages, self.budget.history_bytes)
            self.compaction_prefix = [m for m in messages if m is not notice]
            self.compaction_offset = len(source)
            self.compacted = True
            self.compaction_count += 1
            request = request.override(messages=messages)
            bound = input_estimate(request)
            context = self._context(bound)
            reduced = budget_check("compaction", before=context_before, after=context)
            self.compacted_below = reduced and not budget_check(
                "compact_now", maker=True, compacted=True, context=context,
                context_size=self.context_size, has_ai=True)

        self.context_fraction = context / self.context_size

        weights = [self.weights.uncached, self.weights.creation, self.weights.read]
        predicted = budget_check("predict", context=context, last_input=self.last_input,
                                 cache_active=self.cache_active, compact_now=compact_now, weights=weights)
        self.in_flight, self.in_flight_reserve = predicted, 0
        final = False
        if self.repairing:
            # A repair turn spends the same allowance, without the final-answer reserve.
            if budget_check("exhaustion", maker=False, turns=0, allowed=1, no_progress=0, window=0,
                            context_full=False, spent=self.spent, predicted=predicted, reserve=0,
                            input_limit=self.budget.input_tokens):
                raise RepairUnavailable()
        else:
            original = [m for m in messages if isinstance(m, HumanMessage)]
            reserve = input_estimate(request.override(messages=original, tools=[]))
            reserve = budget_check("reserve", estimate=reserve, context=context,
                                   history_bytes=self.budget.history_bytes, finish_characters=len(FINISH),
                                   cache_active=self.cache_active, weights=weights)
            context_full = budget_check("context_full", maker=self.role == "maker", compacted=self.compacted,
                                        compacted_below=self.compacted_below, compact_now=compact_now,
                                        source_len=len(source), offset=self.compaction_offset,
                                        context=context, context_size=self.context_size)
            exhaustion = budget_check("exhaustion", maker=self.role == "maker", turns=self.turns,
                                      allowed=self.budget.tool_turns, no_progress=self.no_progress_turns,
                                      window=self.budget.no_progress_window, context_full=context_full,
                                      spent=self.spent, predicted=predicted, reserve=reserve,
                                      input_limit=self.budget.input_tokens)
            final = exhaustion is not None
            self.in_flight_reserve = 0 if final else reserve
            if self.final:
                raise BudgetUnavailable("The final reply already consumed the remaining execution allowance.")
            if final:
                self.final = True
                self.exhaustion = exhaustion
                if notice is not None:
                    notice.content = BUDGET_PREFIX + json.dumps(self.remaining("final"), sort_keys=True)
                request = request.override(tool_choice="none",
                                           messages=messages + [HumanMessage(content=FINISH)])
                bound = input_estimate(request)
            if budget_check("exhaustion", maker=False, turns=0, allowed=1, no_progress=0, window=0,
                            context_full=False, spent=self.spent, predicted=predicted, reserve=0,
                            input_limit=self.budget.input_tokens):
                raise BudgetUnavailable("No final model call fits the remaining input-token allowance.")
        self._tool_rule("")
        response = handler(request)
        if notice is not None:
            # Persist budget notices with the call they preceded so subsequent
            # requests retain their append-only cache prefix and crash history.
            response.result.insert(0, notice)
        if final:
            for message in response.result:
                if isinstance(message, AIMessage):
                    message.response_metadata["tl3_budget_exhausted"] = True
                    message.tool_calls = []
                    message.invalid_tool_calls = []
        return self._account(response, bound)

    def charge_dropped(self):
        """Charge a dropped call, then report whether its retry still fits with its reserve."""
        decision = budget_check("dropped", spent=self.spent, in_flight=self.in_flight,
                                reserve=self.in_flight_reserve, allowed=self.budget.input_tokens)
        self.spent = decision["spent"]
        return decision["fits"]

    # A resumed visit continues with the same allowance, compaction and cache pricing.
    # The final-answer flag is not kept: a resumed run recomputes it from this state.
    def state(self):
        return {"spent": self.spent, "turns": self.turns, "last_input": self.last_input,
                "last_estimate": self.last_estimate, "cache_active": self.cache_active,
                "compacted": self.compacted, "compacted_below": self.compacted_below,
                "compaction_count": self.compaction_count,
                "compaction_offset": self.compaction_offset,
                "compaction_prefix": messages_to_dict(self.compaction_prefix), "exhaustion": self.exhaustion,
                "implementation_started": self.implementation_started,
                "no_progress_turns": self.no_progress_turns, "last_progress_kind": self.last_progress_kind,
                "seen_calls": sorted(self.seen_calls), "seen_content": sorted(self.seen_content),
                "seen_tests": sorted(self.seen_tests)}

    def restore(self, state):
        prefix = messages_from_dict(state["compaction_prefix"])
        verdict = budget_check("restore", state=state, maker=self.role == "maker")
        if verdict["kind"] == "invalid":
            raise InvalidAllowanceState(verdict["message"])
        state = verdict["state"]
        self.spent, self.turns, self.last_input, self.compaction_offset = (
            state[name] for name in ("spent", "turns", "last_input", "compaction_offset"))
        self.cache_active, self.compacted, self.compacted_below = (
            state[name] for name in ("cache_active", "compacted", "compacted_below"))
        self.compaction_count = state.get("compaction_count", int(self.compacted))
        self.last_estimate = state["last_estimate"]
        self.compaction_prefix = prefix
        self.exhaustion = state.get("exhaustion")
        self.implementation_started = state.get("implementation_started") is True
        self.no_progress_turns = state.get("no_progress_turns", 0)
        self.last_progress_kind = state.get("last_progress_kind")
        self.seen_calls, self.seen_content, self.seen_tests = (
            set(state.get(name, [])) for name in ("seen_calls", "seen_content", "seen_tests"))

    def _account(self, response, bound):
        usage = [message.usage_metadata for message in response.result if isinstance(message, AIMessage)]
        result = budget_check("account", usage=usage, bound=bound,
                              weights=[self.weights.uncached, self.weights.creation, self.weights.read],
                              spent=self.spent, turns=self.turns, no_progress=self.no_progress_turns,
                              final=self.final, repairing=self.repairing)
        self.last_input = result["last_input"]
        self.spent = result["spent"]
        self.cache_active = result["cache_active"]
        self.last_estimate = bound
        self.turns = result["turns"]
        self.no_progress_turns = result["no_progress_turns"]
        return response
