"""Run the real writer protocol with an offline model that repairs references."""
import json
import runpy
import socket
import sys
from unittest.mock import patch

from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage
from tl3_workers import runtime


class WriterModel(FakeMessagesListChatModel):
    def bind_tools(self, tools, **kwargs):
        assert tools == []
        return self

    def _generate(self, messages, **kwargs):
        inputs = json.loads(messages[1].text)
        violations = inputs.get("violations", [])
        if violations:
            for token in ["@octocat", "@org/team", "owner/repo#12", "#56"]:
                assert any("Remove" in problem and "'" + token + "'" in problem
                           for problem in violations), violations
            text = "The artwork needs permission."
        else:
            text = "Ask @octocat and @org/team about owner/repo#12 or #56"
        if inputs["kind"] == "question":
            reply = {key: inputs["facts"][key] for key in [
                "options", "recommended_option", "recommendation_reason", "how_to_answer",
            ]}
            reply.update(request_quote=inputs["facts"]["issue_title"], obstacle=text)
        elif inputs["kind"] == "clarification":
            reply = {"text": 'About "Maybe do that", ' + text.rstrip(".") + '?'}
        else:
            reply = {"text": text}
        self.responses = [AIMessage(content=json.dumps(reply))]
        return super()._generate(messages, **kwargs)


def no_network(*args, **kwargs):
    raise AssertionError("network forbidden in writer reference tests")


with patch.object(runtime, "create_model", lambda request: WriterModel(responses=[])), \
     patch.object(socket.socket, "connect", no_network), \
     patch.object(socket.socket, "connect_ex", no_network), \
     patch.object(socket, "getaddrinfo", no_network):
    sys.argv = ["tl3_workers", "run"]
    runpy.run_module("tl3_workers", run_name="__main__")
