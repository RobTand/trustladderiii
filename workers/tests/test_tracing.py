"""Exercise the real graph and transport against a local MLflow-shaped receiver."""
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import subprocess
import threading

from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage
import pytest

from tl3_workers import runtime
import test_workers
from test_workers import BRIEF

# pytest collects the shared fixture under this module-level name.
request_doc = test_workers.request_doc


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
            self.send_response(200)
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
        children.append(child)
        return child

    monkeypatch.setattr(subprocess, 'Popen', launch)
    yield children
    for child in children:
        if child.poll() is None:
            child.kill()
        child.wait(timeout=15)


def run_fake(request_doc, monkeypatch, exporters, uri, *, secret='fixture-secret-value', wait=True):
    request_doc['mlflow'] = {'tracking_uri': uri, 'experiment': 'tl3-test'}
    request_doc['trace'] = {'repo': 'RobTand/tl3-sandbox', 'issue': 101, 'request': '7',
                            'attempt': 2, 'round': 1}
    request_doc['inputs']['issue']['body'] = secret
    reply = {**BRIEF, 'brief': secret}
    model = FakeMessagesListChatModel(responses=[AIMessage(
        content=json.dumps(reply), usage_metadata={'input_tokens': 11, 'output_tokens': 7,
                                                 'total_tokens': 18},
        response_metadata={'headers': {'Authorization': 'Bearer ' + secret}},
    )])
    monkeypatch.setattr(runtime, 'create_model', lambda _: model)
    events = []
    runtime.run(request_doc, events.append)
    assert events[-1]['reply'] == reply
    if wait:
        for child in exporters:
            child.wait(timeout=15)
    return events


def decode(packets):
    assert packets, 'worker did not export a trace'
    from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import ExportTraceServiceRequest
    request = ExportTraceServiceRequest.FromString(packets[-1][2])
    return list(request.resource_spans[0].scope_spans[0].spans)


def test_run_produces_tagged_trace(request_doc, monkeypatch, exporters, receiver):
    uri, packets = receiver
    run_fake(request_doc, monkeypatch, exporters, uri)
    spans = decode(packets)
    assert packets[-1][0] == '/v1/traces'
    assert packets[-1][1]['x-mlflow-experiment-id'] == '12'
    root = next(span for span in spans if not span.parent_span_id)
    attrs = {attr.key: attr.value.string_value for attr in root.attributes}
    expected = {'repo': 'RobTand/tl3-sandbox', 'issue': '101', 'request': '7', 'node': '1',
                'role': 'ingest', 'model': 'fixture-model', 'effort': 'high', 'attempt': '2',
                'round': '1', 'generation': '0', 'outcome': 'succeeded',
                'input_tokens': '11', 'output_tokens': '7', 'total_tokens': '18'}
    for key, value in expected.items():
        assert attrs['mlflow.traceTag.' + key] == value
    from mlflow.entities import Span
    decoded = [Span.from_otel_proto(span) for span in spans]
    model = next(span for span in decoded if span.span_type == 'CHAT_MODEL')
    assert model.get_attribute('mlflow.chat.tokenUsage') == {
        'input_tokens': 11, 'output_tokens': 7, 'total_tokens': 18}
    assert all(Span.from_dict(span.to_dict()).span_type == span.span_type for span in decoded)
    assert len(spans) > 1, 'Deep Agents graph was not autologged'
    from mlflow.tracing.trace_manager import InMemoryTraceManager
    assert InMemoryTraceManager.get_instance().get_mlflow_trace_id_from_otel_id(
        int.from_bytes(root.trace_id, 'big')) is None, 'raw spans must leave the SDK cache after export'



def test_run_succeeds_with_mlflow_down(request_doc, monkeypatch, exporters, capfd):
    run_fake(request_doc, monkeypatch, exporters, 'http://127.0.0.1:1')
    assert 'tl3_workers: MLflow export failed' in capfd.readouterr().err


def test_no_secret_in_trace(request_doc, monkeypatch, exporters, receiver):
    uri, packets = receiver
    run_fake(request_doc, monkeypatch, exporters, uri)
    spans = decode(packets)
    assert b'fixture-secret-value' not in packets[-1][2]
    assert request_doc['key_file'].encode() not in packets[-1][2]
    assert b'Authorization' not in packets[-1][2]
    assert all(not span.events and not span.status.message for span in spans)


def test_slow_mlflow_does_not_hold_the_worker(request_doc, monkeypatch, exporters, receiver):
    uri, packets = receiver
    packets.block = True
    events = run_fake(request_doc, monkeypatch, exporters, uri, wait=False)
    assert events[-1]['event'] == 'reply'
    assert packets.started.wait(15), 'export child did not connect'
    assert exporters[-1].poll() is None, 'export child must still be waiting'
    packets.release.set()
    exporters[-1].wait(timeout=15)


def test_tracing_initialization_failure_does_not_fail_run(request_doc, monkeypatch, exporters, capfd):
    from tl3_workers import tracing

    def unavailable(_):
        raise RuntimeError('fixture-secret-value')

    monkeypatch.setattr(tracing, 'Capture', unavailable)
    run_fake(request_doc, monkeypatch, exporters, 'http://127.0.0.1:1')
    stderr = capfd.readouterr().err
    assert 'tl3_workers: MLflow export failed' in stderr
    assert 'fixture-secret-value' not in stderr
