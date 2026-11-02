"""Model turn retries and reply repairs shared by every role."""

from contextlib import contextmanager

import httpx
from jsonschema import ValidationError
from langchain.agents.middleware import AgentMiddleware
from langchain_core.messages import AIMessage, HumanMessage
import openai


# A refused connection, or a stream the proxy or network cut off mid-reply.
DROPPED = (openai.APIConnectionError, httpx.TransportError)
REPAIR = ("Your reply failed validation: {}\nTools are unavailable for this turn. Return one "
          "corrected JSON object matching your reply schema and nothing else.")


REPAIR_MESSAGE_BYTES = 2_048


def validation_message(error):
    return error.message if isinstance(error, ValidationError) else str(error)


def repair_message(error):
    # A schema message echoes the whole invalid reply. Keep where and why it failed.
    message = validation_message(error)
    if isinstance(error, ValidationError):
        path = "/" + "/".join(str(part) for part in error.absolute_path)
        message = f"{error.validator} failed at {path}: {message}"
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


class Unrepairable(ValueError):
    """A check failed for a reason the model cannot fix, such as missing tokenizer data."""


class RepairUnavailable(Exception):
    """The run's input allowance has no room for a repair turn."""


class TurnControl(AgentMiddleware):
    """Retry a dropped model call once, and make repair turns tool-free."""

    def __init__(self):
        self.allowance = None
        self.repairing = False

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
            request = request.override(tools=[], tool_choice=None)
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
        return response


def repaired(control, history, ask, check):
    """Return check(ask()), giving an invalid reply one repair turn in the same run."""
    text = ask()
    try:
        return check(text)
    except Unrepairable:
        raise
    except (ValueError, ValidationError) as first:
        try:
            with control.repair(history, first):
                text = ask()
        except RepairUnavailable:
            raise first from None
        try:
            return check(text)
        except (ValueError, ValidationError) as second:
            raise InvalidReply(first, second) from second
