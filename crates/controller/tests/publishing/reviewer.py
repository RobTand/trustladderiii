"""Run the actual reviewer entry point with an offline model."""
import json
from pathlib import Path
import runpy
import socket
import sys
from unittest.mock import patch

from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage
from tl3_workers import runtime

record = Path(sys.argv[1])
mode = sys.argv[2]


def model(request):
    assert request["role"] == "reviewer"
    inputs = request["inputs"]
    assert any(rule["id"] == "test-report-integrity" for rule in inputs["standing_rules"])
    assert all(check["custody"] == "in-process" for check in inputs["checks"])
    assert Path(request["worktree"], "value.py").read_text() == (
        "def value():\n    return 2\n" if request["trace"]["round"] == 1 else "def value(): return 2\n")
    count = len(record.read_text().splitlines()) if record.exists() else 0
    first = count == 0
    with record.open("a") as output:
        output.write(json.dumps(request) + "\n")
    reply = {"verdict": "approve", "head_sha": inputs["head_sha"],
             "blocking_findings": [], "findings": [{"problem": "Optional style concern", "evidence": "This does not break a goal or rule."}]}
    if (mode in ("changes", "review-retry", "round-one-rebase", "out-of-room-fix") and first) or (mode == "new-attempt" and count < 2):
        reply = {"verdict": "changes", "blocking_findings": [{
            "citation": {"kind": "goal", "id": inputs["goals"][0]["id"]},
            "problem": "Check the result once more.", "evidence": "A fixture requests one fix round."}], "findings": []}
    class Model(FakeMessagesListChatModel):
        def _generate(self, messages, **kwargs):
            if mode in ("changes", "new-attempt", "review-retry", "round-one-rebase"):
                humans = [m for m in messages if m.type == "human"]
                assert len(humans) == ((count + 1) if mode == "round-one-rebase" else (2 if request["resume_session"] else 1))
            if mode in ("changes", "new-attempt", "review-retry", "round-one-rebase") and request["resume_session"]:
                previous = [m for m in messages if m.type == "ai"]
                assert len(previous) == (count if mode == "round-one-rebase" else 1)
                assert json.loads(previous[0].content)["verdict"] == "changes"
                assert previous[0].additional_kwargs["reasoning_content"] == "The goal needs another check."
            if mode == "review-retry" and count == 2:
                raise AssertionError("retry must use the saved reply without a model turn")
            if mode.startswith("out-of-room"):
                assert len([m for m in messages if m.type == "human"]) == (2 if request["resume_session"] else 1)
            return super()._generate(messages, **kwargs)

    exhausted = mode.startswith("out-of-room") and (mode != "out-of-room-fix" or not first)
    return Model(responses=[AIMessage(content=json.dumps(reply), usage_metadata=(
        {"input_tokens": 127_499, "output_tokens": 1, "total_tokens": 127_500} if exhausted else None), additional_kwargs={
        "reasoning_content": "The goal needs another check."})])


run_worker = runtime.run


def run(request, emit):
    def output(event):
        marker = record.with_suffix(".crashed")
        if (mode == "review-retry" and request["resume_session"]
                and event["event"] == "reply" and not marker.exists()):
            marker.write_text("crashed after checkpoint before reply")
            import os
            import signal
            os.kill(os.getpid(), signal.SIGKILL)
        emit(event)
    run_worker(request, output)


def no_network(*args, **kwargs):
    raise AssertionError("review fixture must remain offline")


with patch.object(FakeMessagesListChatModel, "bind_tools", lambda self, tools, **kwargs: self, create=True), \
     patch.object(runtime, "create_model", model), \
     patch.object(runtime, "run", run), \
     patch.object(socket.socket, "connect", no_network), \
     patch.object(socket.socket, "connect_ex", no_network), \
     patch.object(socket, "getaddrinfo", no_network):
    sys.argv = ["tl3_workers", "run"]
    runpy.run_module("tl3_workers", run_name="__main__")
