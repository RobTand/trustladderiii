"""The maker's retrospective (#648): one tool-free turn after its accepted reply.

The retro tells TL3 how to improve the maker's environment. It never changes
the run: its usage stays out of the execution allowance and the run's usage
totals, and any failure is recorded, never raised.

The protocol allows no event after the reply, so the retro runs before it. It
reports one model turn of progress, then gets a hard deadline of at most half
the agent worker's silence and no-progress allowances, so it can never get the
run stopped before the accepted reply is delivered.
"""

from contextlib import nullcontext
import contextvars
import json
import threading

import httpx
from jsonschema import Draft202012Validator, ValidationError
from langchain_core.messages import AIMessage, HumanMessage
import openai

from tl3_workers.contracts import read_reply

SPAN = "tl3.maker_retro"
DEADLINE_S = 60


def deadline(silence_secs=None, no_progress_secs=None):
    """At most DEADLINE_S, and at most half of each of the worker's allowances that is known."""
    return min([DEADLINE_S, *(allowance / 2 for allowance in (silence_secs, no_progress_secs) if allowance is not None)])
RESULT_BYTES = 5_120
ITEM_CHARS = 240
FIELDS = ("slowed_by", "missing", "checks", "noise")
SCHEMA = {
    "type": "object",
    "properties": {name: {"type": "array", "maxItems": 4,
                          "items": {"type": "string", "minLength": 1, "maxLength": ITEM_CHARS}}
                   for name in FIELDS},
    "required": list(FIELDS),
    "additionalProperties": False,
}
PROMPT = f"""Retrospective. TL3 accepted your reply. This turn cannot change it.
This turn does not count against your budget. Tools are not available.
Look back at this run. Tell TL3 how to make the next run easier.
Use only facts from this run.
- slowed_by: the steps that used the most turns or tokens, and why.
- missing: the information or tools that you needed and did not have.
- checks: the automated checks that could have found your mistakes earlier.
- noise: the instructions that did not change what you did, or that were wrong.
Write in ASD-STE100: short sentences, active voice, one fact in each item.
Each list has 0 to 4 items. Each item has {ITEM_CHARS} characters or fewer.
Reply with one JSON object only. This schema replaces your earlier reply schema:
{json.dumps(SCHEMA)}"""


def _usage(message):
    usage = getattr(message, "usage_metadata", None) or {}
    counts = {"input_tokens": usage.get("input_tokens"), "output_tokens": usage.get("output_tokens"),
              "cached_tokens": (usage.get("input_token_details") or {}).get("cache_read")}
    return {key: value for key, value in counts.items() if type(value) is int and value >= 0}


def _status(error):
    if isinstance(error, (openai.APITimeoutError, httpx.TimeoutException, TimeoutError)):
        return "timeout"
    if isinstance(error, (ValueError, ValidationError)):
        return "invalid"
    return "failed"


def _messages(control):
    """The last call's messages and result, as sent, so the retro reads its cached prefix."""
    system, sent, result, _ = control.last_exchange
    answer = [message.model_copy(update={"tool_calls": [], "invalid_tool_calls": []})
              if isinstance(message, AIMessage) else message for message in result]
    return [*([system] if system is not None else []), *sent, *answer, HumanMessage(content=PROMPT)]


def _model(model, tools):
    """The maker's model with the last call's tools bound but uncallable.

    The same tool definitions keep the provider's cached prefix, as repair and
    final turns do (#652). Without tools, the call is plain."""
    from langchain_openai import ChatOpenAI

    if tools:
        return model.bind_tools(tools, tool_choice="none")
    if isinstance(model, ChatOpenAI):
        # parallel_tool_calls is only valid with tools.
        kwargs = {key: value for key, value in model.model_kwargs.items() if key != "parallel_tool_calls"}
        return model.model_copy(update={"model_kwargs": kwargs})
    return model


def _ask(model, control, limit):
    record = {"status": "failed"}
    try:
        message = _model(model, control.last_exchange[3]).invoke(_messages(control), timeout=limit)
        record.update(_usage(message))
        result = read_reply(message.text)
        Draft202012Validator(SCHEMA).validate(result)
        if len(json.dumps(result).encode()) > RESULT_BYTES:
            raise ValueError("retro result too large")
        record.update(status="recorded", result=result)
    except Exception as error:
        record["status"] = _status(error)
    return record


def _traced(model, control, capture, limit):
    """The retro call, in its own thread's copy of the run's context."""
    from tl3_workers.facts import current as current_facts
    from tl3_workers.tracing import failure

    # Detached facts count none of the retro's usage in the run's totals.
    current_facts.set(None)
    record = {"status": "failed"}
    try:
        if capture is not None:
            import mlflow
            span_context = mlflow.start_span(name=SPAN, span_type="AGENT")
        else:
            span_context = nullcontext()
        with span_context as span:
            record = _ask(model, control, limit)
            if span is not None:
                span.set_outputs(record)
    except Exception:
        failure()
    return record


def run(model, control, emit=None, silence_secs=None, no_progress_secs=None):
    """Run the retro once, within its deadline, and record it. Never raises."""
    from tl3_workers.facts import current as current_facts
    from tl3_workers.tracing import active, failure

    if control.last_exchange is None:
        return None
    facts = current_facts.get()
    capture = active.get()
    record = {"status": "timeout"}
    try:
        if emit is not None:
            emit({"event": "progress", "unit": "model_turn"})
        limit = deadline(silence_secs, no_progress_secs)
        context = contextvars.copy_context()
        result = {}
        # A daemon thread: past the deadline the reply goes out, and process
        # exit ends a call that is still waiting on the model.
        thread = threading.Thread(target=lambda: result.update(record=context.run(_traced, model, control, capture, limit)),
                                  name="tl3-retro", daemon=True)
        thread.start()
        thread.join(limit)
        record = result.get("record", record)
    except Exception:
        record = {"status": "failed"}
        failure()
    try:
        if facts is not None:
            facts.retro = record
        if capture is not None:
            capture.tags["retro"] = record["status"]
            for key in ("input_tokens", "cached_tokens", "output_tokens"):
                if key in record:
                    capture.tags["retro_" + key] = str(record[key])
    except Exception:
        failure()
    return record
