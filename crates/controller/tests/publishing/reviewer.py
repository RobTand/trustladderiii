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
    assert Path(request["worktree"], "value.py").read_text() == "def value(): return 2\n"
    first = not record.exists()
    with record.open("a") as output:
        output.write(json.dumps(request) + "\n")
    reply = {"verdict": "approve", "head_sha": inputs["head_sha"],
             "blocking_findings": [], "findings": [{"problem": "Optional style concern", "evidence": "This does not break a goal or rule."}]}
    if mode == "changes" and first:
        reply = {"verdict": "changes", "blocking_findings": [{
            "citation": {"kind": "goal", "id": inputs["goals"][0]["id"]},
            "problem": "Check the result once more.", "evidence": "A fixture requests one fix round."}], "findings": []}
    class Model(FakeMessagesListChatModel):
        def _generate(self, messages, **kwargs):
            if mode == "changes" and not first:
                previous = [m for m in messages if m.type == "ai"]
                assert len(previous) == 1
                assert json.loads(previous[0].content)["verdict"] == "changes"
                assert previous[0].additional_kwargs["reasoning_content"] == "The goal needs another check."
            return super()._generate(messages, **kwargs)

    return Model(responses=[AIMessage(content=json.dumps(reply), additional_kwargs={
        "reasoning_content": "The goal needs another check."})])


def no_network(*args, **kwargs):
    raise AssertionError("review fixture must remain offline")


with patch.object(FakeMessagesListChatModel, "bind_tools", lambda self, tools, **kwargs: self, create=True), \
     patch.object(runtime, "create_model", model), \
     patch.object(socket.socket, "connect", no_network), \
     patch.object(socket.socket, "connect_ex", no_network), \
     patch.object(socket, "getaddrinfo", no_network):
    sys.argv = ["tl3_workers", "run"]
    runpy.run_module("tl3_workers", run_name="__main__")
