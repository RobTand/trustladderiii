"""Real worker entrypoint with only its model replaced by scripted responses."""
from importlib.resources import files
import io
import json
import runpy
import socket
import sys
from unittest.mock import patch

from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage
from tl3_workers import runtime
from tl3_workers.contracts import model_facing, schema

request = json.load(sys.stdin)


class Model(FakeMessagesListChatModel):
    def bind_tools(self, tools, **kwargs):
        assert tools == []
        return self

    def _generate(self, messages, **kwargs):
        prompt = files("tl3_workers").joinpath("prompts/classifier.md").read_text()
        assert messages[0].text == prompt + "\n\nReply with one JSON object only: no prose, no code fences. It must match this schema:\n" + json.dumps(model_facing(schema("classifier-reply")), ensure_ascii=False)
        assert json.loads(messages[1].text) == request["inputs"]
        assert request["key_file"] not in str(messages)
        return super()._generate(messages, **kwargs)


model = Model(responses=[
    AIMessage(content="", tool_calls=[{"name": "write", "args": {"path": "forbidden.txt", "content": "no"}, "id": "write-1", "type": "tool_call"}]),
    AIMessage(content=json.dumps({"kind": "restart"})),
    AIMessage(content=json.dumps({"kind": "remark"})),
])


def no_network(*args, **kwargs):
    raise AssertionError("network forbidden in worker contract")


with patch.object(runtime, "create_model", lambda _: model), \
     patch.object(socket.socket, "connect", no_network), \
     patch.object(socket.socket, "connect_ex", no_network), \
     patch.object(socket, "getaddrinfo", no_network):
    sys.stdin = io.StringIO(json.dumps(request))
    sys.argv = ["tl3_workers", "run"]
    runpy.run_module("tl3_workers", run_name="__main__")
