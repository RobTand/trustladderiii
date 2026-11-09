"""Shared worker clients and PrismaBuild fixtures. No production models."""

from copy import deepcopy
from dataclasses import dataclass
import json
import os
from pathlib import Path
import subprocess
import sys

import httpx
import pytest

from tl3_workers.forcing_rehearsal import ForcingRehearsals
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
    fake = shared.FAKES[str(scripts)]

    def run(staged, document):
        # The real staged runner runs the selected tests on the staged checkout.
        staged = Path(staged)
        selected = document["arguments"][0]
        run = subprocess.run([sys.executable, str(staged / "trusted/tl3_harness/runner.py"),
                              selected, str(staged / "checkout"), "--summary"], capture_output=True, text=True)
        assert run.returncode in (0, 1, 4, 5), run.stdout + run.stderr
        sys.path.insert(0, str(staged / "trusted"))
        try:
            from tl3_harness.test_tool import Summary
        finally:
            sys.path.remove(str(staged / "trusted"))
        summary = Summary()
        for line in run.stdout.splitlines():
            record = json.loads(line)
            if record["event"] == "result":
                summary.add(record["test"], record["outcome"], record["message"])
        if (staged / "proof.json").is_file():
            summary.result["proof"] = json.loads((staged / "proof.json").read_text())
        (scripts / "response.json").write_text(json.dumps(summary.result))
        record = scripts / "submissions.json"
        submissions = json.loads(record.read_text()) if record.exists() else []
        submissions.append(json.loads(selected))
        record.write_text(json.dumps(submissions))
        (scripts / "submitted.json").write_text(json.dumps(document))
        return "tests", summary.result, []
    fake.job = run
    return base, head, before, git


def invoke(submission, response, arguments):
    root, scripts = submission
    (scripts / "response.json").write_text(json.dumps(response))
    with Worktree(root) as tree:
        tree.test_settings = shared.flux_settings(cpus=1, mem_gb=2)
        tool = build_tools(tree, {"run_tests": "Run worktree tests on the admitted test runner."})[0]
        return tool.invoke(arguments), tree


def submitted(submission):
    return json.loads((submission[1] / "submitted.json").read_text())


@pytest.fixture
def generator_backend(tmp_path, monkeypatch):
    """Real generator action in an admitted Flux job, behind A+B's Flux wire fake.

    Fixture boundaries only: the Rust backend operations (in-process, enforcing launch
    intent before Podman, bind before payload, qualification before the gate) and the
    Podman executable (refuses unowned launches, emulates the trusted gate, then runs
    the configured argv in the mounted private copy). Admission, allocation, scratch,
    Supervised, cleanup and artifact validation are real.
    """
    podman_source = """#!{python}
import hashlib, json, os, pathlib, sys, time
time.sleep({latency!r})
args = sys.argv[1:]
if args[:1] == ['run']:
    if '--name' not in args or '--label' not in args:
        sys.exit(125)
    name = args[args.index('--name') + 1]
    labels = dict(args[i + 1].split('=', 1) for i in range(len(args) - 1) if args[i] == '--label')
    mounts = [args[i + 1] for i in range(len(args) - 1) if args[i] == '--mount']
    env = dict(args[i + 1].split('=', 1) for i in range(len(args) - 1) if args[i] == '--env')
    pathlib.Path({root!r}, name + '.json').write_text(json.dumps({{
        'Id': hashlib.sha256(name.encode()).hexdigest(), 'Name': name, 'Labels': labels,
        'Pid': os.getpid(), 'ConmonPid': os.getpid()}}))
    status = pathlib.Path('/proc/self/status').read_text()
    allowed = next(line.split(':', 1)[1].strip() for line in status.splitlines()
                   if line.startswith('Cpus_allowed_list:'))
    print('TL3_RUNTIME_EVIDENCE ' + json.dumps({{'Cpus_allowed_list': allowed, 'cgroup': '0::/fixture\\n'}}),
          file=sys.stderr, flush=True)
    if os.read(0, 1) != b'\\x01':
        sys.exit(125)
    payload = args[args.index({image!r}) + 1:]
    if payload[:5] != ['-I', '-B', '/harness/runtime_evidence.py', '--exec', '--'] or len(payload) < 6:
        sys.exit(126)
    [checkout] = [m for m in mounts if m.startswith('type=bind,src=') and m.endswith(',dst=/checkout')]
    os.chdir(checkout[len('type=bind,src='):-len(',dst=/checkout')])
    argv = payload[5:]
    executable = {{'python3': sys.executable}}.get(argv[0], argv[0])
    os.execve(executable, [executable, *argv[1:]], {{'PATH': '/usr/bin:/bin', **env}})
if args[:2] == ['container', 'inspect']:
    print(pathlib.Path({root!r}, args[-1] + '.json').read_text())
    sys.exit(0)
sys.exit(1 if args[:2] == ['container', 'exists'] else 0)
"""
    from tl3_harness import runtime, scratch
    root = tmp_path / "flux-generator"
    (root / "bin").mkdir(parents=True)
    for name in ("crun", "catatonit"):
        (root / "bin" / name).write_text("#!/bin/sh\nexit 0\n")
        (root / "bin" / name).chmod(0o700)
    podman = root / "podman"
    # Test-only contention model (FX-TEST-BUDGETS convention): every runtime-client call waits first.
    latency = float(os.environ.get("FX_TEST_ACTION_LATENCY_SECONDS", "0"))
    assert 0 <= latency < float("inf"), "FX_TEST_ACTION_LATENCY_SECONDS must be non-negative seconds"
    podman.write_text(podman_source.format(python=sys.executable, root=str(root), image=IMAGE, latency=latency))
    podman.chmod(0o700)
    (root / "xdg").mkdir(mode=0o700)
    (root / "output").mkdir()
    (root / "scratch").mkdir(mode=0o700)
    monkeypatch.setenv("PATH", f"{root / 'bin'}{os.pathsep}{os.environ.get('PATH', '/usr/bin:/bin')}")
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(root / "xdg"))
    monkeypatch.setenv("TL3_EXECUTION_BACKEND", "flux")
    context = {
        "run": {"backend": "flux", "uri": "local:///fixture", "instance": "run/generation", "job": "fJob"},
        "owner": "owner", "correlation": "correlation", "request_digest": "f" * 64,
        "output_dir": str(root / "output"),
        "progress": {"journal": str(root / "output" / "progress.json"), "no_progress_seconds": 5},
        "scratch": {"approved_root": str(root / "scratch"), "leaf": str(root / "scratch" / "fJob")},
        "native_threads": 1,
        "runtime": {"mode": "in_job", "image": IMAGE, "executable": str(podman),
                    "trusted_root": str(Path(runtime.__file__).resolve().parent), "interpreter": "python3"},
        "runtime_identity": {"cleanup": {"fixture": "pinned cleanup helper"}},
        "placement": {"kind": "observed", "allocation": {"fixture": 1}, "topology": {"fixture": 1},
                      "host": os.uname().nodename, "rank": 0, "os_pus": sorted(os.sched_getaffinity(0))},
    }
    launches, ops = {}, []

    def backend(request, **kwargs):
        op = request["op"]
        ops.append(op)
        if op == "observe_allocation":
            return {"facts": {"observed": request["observed"]}}
        if op == "validate_allocation":
            placement = context["placement"]
            return {"grant": {"run": context["run"], "request_digest": context["request_digest"],
                              **{key: placement[key] for key in ("allocation", "topology", "host", "rank", "os_pus")}}}
        if op == "begin_container_launch":
            launcher = request["launcher"]
            assert launcher["execution"] not in launches, "one durable intent per execution"
            name = "tl3-" + launcher["execution"]
            launches[launcher["execution"]] = {
                "launcher": launcher, "name": name, "mode": "in_job",
                "labels": {"tl3.backend": "flux", "tl3.execution": launcher["execution"], "tl3.owner": "owner"}}
            return {"launch": launches[launcher["execution"]]}
        if op == "bind_container_launch":
            launch = launches[request["observed"]["execution"]]
            assert request["observed"]["container"]["name"] == launch["name"]
            return {"binding": {"intent": launch, "ownership": request["observed"], "payload_allowed": True}}
        if op == "validate_runtime_binding":
            assert request["facts"]["execution"] in launches and "bind_container_launch" in ops
            return {"qualification": {"qualified": True}}
        if op == "cleanup_runtime":
            return {"cleanup": {"stopped": True, "evidence": []}}
        raise AssertionError(f"unexpected Rust operation {op}")

    monkeypatch.setattr(runtime, "launch_context", lambda: context)
    monkeypatch.setattr(runtime, "backend_request", backend)
    monkeypatch.setattr(runtime, "_uncertain_containers", set())
    monkeypatch.setattr(scratch, "_owned", {})
    jobs = []

    def flux_job(staged, document):
        # Like native Flux, each job owns a fresh output directory and scratch leaf.
        job = root / "jobs" / str(len(jobs))
        (job / "output").mkdir(parents=True)
        (job / "scratch").mkdir(mode=0o700)
        jobs.append(job)
        context.update(output_dir=str(job / "output"),
                       progress={"journal": str(job / "output" / "progress.json"), "no_progress_seconds": 5},
                       scratch={"approved_root": str(job / "scratch"), "leaf": str(job / "scratch" / "fJob")})
        return shared.generator_job(staged, document)

    # The central Flux fake replaces only the Flux wire; its payload, receipt and
    # import checks run. calls lists each new submission; artifacts its payloads.
    fake = shared.fake_for(tmp_path, monkeypatch)
    fake.job = flux_job
    calls, artifacts = fake.prepared, fake.payloads
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
    inputs = {'base_sha': base, 'tests': {'image': IMAGE, 'execution': dict(shared.FLUX_EXECUTION)},
              'must_split': False, 'findings': [], 'protected_paths': [],
              'ingest': {'brief': 'Preserve adoption, no copying and no publication wait.',
                         'goals': [{'description': 'Make the adoption test deterministic.', 'verification': 'test'}]},
              'forecast': {'fit_tokens': 100000, 'context_tokens': 150000, 'overhead_tokens': {'tools': 1}}}
    calls = []
    fake = shared.fake_for(tmp_path, monkeypatch)

    def submit(staged, document):
        calls.append(document)
        # The Flux fake drives the real staged harness and runner on free remote compute.
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
        child = subprocess.run([sys.executable, '-c', script, json.dumps([IMAGE, *document['arguments']])],
                               cwd=staged, capture_output=True, text=True, check=True)
        return shared.action_payload(child.stdout)
    fake.job = submit
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
