"""Replace model construction only, as in workers/tests/worker_process.py."""
import json
from pathlib import Path
import runpy
import socket
import sys
import time
from unittest.mock import patch

from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage
from tl3_workers import runtime

mode = sys.argv[1]
marker = Path(sys.argv[2])
brief = {"kind": "brief", "brief": "Fix the greeting.", "goals": [
    {"description": "Return hello.", "verification": "test"}], "memory_ids": []}


def no_network(*args, **kwargs):
    raise AssertionError("network forbidden in supervisor fixtures")


def create_model(request):
    marker.write_text(str(__import__("os").getpid()))
    if mode == "blocked":
        while not marker.with_suffix(".release").exists():
            time.sleep(0.01)
    reply = brief if request["role"] == "ingest" else {"verdict": "agree", "objections": []}
    if mode == "early":
        reply = {"kind": "early_end", "outcome": "duplicate", "reason": "Already requested."} if request["role"] == "ingest" else reply
    if mode == "disagree" and request["role"] == "audit":
        reply = {"verdict": "disagree", "objections": ["Preserve punctuation."]}
    return FakeMessagesListChatModel(responses=[AIMessage(content=json.dumps(reply))])


if mode in {"bad-schema", "nonzero", "missing", "reply-before-exit", "duplicate", "duplicate-key", "wrong-role", "malformed"}:
    request = json.load(sys.stdin)
    reply = brief if mode != "bad-schema" else {"kind": "brief", "brief": "Missing goals"}
    if mode == "duplicate-key":
        print('{"event":"reply","role":"ingest","reply":{"kind":"brief","brief":"x","brief":"y","goals":[{"description":"x","verification":"test"}],"memory_ids":[]}}', flush=True)
    elif mode == "malformed":
        print("not json", flush=True)
    elif mode != "missing":
        print(json.dumps({"event": "reply", "role": "audit" if mode == "wrong-role" else request["role"], "reply": reply}), flush=True)
    marker.write_text(str(__import__("os").getpid()))
    if mode == "reply-before-exit":
        while not marker.with_suffix(".release").exists():
            time.sleep(0.01)
    if mode == "duplicate":
        print(json.dumps({"event": "reply", "role": request["role"], "reply": reply}), flush=True)
    sys.exit(7 if mode == "nonzero" else 0)

with patch.object(runtime, "create_model", create_model), \
     patch.object(socket.socket, "connect", no_network), \
     patch.object(socket.socket, "connect_ex", no_network), \
     patch.object(socket, "getaddrinfo", no_network):
    sys.argv = ["tl3_workers", "run"]
    runpy.run_module("tl3_workers", run_name="__main__")
