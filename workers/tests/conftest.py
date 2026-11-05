import os
import socket

import pytest


@pytest.fixture(autouse=True)
def disk_sync(request, monkeypatch):
    if request.node.get_closest_marker('durability'):
        return
    # Content and verdict tests do not need shared-disk durability barriers.
    monkeypatch.setattr(os, 'fsync', lambda descriptor: None)
    monkeypatch.setattr(os, 'fdatasync', lambda descriptor: None)


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


@pytest.fixture(autouse=True)
def budget_binary(request, tmp_path_factory, monkeypatch):
    import json
    import subprocess
    import sys
    from pathlib import Path

    import checker_stub

    # #823: summary facts contain base commit identities. Record and replay use
    # the same immutable fixture dates so full-document equality is meaningful.
    monkeypatch.setenv('GIT_AUTHOR_DATE', '2000-01-01T00:00:00+0000')
    monkeypatch.setenv('GIT_COMMITTER_DATE', '2000-01-01T00:00:00+0000')
    tmp_path = tmp_path_factory.mktemp("checker")
    if os.environ.get("TEST_RECORD_CHECKER_DIR"):
        yield from checker_stub.record(request.node.nodeid, tmp_path, monkeypatch)
        return
    if os.environ.get("TEST_REAL_CHECKER") == "1":
        yield
        return

    cursor = tmp_path / "checker-cursor"
    checker = tmp_path / "recorded-rust-checker"
    checker.write_text(
        f"#!{sys.executable}\n"
        "import runpy, sys\n"
        f"sys.argv[1:1] = ['--replay', {request.node.nodeid!r}, {str(cursor)!r}]\n"
        f"runpy.run_path({str(Path(checker_stub.__file__).resolve())!r}, run_name='__main__')\n")
    checker.chmod(0o755)
    monkeypatch.setenv("TL3_BINARY", str(checker))
    original = subprocess.run

    def recorded(command, *args, **kwargs):
        if isinstance(command, (list, tuple)) and command and command[0] == str(checker):
            try:
                value = checker_stub.replay(request.node.nodeid, cursor, list(command[1:]),
                                            json.loads(kwargs["input"]))
                code, stdout, stderr = 0, json.dumps(value, ensure_ascii=False), ""
            except RuntimeError as error:
                code, stdout, stderr = 1, "", str(error)
            if not (kwargs.get("text") or kwargs.get("encoding") or kwargs.get("universal_newlines")):
                stdout, stderr = stdout.encode(), stderr.encode()
            if code and kwargs.get("check"):
                raise subprocess.CalledProcessError(code, command, output=stdout, stderr=stderr)
            return subprocess.CompletedProcess(command, code, stdout=stdout, stderr=stderr)
        return original(command, *args, **kwargs)

    # Keep strict production serialization and parsing. Only the test process's
    # stub executable is doubled; child workers inherit its actual replay script.
    monkeypatch.setattr(subprocess, "run", recorded)
    yield
    script = checker_stub.cases().get(request.node.nodeid, [])
    consumed = int(cursor.read_text()) if cursor.exists() else 0
    assert consumed == len(script), f"unconsumed Rust checker steps in {request.node.nodeid}: {consumed}/{len(script)}"
