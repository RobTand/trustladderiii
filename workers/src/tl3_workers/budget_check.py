"""Synchronous transport to the release's Rust budget implementation.

Python retains LangChain objects and I/O; Rust owns every budget decision.
"""
from functools import cache
import json
import os
from pathlib import Path
import subprocess

import jsonschema

from tl3_workers.contracts import read_json
from tl3_workers.reply_check import ReplyCheckInfrastructureError

TIMEOUT_SECONDS = 5


@cache
def _validator(kind):
    schema = json.loads(Path(__file__).with_name("schemas").joinpath("budget-wire.json").read_text())
    return jsonschema.Draft202012Validator(schema["responses"][kind])


def check(kind, **facts):
    binary = os.environ.get("TL3_BINARY")
    if not binary:
        raise ReplyCheckInfrastructureError("budget validator could not start")
    try:
        result = subprocess.run(
            [binary, "check-budget"], input=json.dumps({"kind": kind, **facts}, allow_nan=False),
            text=True, encoding="utf-8", capture_output=True, check=True, timeout=TIMEOUT_SECONDS,
        )
    except subprocess.TimeoutExpired:
        raise ReplyCheckInfrastructureError("budget validator timed out") from None
    except subprocess.CalledProcessError:
        raise ReplyCheckInfrastructureError("budget validator process failed") from None
    except OSError:
        raise ReplyCheckInfrastructureError("budget validator could not start") from None
    except (UnicodeError, ValueError, TypeError):
        raise ReplyCheckInfrastructureError("budget validator returned invalid output") from None
    try:
        document = read_json(result.stdout)
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
