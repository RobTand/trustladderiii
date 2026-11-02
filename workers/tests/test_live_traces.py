"""Live worker traces (#430): visible while a run works, durable on disk first,
and closed from the span log when the worker dies."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import threading

from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage
import pytest

from tl3_workers import runtime
import test_workers
from test_workers import BRIEF
import worker_process

request_doc = test_workers.request_doc
TAGS = {'repo': 'RobTand/tl3-sandbox', 'issue': '101', 'request': '7', 'node': '1', 'generation': '0',
        'role': 'ingest', 'model': 'fixture-model', 'effort': 'high', 'attempt': '2', 'round': '1',
        'prompt_version': 'packaged'}
USAGE = {'input_tokens': 11, 'output_tokens': 7, 'total_tokens': 18}


class Server:
    """MLflow 3.17.2's OTLP storage rules (SqlAlchemyStore.log_spans): spans are
    upserted by trace and span id; a trace stays IN_PROGRESS until a batch holds
    its root span, whose status then sets the state once; every batch's resource
    attributes become trace tags, and the root's mlflow.traceTag.* attributes
    are written after them."""

    def __init__(self):
        self.changed = threading.Condition()
        self.traces = {}
        self.posts = 0

    def store(self, body):
        from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import ExportTraceServiceRequest
        from mlflow.tracing.utils.otlp import _decode_otel_proto_anyvalue

        packet = ExportTraceServiceRequest.FromString(body)
        with self.changed:
            for resource_spans in packet.resource_spans:
                resource = {attr.key: _decode_otel_proto_anyvalue(attr.value)
                            for attr in resource_spans.resource.attributes}
                for scope in resource_spans.scope_spans:
                    for span in scope.spans:
                        trace = self.traces.setdefault(span.trace_id.hex(), {
                            'state': 'IN_PROGRESS', 'tags': {}, 'spans': {}})
                        trace['spans'][span.span_id.hex()] = span
                        trace['tags'].update({key: str(value) for key, value in resource.items()
                                              if not key.startswith('mlflow.')})
                        if not span.parent_span_id:
                            if trace['state'] == 'IN_PROGRESS':
                                trace['state'] = 'ERROR' if span.status.code == 2 else 'OK'
                            for attr in span.attributes:
                                if attr.key.startswith('mlflow.traceTag.'):
                                    trace['tags'][attr.key[len('mlflow.traceTag.'):]] = \
                                        attr.value.string_value
            self.posts += 1
            self.changed.notify_all()

    def wait(self, predicate):
        with self.changed:
            assert self.changed.wait_for(lambda: predicate(self), timeout=30), 'MLflow never saw it'

    def only(self):
        assert len(self.traces) == 1, f'expected one trace, got {len(self.traces)}'
        return next(iter(self.traces.values()))


def spans_of(trace, kind):
    from mlflow.entities import Span
    spans = [Span.from_otel_proto(span) for span in trace['spans'].values()]
    return [span for span in spans if span.span_type == kind]


@pytest.fixture
def mlflow_server():
    server = Server()

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b'{"experiment":{"experiment_id":"12"}}')

        def do_POST(self):
            body = self.rfile.read(int(self.headers['Content-Length']))
            if self.path == '/v1/traces':
                server.store(body)
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b'{}')

        def log_message(self, *args):
            pass

    http = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    thread = threading.Thread(target=http.serve_forever, daemon=True)
    thread.start()
    server.uri = f'http://127.0.0.1:{http.server_port}'
    server.http = http
    yield server
    http.shutdown()
    http.server_close()
    thread.join()


@pytest.fixture
def children(monkeypatch):
    started = []
    popen = subprocess.Popen

    def launch(*args, **kwargs):
        child = popen(*args, **kwargs)
        started.append(child)
        return child

    monkeypatch.setattr(subprocess, 'Popen', launch)
    yield started
    for child in started:
        if child.poll() is None:
            child.kill()
        child.wait(timeout=15)


def two_turn_request(request_doc, tmp_path, uri):
    (tmp_path / 'greeting.py').write_text('def greet():\n    return "hello"\n')
    request_doc['mlflow'] = {'tracking_uri': uri, 'experiment': 'tl3-test'}
    request_doc['trace'] = {'repo': 'RobTand/tl3-sandbox', 'issue': 101, 'request': '7',
                            'attempt': 2, 'round': 1, 'log': str(tmp_path / 'attempt' / 'trace.jsonl')}
    (tmp_path / 'attempt').mkdir()
    return [AIMessage(content='', tool_calls=[{'name': 'read', 'args': {'path': 'greeting.py'}, 'id': 'read1'}],
                      usage_metadata=USAGE),
            AIMessage(content=json.dumps(BRIEF), usage_metadata=USAGE)]


def held_model(turns, hold):
    class Held(FakeMessagesListChatModel):
        def _generate(self, *args, **kwargs):
            if self.i == len(turns) - 1:
                hold.wait(30)
            return super()._generate(*args, **kwargs)

    return Held(responses=turns)


def first_turn_live(server):
    return any(spans_of(trace, 'CHAT_MODEL') for trace in server.traces.values())


def test_trace_is_visible_mid_run_and_final_after(request_doc, tmp_path, monkeypatch, children,
                                                 mlflow_server):
    turns = two_turn_request(request_doc, tmp_path, mlflow_server.uri)
    hold = threading.Event()
    monkeypatch.setattr(runtime, 'create_model', lambda _: held_model(turns, hold))
    events, errors = [], []

    def work():
        try:
            runtime.run(request_doc, events.append)
        except BaseException as error:
            errors.append(error)

    worker = threading.Thread(target=work)
    worker.start()
    try:
        mlflow_server.wait(first_turn_live)
        live = mlflow_server.only()
        assert live['state'] == 'IN_PROGRESS'
        assert {key: live['tags'].get(key) for key in TAGS} == TAGS
        assert len(spans_of(live, 'CHAT_MODEL')) == 1
        assert not any(not span.parent_span_id for span in live['spans'].values()), \
            'the root span closes the trace, so it waits for the end'
    finally:
        hold.set()
        worker.join(30)
    assert not errors and events[-1]['reply'] == BRIEF
    for child in children:
        assert child.wait(timeout=15) == 0
    final = mlflow_server.only()
    assert final['state'] == 'OK'
    assert len(spans_of(final, 'CHAT_MODEL')) == 2
    root = next(span for span in final['spans'].values() if not span.parent_span_id)
    exported = {attr.key[len('mlflow.traceTag.'):]: attr.value.string_value for attr in root.attributes
                if attr.key.startswith('mlflow.traceTag.')}
    assert final['tags'] == exported, 'live tags must end as the end-of-run export left them'
    assert exported['outcome'] == 'succeeded' and exported['total_tokens'] == '36'
    log = Path(request_doc['trace']['log'])
    assert Path(str(log) + '.shipped').exists()
    assert b'never-read-this-key' not in log.read_bytes()


def test_mlflow_outage_mid_run_keeps_the_run_and_the_durable_log(request_doc, tmp_path, monkeypatch,
                                                                  children, mlflow_server, capfd):
    turns = two_turn_request(request_doc, tmp_path, mlflow_server.uri)
    hold = threading.Event()
    monkeypatch.setattr(runtime, 'create_model', lambda _: held_model(turns, hold))
    events = []
    worker = threading.Thread(target=lambda: runtime.run(request_doc, events.append))
    worker.start()
    try:
        mlflow_server.wait(first_turn_live)
    finally:
        mlflow_server.http.shutdown()
        mlflow_server.http.server_close()
        hold.set()
        worker.join(30)
    assert events[-1]['reply'] == BRIEF
    for child in children:
        child.wait(timeout=15)
    log = Path(request_doc['trace']['log'])
    kinds = [json.loads(line)['kind'] for line in log.read_text().splitlines()]
    assert kinds[0] == 'start' and kinds[-1] == 'final' and kinds.count('span') >= 2
    assert not Path(str(log) + '.shipped').exists(), 'an unsent final must stay for the controller'
    assert 'tl3_workers: MLflow export failed' in capfd.readouterr().err


HOLD_LAST = """
import threading
_generate = FakeMessagesListChatModel._generate
def held(self, *args, **kwargs):
    if self.i == len(script) - 1:
        threading.Event().wait()
    return _generate(self, *args, **kwargs)
FakeMessagesListChatModel._generate = held
"""


def test_killed_worker_leaves_a_complete_crash_marked_trace(request_doc, tmp_path, mlflow_server):
    turns = two_turn_request(request_doc, tmp_path, mlflow_server.uri)
    script = tmp_path / 'scripted-model.json'
    script.write_text(json.dumps([turn.model_dump(include={'content', 'tool_calls', 'usage_metadata'})
                                  for turn in turns]))
    launch = worker_process.LAUNCH.replace('def create_model', HOLD_LAST + 'def create_model')
    env = {**os.environ, 'LANGSMITH_TRACING': 'false', 'LANGCHAIN_TRACING_V2': 'false'}
    worker = subprocess.Popen([sys.executable, '-c', launch, str(script)], stdin=subprocess.PIPE,
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=env,
                              start_new_session=True)
    try:
        worker.stdin.write(json.dumps(request_doc).encode())
        worker.stdin.close()
        mlflow_server.wait(first_turn_live)
    finally:
        os.killpg(worker.pid, signal.SIGKILL)
        worker.wait(timeout=15)
    assert mlflow_server.only()['state'] == 'IN_PROGRESS'
    log = Path(request_doc['trace']['log'])
    (log.parent / 'exit.json').write_text(json.dumps({'code': -9}))
    # What the controller starts when it sees the attempt end (#430). It adds -I;
    # Bazel's test Python finds packages through PYTHONPATH, which -I ignores.
    with log.open('rb') as records:
        flush = subprocess.run([sys.executable, '-m', 'tl3_workers.tracing', 'flush', str(log)],
                               stdin=records, capture_output=True, timeout=30)
    assert flush.returncode == 0, flush.stderr
    trace = mlflow_server.only()
    assert trace['state'] == 'ERROR'
    assert {key: trace['tags'].get(key) for key in TAGS} == TAGS
    assert trace['tags']['outcome'] == 'crashed'
    assert trace['tags']['failure_class'] == 'WorkerCrashed'
    assert trace['tags']['failure_message'] == 'worker exited with code -9'
    assert trace['tags']['total_tokens'] == '18'
    assert len(spans_of(trace, 'CHAT_MODEL')) == 1
    root = next(span for span in trace['spans'].values() if not span.parent_span_id)
    assert root.end_time_unix_nano >= max(span.end_time_unix_nano for span in trace['spans'].values())
    assert Path(str(log) + '.shipped').exists()
    # A controller restart may flush again: span ids make it a replay, not a second trace.
    spans = set(trace['spans'])
    with log.open('rb') as records:
        again = subprocess.run([sys.executable, '-m', 'tl3_workers.tracing', 'flush', str(log)],
                               stdin=records, capture_output=True, timeout=30)
    assert again.returncode == 0
    assert mlflow_server.only()['state'] == 'ERROR' and set(mlflow_server.only()['spans']) == spans
