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

from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage
from tl3_workers import runtime

os.environ["TIKTOKEN_CACHE_DIR"] = str(Path(tempfile.gettempdir()) / "data-gym-cache")

mode = sys.argv[1]
marker = Path(sys.argv[2])
brief = {"kind": "brief", "brief": "Fix the greeting.", "goals": [
    {"description": "Return hello.", "verification": "test"}], "memory_ids": [], "files": ["greeting.py"]}


def publish_marker(path, text):
    pending = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    pending.write_text(text)
    pending.replace(path)


def no_network(*args, **kwargs):
    raise AssertionError("network forbidden in supervisor fixtures")


def create_model(request):
    if mode != "plan-blocked" or request["role"] == "planner":
        publish_marker(marker, str(os.getpid()))
    if mode == "blocked" or (mode in {"audit-blocked", "steering-commands", "steering-replacement"} and request["role"] == "audit") or (mode == "plan-blocked" and request["role"] == "planner"):
        while not marker.with_suffix(".release").exists():
            time.sleep(0.01)
    if request["role"] == "classifier":
        body = request["inputs"]["comment"]["body"]
        replies = json.loads((Path(__file__).parent / "fixtures/steering/classifications.json").read_text())
        reply = replies[body]
        if reply.get("question_id") == "current":
            reply["question_id"] = request["inputs"]["state"]["open_question"]["id"]
        if body == "Answer the old question":
            publish_marker(marker.with_suffix(".classifying"), "ready")
            while not marker.with_suffix(".answer").exists():
                time.sleep(0.01)
        return FakeMessagesListChatModel(responses=[AIMessage(content=json.dumps(reply))])
    if mode.startswith("steering") and request["role"] == "ingest" and request["inputs"]["issue"]["number"] != 12:
        while True:
            time.sleep(1)
    reply = brief if request["role"] == "ingest" else {"verdict": "agree", "objections": []}
    if request["role"] == "planner":
        reply = {"kind": "leaf", "files": ["greeting.py", "tests/test_greeting.py"],
                 "checks": [{"kind": "demonstrating", "node_ids": ["tests/test_greeting.py::test_hello"], "goal_ids": [0]}],
                 "no_design_questions": True, "lands_on_its_own": True}
        if mode == "plan-bad-schema":
            reply["checks"][0]["kind"] = "invalid"
        elif mode == "plan-invalid":
            reply["files"] = ["conftest.py"]
        elif mode in {"plan-split", "plan-bad-split"}:
            reply = {"kind": "split", "reason": "does_not_fit", "pieces": [
                {"brief": "Fix greeting.", "goal_ids": [0], "files": ["greeting.py"]},
                {"brief": "Test greeting.", "goal_ids": [0], "files": ["tests/test_greeting.py"]}]}
            if mode == "plan-bad-split": reply["pieces"] = reply["pieces"][:1]
        elif mode == "plan-cant-meet":
            reply = {"kind": "cant_meet", "report": "Need a decision on the greeting.", "files": []}
    if mode == "early":
        reply = {"kind": "early_end", "outcome": "duplicate", "reason": "Already requested."} if request["role"] == "ingest" else reply
    if mode in {"disagree", "steering-answers"} and request["role"] == "audit":
        reply = {"verdict": "disagree", "objections": ["Preserve punctuation."]}
    return FakeMessagesListChatModel(responses=[AIMessage(content=json.dumps(reply))])


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

with patch.object(FakeMessagesListChatModel, "bind_tools", lambda self, tools, **kwargs: self, create=True), \
     patch.object(runtime, "create_model", create_model), \
     patch.object(socket.socket, "connect", no_network), \
     patch.object(socket.socket, "connect_ex", no_network), \
     patch.object(socket, "getaddrinfo", no_network):
    sys.argv = ["tl3_workers", "run"]
    runpy.run_module("tl3_workers", run_name="__main__")
