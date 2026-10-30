"""A supervisor-shaped test client. It never selects a production model."""

from dataclasses import dataclass
import json
import os
from pathlib import Path
import subprocess
import sys


@dataclass
class Result:
    exit_code: int
    events: list[dict]
    stderr: str


# Only the model constructor changes. The installed module's main, graph,
# request validation, stream and reply validation run in the child process.
LAUNCH = """
import json
from pathlib import Path
import runpy
import socket
import sys
from unittest.mock import patch
from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage
from tl3_workers import runtime

script = json.loads(Path(sys.argv[1]).read_text())
model = FakeMessagesListChatModel(responses=[AIMessage(**turn) for turn in script])
def no_network(*args, **kwargs):
    raise AssertionError('network forbidden in worker tests')
with patch.object(runtime, 'create_model', return_value=model), \\
     patch.object(socket.socket, 'connect', no_network), \\
     patch.object(socket.socket, 'connect_ex', no_network), \\
     patch.object(socket, 'getaddrinfo', no_network):
    sys.argv = ['tl3_workers', 'run']
    runpy.run_module('tl3_workers', run_name='__main__')
"""


def start_worker(tmp_path, request, turns, *, raw=None):
    script = tmp_path / "scripted-model.json"
    script.write_text(json.dumps(turns))
    env = os.environ.copy()
    env.update({"LANGSMITH_TRACING": "false", "LANGCHAIN_TRACING_V2": "false"})
    process = subprocess.run(
        [sys.executable, "-c", LAUNCH, str(script)],
        input=json.dumps(request) if raw is None else raw,
        text=True, capture_output=True, env=env, timeout=45,
    )
    events = [json.loads(line) for line in process.stdout.splitlines()]
    return Result(process.returncode, events, process.stderr)


def run_real_entrypoint(request, *, raw=None):
    return subprocess.run(
        [sys.executable, "-m", "tl3_workers", "run"],
        input=json.dumps(request) if raw is None else raw,
        text=True, capture_output=True, timeout=45,
    )
