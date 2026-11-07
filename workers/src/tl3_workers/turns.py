"""Model turn retries and reply repairs shared by every role."""

from contextlib import contextmanager

import httpx
from jsonschema import ValidationError
from langchain.agents.middleware import AgentMiddleware
from langchain_core.messages import AIMessage, HumanMessage, ToolMessage
import openai


# A refused connection, or a stream the proxy or network cut off mid-reply.
DROPPED = (openai.APIConnectionError, httpx.TransportError)
REPAIR = ("Your reply failed validation: {}\nTools are unavailable for this turn. Return one "
          "corrected JSON object matching your reply schema and nothing else.")


REPAIR_MESSAGE_BYTES = 2_048


def validation_message(error):
    return error.message if isinstance(error, ValidationError) else str(error)


def repair_message(error):
    from tl3_workers.rules import render_all
    from tl3_workers.contracts import schema_violations
    role = getattr(error, 'rule_role', 'schema')
    if getattr(error, 'rule_violations', None):
        return render_all(error.rule_violations, role=role)
    if isinstance(error, ValidationError):
        return render_all(schema_violations((error,), role=role), role=role)
    # Legacy sources keep their existing bounded repair text until migrated.
    message = validation_message(error)
    raw = message.encode()
    if len(raw) <= REPAIR_MESSAGE_BYTES:
        return message
    # jsonschema puts the instance first and the reason last; keep both ends.
    head, tail = raw[:REPAIR_MESSAGE_BYTES - 512], raw[-500:]
    return head.decode(errors="ignore") + " [...] " + tail.decode(errors="ignore")


class InvalidReply(ValueError):
    def __init__(self, first, second):
        super().__init__(f"{validation_message(first)}; repaired reply: {validation_message(second)}")
        self.errors = (first, second)


class RepairScopeError(ValidationError):
    """The single correction changed data outside its row permissions."""

    def __init__(self):
        super().__init__('repaired reply changed fields outside the permitted edits',
                         validator='repair_scope')


class Unrepairable(ValueError):
    """A check failed for a reason the model cannot fix, such as missing tokenizer data."""


class RepairUnavailable(Exception):
    """The run's input allowance has no room for a repair turn."""


def _tool_name(tool):
    if isinstance(tool, dict):
        return tool.get("name") or tool.get("function", {}).get("name")
    return getattr(tool, "name", None)


class TurnControl(AgentMiddleware):
    """Retry a dropped model call once, and make repair turns tool-free."""

    def __init__(self):
        self.allowance = None
        self.repairing = False
        self.repair_attempts = 0
        self.repair_unavailable = None
        # The last model call as sent, its result and its tools: a later turn reuses its cached prefix.
        self.last_exchange = None
        # Tools the harness profile removes after this middleware, before the model sees them.
        self.hidden_tools = frozenset()

    @contextmanager
    def repair(self, history, error):
        history.append(HumanMessage(content=REPAIR.format(repair_message(error))))
        self.repairing = True
        if self.allowance is not None:
            self.allowance.repairing = True
        try:
            yield
        finally:
            self.repairing = False
            if self.allowance is not None:
                self.allowance.repairing = False

    def wrap_model_call(self, request, handler):
        if self.repairing:
            request = request.override(tool_choice="none")
        try:
            response = handler(request)
        except DROPPED:
            # The retry happens on the failure itself. A second failure ends the run.
            if self.allowance is not None and not self.allowance.charge_dropped():
                raise
            response = handler(request)
        if self.repairing:
            # A repair turn may not act. Its reply is validated as text.
            response.result = [
                message.model_copy(update={
                    "tool_calls": [], "invalid_tool_calls": [],
                    "additional_kwargs": {key: value for key, value in message.additional_kwargs.items()
                                          if key != "tool_calls"}})
                if isinstance(message, AIMessage) else message
                for message in response.result]
        shown = [tool for tool in request.tools if _tool_name(tool) not in self.hidden_tools]
        self.last_exchange = (request.system_message, list(request.messages), list(response.result), shown)
        return response

    def wrap_tool_call(self, request, handler):
        if self.repairing:
            return ToolMessage(content="Tools are unavailable for this turn.",
                               tool_call_id=request.tool_call["id"], status="error")
        return handler(request)


def repaired(control, history, ask, check):
    """Return check(ask()), giving an invalid reply one repair turn in the same run."""
    text = ask()
    original = text
    try:
        return check(text)
    except Unrepairable:
        raise
    except (ValueError, ValidationError) as first:
        from tl3_harness.rules import SourceEdit
        if any(isinstance(item.repair, SourceEdit) for item in getattr(first, 'rule_violations', ())):
            raise first
        try:
            with control.repair(history, first):
                control.repair_attempts = 1
                text = ask()
        except RepairUnavailable:
            control.repair_attempts = 0
            control.repair_unavailable = 'input_allowance'
            raise first from None
        try:
            deferred = getattr(first, 'guard_after_check', False)
            result = check(text) if deferred else None
            violations = () if deferred else getattr(first, 'rule_violations', ())
            if violations:
                from tl3_workers.contracts import read_reply
                from tl3_workers.rules import permits
                before = read_reply(original) if isinstance(original, str) else original
                after = read_reply(text) if isinstance(text, str) else text
                if not permits(violations, before, after, role=getattr(first, 'rule_role', 'schema')):
                    raise RepairScopeError()
            return result if deferred else check(text)
        except (ValueError, ValidationError) as second:
            raise InvalidReply(first, second) from second
