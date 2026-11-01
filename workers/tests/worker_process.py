"""A supervisor-shaped test client. It never selects a production model."""

from dataclasses import dataclass
import json
import os
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
FakeMessagesListChatModel.bind_tools = lambda self, tools, **kwargs: self
model = FakeMessagesListChatModel(responses=[AIMessage(**turn) for turn in script])
mode = sys.argv[2] if len(sys.argv) > 2 else ''
def create_model(request):
    if mode == 'quota':
        import httpx
        from openai import RateLimitError
        response = httpx.Response(429, headers={'retry-after': '600'}, request=httpx.Request('POST', 'http://model.invalid/v1'))
        raise RateLimitError('secret provider text', response=response, body=None)
    if mode == 'fail-with-secret':
        raise RuntimeError('fixture-secret-value ' + request['key_file'])
    if mode == 'noisy':
        import os, subprocess
        os.write(1, b'stray fd write\\n')
        print('stray print')
        sys.__stdout__.write('stray dunder write\\n')
        sys.__stdout__.flush()
        subprocess.run([sys.executable, '-c', "print('stray child')"], check=True)
    return model
def no_network(*args, **kwargs):
    raise AssertionError('network forbidden in worker tests')
with patch.object(runtime, 'create_model', create_model), \\
     patch.object(socket.socket, 'connect', no_network), \\
     patch.object(socket.socket, 'connect_ex', no_network), \\
     patch.object(socket, 'getaddrinfo', no_network):
    sys.argv = ['tl3_workers', 'run']
    runpy.run_module('tl3_workers', run_name='__main__')
"""


def start_worker(tmp_path, request, turns, *, raw=None, mode=None, env_overrides=None):
    script = tmp_path / "scripted-model.json"
    script.write_text(json.dumps(turns))
    env = os.environ.copy()
    env.update({"LANGSMITH_TRACING": "false", "LANGCHAIN_TRACING_V2": "false"})
    env.update(env_overrides or {})
    process = subprocess.run(
        [sys.executable, "-c", LAUNCH, str(script)] + ([mode] if mode else []),
        input=(json.dumps(request) if raw is None else raw).encode("utf-8"),
        capture_output=True, env=env, timeout=45,
    )
    events = [json.loads(line) for line in process.stdout.decode("utf-8").splitlines()]
    return Result(process.returncode, events, process.stderr.decode("utf-8", "replace"))


def run_real_entrypoint(request, *, raw=None):
    return subprocess.run(
        [sys.executable, "-m", "tl3_workers", "run"],
        input=json.dumps(request) if raw is None else raw,
        text=True, capture_output=True, timeout=45,
    )


def fill_visible_fields(monkeypatch, fields):
    """Model the fields advertised by the schema, including optional evidence."""
    setup = '''
class SchemaFollowingModel(FakeMessagesListChatModel):
    def _generate(self, messages, **kwargs):
        result = super()._generate(messages, **kwargs)
        message = result.generations[0].message
        reply = json.loads(message.content)
        prompt = next(m.content for m in messages if m.type == 'system')
        proposal = json.loads(prompt.rsplit('Reply with one JSON object matching this schema:\\n', 1)[1])
        variant = next(v for v in proposal['oneOf']
                       if v['properties']['kind']['const'] == reply['kind'])
        for name, value in FIELDS.items():
            if name in variant['properties']:
                reply[name] = value
        message.content = json.dumps(reply)
        return result
'''.replace('FIELDS', repr(fields))
    monkeypatch.setattr(__import__(__name__), 'LAUNCH', LAUNCH.replace(
        'model = FakeMessagesListChatModel(', setup + '\nmodel = SchemaFollowingModel('))
