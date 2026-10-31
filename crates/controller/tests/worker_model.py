"""Replace model construction only, as in workers/tests/worker_process.py."""
import json
import os
import tempfile
from pathlib import Path
import runpy
import socket
import sys
import time
from unittest.mock import patch


os.environ["TIKTOKEN_CACHE_DIR"] = str(Path(tempfile.gettempdir()) / "data-gym-cache")

mode = sys.argv[1]
marker = Path(sys.argv[2])
from worker_replies import brief, publish_marker, scripted_reply


def no_network(*args, **kwargs):
    raise AssertionError("network forbidden in supervisor fixtures")


def create_model(request):
    if request["role"] == "maker":
        return FakeMessagesListChatModel(responses=[
            AIMessage(content="", tool_calls=[{"name": "write", "args": {
                "path": "greeting.py", "content": "def greet(): return 'hello'\n",
            }, "id": "greeting", "type": "tool_call"}]),
            AIMessage(content=json.dumps({"kind": "done", "self_check": [
                {"goal_id": goal["id"], "status": "met", "evidence": "greet returns hello with the original signature."}
                for goal in request["inputs"]["goals"]
            ]})),
        ])
    reply = scripted_reply(request, mode, marker)
    return FakeMessagesListChatModel(responses=[AIMessage(content=json.dumps(reply))])

if mode == "external-wait":
    json.load(sys.stdin)
    print(json.dumps({"event": "progress", "unit": "external_wait",
                      "wait": {"upstream": "prismabuild", "key": "a" * 64}}), flush=True)
    marker.write_text(str(os.getpid()))
    repeated = False
    while not marker.with_suffix(".release").exists():
        if marker.with_suffix(".repeat").exists() and not repeated:
            print(json.dumps({"event": "progress", "unit": "external_wait",
                              "wait": {"upstream": "prismabuild", "key": "a" * 64}}), flush=True)
            marker.with_suffix(".repeated").write_text("declared")
            repeated = True
        time.sleep(0.01)
    sys.exit(1)

if mode in {"bad-schema", "nonzero", "missing", "reply-before-exit", "duplicate", "duplicate-key", "wrong-role", "malformed", "partial", "partial-crash", "oversized-size", "forged-fit", "forged-context", "forged-overhead", "forged-files"}:
    request = json.load(sys.stdin)
    reply = dict(brief) if mode != "bad-schema" else {"kind": "brief", "brief": "Missing goals"}
    if mode != "bad-schema":
        from tl3_workers.forecast import Forecaster
        os.chdir(request["worktree"])
        reply["forecast"] = Forecaster(request["inputs"]["forecast"]).measure(reply["brief"], reply["files"])
    if mode == "oversized-size":
        reply["forecast"]["brief_tokens"] = 256 * request["inputs"]["forecast"]["fit_tokens"]
        reply["forecast"]["tokens"] = reply["forecast"]["brief_tokens"] + sum(reply["forecast"]["file_tokens"].values()) + reply["forecast"]["overhead_tokens"]
        reply["forecast"]["fits"] = False
    elif mode in {"forged-fit", "forged-context"}:
        field = "fit_tokens" if mode == "forged-fit" else "context_tokens"
        reply["forecast"][field] += 1
    elif mode == "forged-overhead":
        reply["forecast"]["overhead_tokens"] += 1
        reply["forecast"]["tokens"] += 1
    elif mode == "forged-files":
        reply["forecast"]["file_tokens"] = {"forged.py": sum(reply["forecast"]["file_tokens"].values())}
    if mode == "duplicate-key":
        print('{"event":"reply","role":"ingest","reply":{"kind":"brief","brief":"x","brief":"y","goals":[{"description":"x","verification":"test"}],"memory_ids":[]}}', flush=True)
    elif mode in {"partial", "partial-crash"}:
        print(json.dumps({"event": "progress", "unit": "model_turn"}), flush=True)
        print('{"event":"reply","role":"ingest","reply":', end="", flush=True)
    elif mode == "malformed":
        print("not json", flush=True)
    elif mode != "missing":
        print(json.dumps({"event": "reply", "role": "audit" if mode == "wrong-role" else request["role"], "reply": reply}), flush=True)
    publish_marker(marker, str(os.getpid()))
    if mode in {"reply-before-exit", "partial-crash"}:
        while not marker.with_suffix(".release").exists():
            time.sleep(0.01)
    if mode == "duplicate":
        print(json.dumps({"event": "reply", "role": request["role"], "reply": reply}), flush=True)
    sys.exit(7 if mode == "nonzero" else 0)

from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage
from tl3_workers import runtime

with patch.object(FakeMessagesListChatModel, "bind_tools", lambda self, tools, **kwargs: self, create=True), \
     patch.object(runtime, "create_model", create_model), \
     patch.object(socket.socket, "connect", no_network), \
     patch.object(socket.socket, "connect_ex", no_network), \
     patch.object(socket, "getaddrinfo", no_network):
    sys.argv = ["tl3_workers", "run"]
    runpy.run_module("tl3_workers", run_name="__main__")
