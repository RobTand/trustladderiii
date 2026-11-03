"""The maker's retrospective: one tool-free turn after its accepted reply (#648)."""

from copy import deepcopy
from dataclasses import asdict, replace
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import threading
from types import SimpleNamespace

import httpx
from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage, HumanMessage
import openai
import pytest

from tl3_workers import runtime
from tl3_workers.roles import maker

# Local copies of test_maker_budget's maker request and test_tracing's receiver: a test module that imports another test module adds a
# test_to_test edge, which the Gate's graph-impact check refuses.
def git(root, *args):
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    env.update({
        "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_AUTHOR_NAME": "Fixture", "GIT_AUTHOR_EMAIL": "fixture@example.invalid",
        "GIT_COMMITTER_NAME": "Fixture", "GIT_COMMITTER_EMAIL": "fixture@example.invalid",
    })
    return subprocess.run(
        ["git", "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false", *args],
        cwd=root, env=env, check=True, capture_output=True, text=True,
    ).stdout.strip()


@pytest.fixture
def maker_request(tmp_path):
    root = tmp_path / "repo"
    root.mkdir()
    git(root, "init", "--quiet", "--template=")
    (root / "greeting.py").write_text("def greet(): return 'goodbye'\n")
    git(root, "add", ".")
    git(root, "commit", "--quiet", "-m", "Base")
    return {
        "protocol": 1,
        "operation": {"node": 1, "generation": 0, "kind": "make"},
        "role": "maker",
        "model": {"base_url": "http://model.invalid/v1", "name": "fixture-model",
                  "effort": "high", "budget_tokens": 32000},
        "key_file": str(tmp_path / "unused-key"),
        "worktree": str(root),
        "inputs": {
            "brief": "Return hello without changing greet's interface.",
            "goals": [{"id": "greeting", "description": "Return hello.", "verification": "test"},
                      {"id": "interface", "description": "Keep greet's interface.", "verification": "review"}],
            "plan": {
                "files": ["greeting.py", "test_greeting.py"],
                "checks": [
                    {"name": "hello", "kind": "demonstrating",
                     "tests": ["test_greeting.py::test_hello"], "goal_ids": ["greeting"]},
                    {"name": "signature", "kind": "behavior-preserving",
                     "tests": ["test_greeting.py::test_signature"], "goal_ids": ["interface"]},
                ],
            },
            "attempt": 1, "round": 0,
            "tests": {"image": "sha256:" + "a" * 64,
                      "tools_path": "/mnt/shared/prismabuild-fleet/repo/tools"},
            "context_size": 1000,
        },
    }


@pytest.fixture
def receiver():
    class Packets(list):
        block = False
        started = threading.Event()
        release = threading.Event()

    packets = Packets()

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            if packets.block:
                packets.started.set()
                packets.release.wait(15)
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b'{"experiment":{"experiment_id":"12"}}')

        def do_POST(self):
            packets.append((self.path, self.headers, self.rfile.read(int(self.headers['Content-Length']))))
            self.send_response(getattr(packets, 'tag_status', 200)
                               if self.path.endswith('set-experiment-tag') else 200)
            self.end_headers()
            self.wfile.write(b'{}')

        def log_message(self, *args):
            pass

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    yield f'http://127.0.0.1:{server.server_port}', packets
    packets.release.set()
    server.shutdown()
    server.server_close()
    thread.join()


@pytest.fixture
def exporters(monkeypatch):
    children = []
    popen = subprocess.Popen

    def launch(*args, **kwargs):
        child = popen(*args, **kwargs)
        command = child.args
        if (isinstance(command, (list, tuple)) and len(command) > 2
                and command[2] == 'ship' and Path(command[1]).name == 'tracing.py'):
            children.append(child)
        return child

    monkeypatch.setattr(subprocess, 'Popen', launch)
    yield children
    for child in children:
        if child.poll() is None:
            child.kill()
        child.wait(timeout=15)


def exported_spans(packets):
    from mlflow.entities import Span
    from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import ExportTraceServiceRequest
    assert packets, "worker did not export a trace"
    request = ExportTraceServiceRequest.FromString(packets[-1][2])
    return [Span.from_otel_proto(span) for span in request.resource_spans[0].scope_spans[0].spans]

CANT_MEET = {"kind": "cant_meet", "report": "The plan needs a file it does not declare."}
RETRO = {"slowed_by": ["Three reads of greeting.py repeated the same lines."],
         "missing": ["The brief did not name the test runner image."],
         "checks": ["A lint for unused imports would have caught my first edit."],
         "noise": []}
WORK_TOKENS, RETRO_TOKENS = 100, 7_777


class RetroModel(FakeMessagesListChatModel):
    """Answers the maker's first turn with its final reply, then the retro turn."""
    calls: list = []
    offered: set = set()
    final_reply: dict = CANT_MEET
    work_turns: int = 1
    retro: object = RETRO
    hold: object = None  # an Event the retro turn waits on, for at most 8 seconds
    holding: bool = False  # true while the retro turn waits on hold

    def bind_tools(self, tools, **kwargs):
        self.offered = {tool.name for tool in tools}
        return self

    def _generate(self, messages, **kwargs):
        index = len(self.calls)
        self.calls.append((deepcopy(messages), set(self.offered)))
        self.offered = set()
        if index < self.work_turns - 1:
            message = AIMessage(content="", tool_calls=[{"name": "read", "args": {"path": "greeting.py"},
                                                         "id": str(index), "type": "tool_call"}],
                                usage_metadata={"input_tokens": WORK_TOKENS, "output_tokens": 1,
                                                "total_tokens": WORK_TOKENS + 1})
        elif index == self.work_turns - 1:
            message = AIMessage(content=json.dumps(self.final_reply), usage_metadata={
                "input_tokens": WORK_TOKENS, "output_tokens": 1, "total_tokens": WORK_TOKENS + 1})
        else:
            if self.hold is not None:
                self.holding = True
                self.hold.wait(8)
                self.holding = False
            if isinstance(self.retro, BaseException):
                raise self.retro
            text = self.retro if isinstance(self.retro, str) else json.dumps(self.retro)
            message = AIMessage(content=text, usage_metadata={
                "input_tokens": RETRO_TOKENS, "output_tokens": 50, "total_tokens": RETRO_TOKENS + 50,
                "input_token_details": {"cache_read": 7_000}})
        self.responses = [message]
        self.i = 0
        return super()._generate(messages, **kwargs)


def install(monkeypatch, request, tmp_path, **model_args):
    request["inputs"]["context_size"] = 150_000
    request["trace"] = {"completion": str(tmp_path / "facts.json")}
    model = RetroModel(responses=[], calls=[], **model_args)
    monkeypatch.setattr(runtime, "create_model", lambda _: model)
    return model


def facts(tmp_path):
    return json.loads((tmp_path / "facts.json").read_text())


def test_retro_follows_the_accepted_reply_with_uncallable_tools(maker_request, monkeypatch, tmp_path):
    model = install(monkeypatch, maker_request, tmp_path)
    events = []
    runtime.run(maker_request, events.append)
    assert events[-1]["reply"] == CANT_MEET
    assert len(model.calls) == 2, "one retro turn follows the accepted reply"
    messages, offered = model.calls[1]
    assert offered == model.calls[0][1], "the retro binds the maker's tools, uncallable, for its cached prefix"
    assert any(isinstance(m, AIMessage) and json.loads(m.text) == CANT_MEET for m in messages), \
        "the retro sees the run it looks back on"
    assert isinstance(messages[-1], HumanMessage)
    assert "retro" in messages[-1].text.lower()
    row = facts(tmp_path)
    assert row["outcome"] == "succeeded"
    assert row["retro"]["status"] == "recorded"
    assert row["retro"]["result"] == RETRO


def test_retro_usage_is_outside_the_budget_and_the_run_totals(maker_request, monkeypatch, tmp_path):
    # Two working turns exhaust a one-turn allowance, so the reply carries the budget diagnostic.
    budget = SimpleNamespace(**{**asdict(maker.ROLE.exploration_budget), "tool_turns": 1,
                                "input_tokens": 6_000_000, "finalization_turns": 0})
    monkeypatch.setattr(maker, "ROLE", replace(maker.ROLE, exploration_budget=budget))
    model = install(monkeypatch, maker_request, tmp_path, work_turns=2)
    events = []
    runtime.run(maker_request, events.append)
    reply = events[-1]["reply"]
    assert len(model.calls) == 3, "the retro still runs after the allowance is spent"
    assert reply["budget_exhaustion"]["input_tokens"]["used"] == 2 * WORK_TOKENS
    row = facts(tmp_path)
    assert (row["input_tokens"], row["output_tokens"]) == (2 * WORK_TOKENS, 2)
    assert row["usage_complete"] is True
    assert row["retro"]["input_tokens"] == RETRO_TOKENS
    assert row["retro"]["cached_tokens"] == 7_000
    assert row["retro"]["output_tokens"] == 50


def _timeout():
    return openai.APITimeoutError(request=httpx.Request("POST", "http://model.invalid/v1"))


@pytest.mark.parametrize("retro,status", [
    (RuntimeError("proxy text that must not be kept"), "failed"),
    (_timeout(), "timeout"),
    ("I learned a lot.", "invalid"),
    ({**RETRO, "noise": ["x" * 241]}, "invalid"),
    ({**RETRO, "extra": []}, "invalid"),
])
def test_a_failing_retro_leaves_the_outcome_unchanged(maker_request, monkeypatch, tmp_path, retro, status):
    install(monkeypatch, maker_request, tmp_path, retro=retro)
    events = []
    runtime.run(maker_request, events.append)
    assert events[-1] == {"event": "reply", "role": "maker", "reply": CANT_MEET}
    row = facts(tmp_path)
    assert row["outcome"] == "succeeded"
    assert row["failure"] is None
    assert row["input_tokens"] == WORK_TOKENS
    assert row["retro"]["status"] == status
    assert "result" not in row["retro"]
    assert "proxy text" not in (tmp_path / "facts.json").read_text()


def test_an_out_of_room_reply_gets_no_retro(maker_request, monkeypatch, tmp_path):
    model = install(monkeypatch, maker_request, tmp_path, final_reply={"kind": "out_of_room"})
    events = []
    runtime.run(maker_request, events.append)
    assert events[-1]["reply"] == {"kind": "out_of_room"}
    assert len(model.calls) == 1, "an out_of_room ending has no context left for a retro"
    assert "retro" not in facts(tmp_path)


def run_while_retro_holds(maker_request, monkeypatch, tmp_path):
    """Run the maker; report whether the retro was still waiting when the reply went out."""
    release = threading.Event()
    model = install(monkeypatch, maker_request, tmp_path, hold=release)
    events, waiting = [], []

    def record(event):
        if event["event"] == "reply":
            waiting.append(model.holding)
        events.append(event)

    try:
        runtime.run(maker_request, record)
    finally:
        release.set()
    return events, waiting


def test_a_slow_retro_cannot_delay_or_lose_the_reply(maker_request, monkeypatch, tmp_path):
    from tl3_workers import retro
    # The deadline keeps a slow retro well inside the worker's silence allowance.
    monkeypatch.setattr(retro, "DEADLINE_S", 0.5, raising=False)
    events, waiting = run_while_retro_holds(maker_request, monkeypatch, tmp_path)
    assert waiting == [True], "the reply goes out while the slow retro still waits"
    assert events[-1] == {"event": "reply", "role": "maker", "reply": CANT_MEET}
    assert events[-2] == {"event": "progress", "unit": "model_turn"}, "the retro turn reports progress first"
    assert len([event for event in events if event.get("unit") == "model_turn"]) == 2
    row = facts(tmp_path)
    assert row["outcome"] == "succeeded"
    assert row["retro"]["status"] == "timeout"


def test_the_retro_deadline_is_half_the_silence_allowance(maker_request, monkeypatch, tmp_path):
    # A future, shorter allowance still leaves the reply time to go out. The
    # 60-second default would wait out the 8-second hold before replying.
    maker_request["silence_secs"] = 1
    events, waiting = run_while_retro_holds(maker_request, monkeypatch, tmp_path)
    assert waiting == [True], "a 1-second allowance gives the retro half a second"
    assert events[-1] == {"event": "reply", "role": "maker", "reply": CANT_MEET}
    assert facts(tmp_path)["retro"]["status"] == "timeout"


def test_the_retro_deadline_is_half_the_shorter_allowance(maker_request, monkeypatch, tmp_path):
    # A no-progress allowance shorter than silence bounds the retro too (#661 review).
    from tl3_workers import retro
    used = []
    real = retro.deadline
    monkeypatch.setattr(retro, "deadline", lambda *args, **kwargs: used.append(real(*args, **kwargs)) or used[-1])
    maker_request.update(silence_secs=120, no_progress_secs=20)
    install(monkeypatch, maker_request, tmp_path)
    runtime.run(maker_request, lambda event: None)
    assert used == [10]


def test_the_request_may_carry_the_silence_allowance(maker_request):
    from jsonschema import ValidationError
    from tl3_workers.contracts import validate

    validate("request", {**maker_request, "silence_secs": 120, "no_progress_secs": 20})
    for field in ("silence_secs", "no_progress_secs"):
        with pytest.raises(ValidationError):
            validate("request", {**maker_request, field: 0})


def test_retro_is_a_separate_span_on_the_maker_trace(maker_request, monkeypatch, tmp_path, exporters, receiver):
    uri, packets = receiver
    maker_request["mlflow"] = {"tracking_uri": uri, "experiment": "tl3-test"}
    install(monkeypatch, maker_request, tmp_path)
    runtime.run(maker_request, lambda event: None)
    for child in exporters:
        assert child.wait(timeout=15) == 0
    spans = exported_spans(packets)
    root = next(span for span in spans if span.parent_id is None)
    retro = next(span for span in spans if span.name == "tl3.maker_retro")
    assert retro.parent_id == root.span_id
    assert retro.outputs["status"] == "recorded"
    assert retro.outputs["result"] == RETRO
    retro_models = [span for span in spans if span.parent_id == retro.span_id
                    and span.span_type == "CHAT_MODEL"]
    assert len(retro_models) == 1
    assert root.get_attribute("mlflow.traceTag.retro") == "recorded"
    assert root.get_attribute("mlflow.traceTag.input_tokens") == str(WORK_TOKENS)
    assert root.get_attribute("mlflow.traceTag.retro_input_tokens") == str(RETRO_TOKENS)
    assert root.outputs == CANT_MEET
