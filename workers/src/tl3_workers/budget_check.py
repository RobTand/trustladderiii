"""Synchronous transport to the release's Rust budget implementation.

Python retains LangChain objects and I/O; Rust owns every budget decision.
"""
from functools import cache
import json
from pathlib import Path

import jsonschema

from tl3_workers import reply_check
from tl3_workers.reply_check import ReplyCheckInfrastructureError


@cache
def _validator(kind):
    schema = json.loads(Path(__file__).with_name("schemas").joinpath("budget-wire.json").read_text())
    return jsonschema.Draft202012Validator(schema["responses"][kind])


def check(kind, **facts):
    document = reply_check.run_check(["check-budget"], {"kind": kind, **facts}, name="budget")
    try:
        if not isinstance(document, dict) or set(document) != {"result"}:
            raise ValueError()
        _validator(kind).validate(document["result"])
    except (ValueError, KeyError, TypeError, OSError, jsonschema.ValidationError, jsonschema.SchemaError):
        raise ReplyCheckInfrastructureError("budget validator returned invalid output") from None
    return document["result"]


def history_facts(history):
    # LangChain instances stay at the Python edge; only identities cross the wire.
    from langchain_core.messages import AIMessage, ToolMessage
    return [{"kind": "ai", "calls": [call["id"] for call in message.tool_calls]}
            if isinstance(message, AIMessage) else {"kind": "tool", "id": message.tool_call_id}
            if isinstance(message, ToolMessage) else {"kind": "other"} for message in history]
