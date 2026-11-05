"""Shared worker clients and PrismaBuild fixtures. No production models."""

from copy import deepcopy
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

import httpx
import pytest

from tl3_workers.forcing_rehearsal import ForcingRehearsals
from tl3_workers.pb_tests import TestSettings
from tl3_workers.roles import planner
from tl3_workers.tools import Worktree, build_tools
import pb_fixtures as shared


IMAGE = shared.IMAGE
KEY = shared.KEY
call = shared.call
configured_generator = shared.configured_generator
generator_repo = shared.generator_repo
submission = shared.submission


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


class _EntryOs:
    """The os module as tl3_workers.__main__ sees it, minus main's fd 1 swap.

    In a child, main keeps fd 1 for protocol events and points fd 1 at stderr.
    In-process, the events go to memory and the test process keeps its fds.
    """

    def __init__(self, output):
        self._output = output

    def __getattr__(self, name):
        return getattr(os, name)

    def dup(self, fd):
        assert fd == 1, fd
        return fd

    def fdopen(self, fd, mode):
        assert (fd, mode) == (1, "wb"), (fd, mode)
        return self._output

    def dup2(self, source, target):
        assert (source, target) == (2, 1), (source, target)


def bind_tools(model, tools, **kwargs):
    """Tool binding for the in-process fake; fixtures may replace it."""
    return model


def start_worker_in_process(tmp_path, request, turns, *, raw=None, mode=None, env_overrides=None):
    """start_worker for cases whose assertion is a decision, not the process boundary.

    It runs the same installed main (request validation, graph, stream, reply
    validation) with the same scripted model, without a fresh interpreter. Tests
    of the stdout protocol, stray fd writes, the kill checkpoint or the exporter
    keep start_worker.
    """
    import io
    import socket
    from unittest.mock import patch

    from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
    from langchain_core.messages import AIMessage

    import tl3_workers.__main__ as entry
    from tl3_workers import runtime

    assert mode in (None, "quota", "fail-with-secret"), "this mode tests the process boundary"
    # The child reads the script back from JSON; so does this.
    script = json.loads(json.dumps(turns))
    model = FakeMessagesListChatModel(responses=[AIMessage(**turn) for turn in script])

    def create_model(request):
        if mode == "quota":
            from openai import RateLimitError
            response = httpx.Response(429, headers={"retry-after": "600"},
                                      request=httpx.Request("POST", "http://model.invalid/v1"))
            raise RateLimitError("secret provider text", response=response, body=None)
        if mode == "fail-with-secret":
            raise RuntimeError("fixture-secret-value " + request["key_file"])
        return model

    def no_network(*args, **kwargs):
        raise AssertionError("network forbidden in worker tests")

    payload = (json.dumps(request) if raw is None else raw).encode("utf-8")
    output, errors = io.BytesIO(), io.StringIO()
    stdin = io.TextIOWrapper(io.BytesIO(payload), encoding="utf-8")
    environment = {"LANGSMITH_TRACING": "false", "LANGCHAIN_TRACING_V2": "false", **(env_overrides or {})}
    cwd, stdout = os.getcwd(), sys.stdout
    try:
        with patch.dict(os.environ, environment), \
             patch.object(entry, "os", _EntryOs(output)), \
             patch.object(sys, "argv", ["tl3_workers", "run"]), \
             patch.object(sys, "stdin", stdin), \
             patch.object(sys, "stderr", errors), \
             patch.object(FakeMessagesListChatModel, "bind_tools",
                          lambda self, tools, **kwargs: bind_tools(self, tools, **kwargs)), \
             patch.object(runtime, "create_model", create_model), \
             patch.object(socket.socket, "connect", no_network), \
             patch.object(socket.socket, "connect_ex", no_network), \
             patch.object(socket, "getaddrinfo", no_network):
            exit_code = entry.main()
    finally:
        # main points sys.stdout at stderr and enters the worktree, as in a child.
        sys.stdout = stdout
        os.chdir(cwd)
    events = [json.loads(line) for line in output.getvalue().decode("utf-8").splitlines()]
    return Result(exit_code, events, errors.getvalue())


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
        proposal = json.loads(prompt.rsplit('Reply with one JSON object only: no prose, no code fences. It must match this schema:\\n', 1)[1])
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


def prepare_base_submission(submission, base_value="base", head_value="head"):
    root, scripts = submission

    def git(*args):
        return subprocess.check_output([
            "git", "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false",
            "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", *args,
        ], cwd=root, text=True).strip()

    git("init", "--quiet", "--template=")
    (root / "pyproject.toml").write_text(
        '[tool.tl3.harness]\ncommand=["python", "-m", "pytest"]\ntest-files=["test_*.py"]\n')
    (root / "example_module.py").write_text(f"value = {base_value!r}\n")
    test_source = ("from example_module import value\ndef test_value():\n"
                   "    if value == 'error': raise RuntimeError('fixture error')\n"
                   "    assert value == 'head'\n"
                   "def test_other(): assert value == 'head'\ndef test_pass(): pass\n")
    if base_value != "missing":
        (root / "test_code.py").write_text(test_source)
    git("add", ".")
    git("commit", "--quiet", "-m", "Check base")
    base = git("rev-parse", "HEAD")
    (root / "test_code.py").write_text(test_source)
    (root / "example_module.py").write_text("value = 'earlier maker commit'\n")
    git("add", ".")
    git("commit", "--quiet", "-m", "Earlier maker round")
    head = git("rev-parse", "HEAD")
    (root / "example_module.py").write_text(f"value = {head_value!r}\n")
    before = git("status", "--porcelain")
    script = scripts / "pbrun.py"
    script.write_text(script.read_text().replace(
        'assert (root / "checkout/uncommitted.py").read_bytes() == b"current dirty bytes\\x00\\xff"\n', "",
    ).replace(
        'print(json.dumps({"schema":',
        '''import subprocess
sys.path.insert(0, str(root / "trusted"))
from tl3_harness.test_tool import Summary
selected = args[args.index("--") + 6]
run = subprocess.run([sys.executable, str(root / "trusted/tl3_harness/runner.py"),
                      selected, str(root / "checkout"), "--summary"], capture_output=True, text=True)
assert run.returncode in (0, 1, 4, 5), run.stdout + run.stderr
summary = Summary()
for line in run.stdout.splitlines():
    record = json.loads(line)
    if record["event"] == "result":
        summary.add(record["test"], record["outcome"], record["message"])
if (root / 'proof.json').is_file():
    summary.result['proof'] = json.loads((root / 'proof.json').read_text())
(here / "response.json").write_text(json.dumps(summary.result))
record = here / "submissions.json"
submissions = json.loads(record.read_text()) if record.exists() else []
submissions.append(json.loads(selected))
record.write_text(json.dumps(submissions))
print(json.dumps({"schema":''',
    ).replace('"action_key": "a" * 64', '"action_key": ("a" if len(submissions) == 1 else "c") * 64'))
    waiter = scripts / "pbwait.py"
    waiter.write_text(waiter.read_text().replace('"action_key": "a" * 64', '"action_key": sys.argv[-1]'))
    return base, head, before, git


def invoke(submission, response, arguments):
    root, scripts = submission
    (scripts / "response.json").write_text(json.dumps(response))
    with Worktree(root) as tree:
        tree.test_settings = TestSettings(IMAGE, scripts, cpus=1, mem_gb=2)
        tool = build_tools(tree, {"run_tests": "Run worktree tests through PrismaBuild."})[0]
        return tool.invoke(arguments), tree


def submitted(submission):
    return json.loads((submission[1] / "submitted.json").read_text())


@pytest.fixture
def generator_backend(tmp_path, monkeypatch):
    # Exercise real git-index generation and artifact validation. Only the fleet
    # transport and Docker launch are replaced; these tests themselves run remotely.
    from tl3_harness import generator
    from tl3_workers import pb_generators
    monkeypatch.setattr(generator, "require_admission", lambda: None)
    monkeypatch.setattr(generator, "container_command", lambda tree, container, argv, cidfile: argv)
    calls = []
    artifacts = {}

    def command(args):
        if args[1].endswith("pbrun.py"):
            root = Path(args[args.index("--cwd") + 1])
            contract = json.loads((root / "generator.json").read_text())
            result = generator.run(root / "checkout", generator.Container(IMAGE), **contract)
            raw = (generator.PREFIX + json.dumps(result) + "\n").encode()
            log = tmp_path / f"generator-{len(calls)}.log"
            log.write_bytes(raw)
            artifacts[KEY] = {"action_key": KEY, "status": "executed", "exit_code": 0,
                              "diagnostics": {"elapsed_s": 123.5},
                              "succeeded": True, "result": {"verified": True,
                              "receipt_sha256": "b" * 64, "stdout": {
                                  "path": str(log), "bytes": len(raw),
                                  "sha256": hashlib.sha256(raw).hexdigest()}}}
            calls.append(args)
            return 0, {"schema": "prismabuild.submission.v1", "action_key": KEY}
        return 0, {"schema": "prismabuild.endings.v1", "complete": True,
                   "endings": [artifacts[KEY]]}

    monkeypatch.setattr(pb_generators, "_json_command", command)
    return calls, artifacts


NODE = 'tests/test_adoption.py::test_adopt'


FORCING = {'symptom': 'Publishing waits for a slow decision.', 'environment': {},
           'patches': [{'target': 'publisher.Publisher.decide', 'delay_seconds': 0.001}]}


SOURCE = '''
class Publisher:
    def decide(self):
        return True

    def try_adopt(self):
        return True

    def publish(self):
        if self.try_adopt():
            return True
        return self.decide()
'''


TEST = '''
from publisher import Publisher
import time

def test_adopt():
    start = time.monotonic()
    assert Publisher().publish()
    assert time.monotonic() - start < 0.5
'''


# Rehearsal tests exercise execution and custody, not pytest's diagnostic
# rendering. Keep real exceptions, outcomes, forcing hits and source receipts;
# give their display reports stable text instead of clock values and addresses.
REHEARSAL_RUNNER = '''
import pytest
import runpy
import sys
from pathlib import Path

sys.path.insert(0, str(Path(sys.argv[1]).parent))
import red_message

class StableReports:
    @pytest.hookimpl(wrapper=True, trylast=True)
    def pytest_runtest_makereport(self, item, call):
        report = yield
        if report.failed:
            assertion = red_message.rewritten_assertion(call.excinfo.value)
            message = (red_message.assertion_reason(call.excinfo.value, assertion)
                       if assertion and assertion[0].msg is not None else '')
            report.longrepr = f'{item.nodeid}: {call.when}: {call.excinfo.typename}: {message}'
        return report

    @pytest.hookimpl(wrapper=True, trylast=True)
    def pytest_make_collect_report(self, collector):
        report = yield
        if report.failed:
            report.longrepr = f'{collector.nodeid}: collection failed'
        return report

original = pytest.main
pytest.main = lambda args, plugins: original(args, plugins=[StableReports(), *plugins])
sys.argv = sys.argv[1:]
runpy.run_path(sys.argv[0], run_name='__main__')
'''


def git(root, *args):
    return subprocess.run(['git', '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                           *args], cwd=root, check=True, capture_output=True, text=True).stdout.strip()


@pytest.fixture
def rehearsal(tmp_path, monkeypatch):
    root = tmp_path / 'repo'
    (root / 'tests').mkdir(parents=True)
    (root / 'publisher.py').write_text(SOURCE)
    (root / 'tests/test_adoption.py').write_text(TEST)
    (root / 'pyproject.toml').write_text('[tool.tl3.harness]\ntest-files=["tests/*.py"]\n')
    git(root, 'init', '-q')
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'base')
    base = git(root, 'rev-parse', 'HEAD')
    inputs = {'base_sha': base, 'tests': {'image': IMAGE, 'tools_path': str(tmp_path)},
              'must_split': False, 'findings': [], 'protected_paths': [],
              'ingest': {'brief': 'Preserve adoption, no copying and no publication wait.',
                         'goals': [{'description': 'Make the adoption test deterministic.', 'verification': 'test'}]},
              'forecast': {'fit_tokens': 100000, 'context_tokens': 150000, 'overhead_tokens': {'tools': 1}}}
    calls = []
    from tl3_workers import pb_tests

    def submit(command):
        if 'pbrun.py' in command[1]:
            calls.append(command)
            staged = Path(command[command.index('--cwd') + 1])
            # A fake PB transport drives the real staged harness and runner on free remote compute.
            # Admission/container and display seams are fixtures; served code still runs.
            script = '''
import sys
from pathlib import Path
sys.path.insert(0, str(Path('trusted').resolve()))
from tl3_harness import test_tool
from tl3_harness import execution
import json
execution.require_admission = test_tool.require_admission = lambda: None
import os
os.environ['PRISMABUILD_ACTION_PROGRESS_PATH'] = str(Path('progress.json').resolve())
os.environ['PRISMABUILD_ACTION_PROGRESS_TOKEN'] = 'fixture'
test_tool.container_command = lambda tree, container, tests: [sys.executable, '-I', '-B', '-c', RUNNER,
    str(Path(execution.__file__).with_name('runner.py')), '-', str(tree)]
sys.argv = ['action.py', *json.loads(sys.argv[1])]
test_tool.main()
'''
            script = script.replace('RUNNER', repr(REHEARSAL_RUNNER))
            args = command[command.index('action.py') + 1:]
            child = subprocess.run([sys.executable, '-c', script, json.dumps(args)], cwd=staged,
                                   capture_output=True, text=True, check=True)
            log = tmp_path / 'action.log'
            log.write_text(child.stdout)
            return 0, {'schema': 'prismabuild.submission.v1', 'action_key': 'a' * 64}
        import hashlib
        log = tmp_path / 'action.log'
        raw = log.read_bytes()
        return 0, {'schema': 'prismabuild.endings.v1', 'complete': True, 'endings': [
            {'action_key': 'a' * 64, 'status': 'executed', 'exit_code': 0, 'succeeded': True,
             'result': {'verified': True, 'receipt_sha256': 'b' * 64,
                        'stdout': {'path': str(log), 'bytes': len(raw),
                                   'sha256': hashlib.sha256(raw).hexdigest()}}}]}
    monkeypatch.setattr(pb_tests, '_json_command', submit)
    monkeypatch.setattr(planner.Forecaster, 'measure', lambda *a: {'fits': True})
    monkeypatch.chdir(root)
    with Worktree(root) as tree:
        yield ForcingRehearsals(tree, inputs), root, inputs, calls


def plan(forcing=FORCING):
    return {'kind': 'leaf', 'estimated_exploration_turns': 0,
            'files': ['tests/test_adoption.py'], 'edits_tests': [NODE],
            'checks': [{'proof': {'subjects': [], 'observer_support': [], 'red_message': 'expected regression'}, 'kind': 'demonstrating', 'node_ids': [NODE], 'goal_ids': [0],
                        'forcing': deepcopy(forcing)}], 'no_design_questions': True, 'lands_on_its_own': True}


def reachable():
    result = deepcopy(FORCING)
    result['patches'] = [{'target': 'publisher.Publisher.try_adopt', 'delay_seconds': 1}]
    return result


def response_frames(content, *, duplicate_usage=False, trailing_usage=None, tool_calls=None):
    chunk = {"id": "fixture-completion", "object": "chat.completion.chunk", "created": 0,
             "model": "fixture-model"}
    usage = {"prompt_tokens": 1000, "completion_tokens": 5, "total_tokens": 1005,
             "prompt_tokens_details": {"cached_tokens": 800, "cache_write_tokens": 100}}
    delta = {"role": "assistant", "content": content}
    if tool_calls is not None:
        delta["tool_calls"] = tool_calls
    frames = [{**chunk, "choices": [{"index": 0, "delta": delta,
                                    "finish_reason": None}]},
              {**chunk, "choices": [{"index": 0, "delta": {},
                                    "finish_reason": "tool_calls" if tool_calls else "stop"}], "usage": usage}]
    if duplicate_usage or trailing_usage is not None:
        frames.append({**chunk, "choices": [], "usage": trailing_usage or usage})
    stream = "".join("data: " + json.dumps(frame) + "\n\n" for frame in frames) + "data: [DONE]\n\n"
    return httpx.Response(200, headers={"content-type": "text/event-stream"}, content=stream)


BRIEF = {
    "kind": "brief", "brief": "Fix the greeting without changing its interface.",
    "goals": [
        {"description": "Return the requested greeting.", "verification": "test"},
        {"description": "Keep the public interface clear.", "verification": "review"},
    ],
    "memory_ids": [],
}

AGREE = {"verdict": "agree", "objections": []}


@pytest.fixture
def request_doc(tmp_path):
    return {
        "protocol": 1,
        "operation": {"node": 1, "generation": 0, "kind": "ingest"},
        "role": "ingest",
        "model": {"base_url": "http://model.invalid/v1", "name": "fixture-model",
                  "effort": "high", "budget_tokens": 32000},
        "key_file": str(tmp_path / "never-read-this-key"),
        "inputs": {
            "issue": {"number": 1, "title": "Fix greeting", "body": "Return hello.",
                      "author": "requester", "comments": []},
            "repo": {"readme": "A greeting library.", "file_list": ["greeting.py"]},
        },
        "worktree": str(tmp_path),
    }
