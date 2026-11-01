import socket

import pytest


def offline(*args, **kwargs):
    raise AssertionError("network forbidden in worker tests")


@pytest.fixture(autouse=True)
def no_network(monkeypatch):
    monkeypatch.setenv("LANGSMITH_TRACING", "false")
    monkeypatch.setenv("LANGCHAIN_TRACING_V2", "false")
    monkeypatch.setattr(socket.socket, "connect", offline)
    monkeypatch.setattr(socket.socket, "connect_ex", offline)
    monkeypatch.setattr(socket, "getaddrinfo", offline)


@pytest.fixture(autouse=True)
def fake_model_tool_binding(monkeypatch):
    from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
    monkeypatch.setattr(FakeMessagesListChatModel, "bind_tools", lambda self, tools, **kwargs: self)
