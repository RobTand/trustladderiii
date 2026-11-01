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


def run_fake(request_doc, monkeypatch, exporters, uri, *, secret='sk-fixture296secretvalue0123456789', wait=True):
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
    assert b'sk-fixture296secretvalue0123456789' not in packets[-1][2]
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


@pytest.mark.parametrize('error,category,message', [
    (ValueError('done requires code changes'), 'ValueError', 'done requires code changes'),
    (ValueError('reply cites a memory it was not given'), 'ValueError',
     'reply cites a memory it was not given'),
    (RuntimeError('fixture-secret-value /private/key provider payload'), 'RuntimeError', 'worker run failed'),
])
def test_failed_worker_trace_records_a_safe_reason(request_doc, monkeypatch, exporters, receiver,
                                                  error, category, message):
    uri, packets = receiver
    request_doc['mlflow'] = {'tracking_uri': uri, 'experiment': 'tl3-test'}
    def fail(_):
        raise error
    monkeypatch.setattr(runtime, 'create_model', fail)
    with pytest.raises(type(error)):
        runtime.run(request_doc, lambda event: None)
    for child in exporters:
        child.wait(timeout=15)
    spans = decode(packets)
    root = next(span for span in spans if not span.parent_span_id)
    attrs = {attr.key: attr.value.string_value for attr in root.attributes}
    assert attrs['mlflow.traceTag.outcome'] == 'failed'
    assert 'mlflow.traceTag.failure_class' in attrs
    assert attrs['mlflow.traceTag.failure_class'] == category
    assert attrs['mlflow.traceTag.failure_message'] == message
    assert b'fixture-secret-value' not in packets[-1][2]
    assert b'/private/key' not in packets[-1][2]
    assert all(not span.events and not span.status.message for span in spans)


def test_schema_failure_diagnostic_does_not_assume_reply_validation():
    from jsonschema import Draft202012Validator, ValidationError
    from tl3_workers.tracing import failure_tags

    with pytest.raises(ValidationError) as caught:
        Draft202012Validator({"type": "integer"}).validate("fixture-secret-value")
    assert failure_tags(caught.value) == {
        "failure_class": "ValidationError", "failure_message": "schema validation failed: type at schema type"}


def exported_spans(packets):
    from mlflow.entities import Span
    return [Span.from_otel_proto(span) for span in decode(packets)]


def test_model_and_root_capture_readable_content(request_doc, monkeypatch, exporters, receiver):
    uri, packets = receiver
    run_fake(request_doc, monkeypatch, exporters, uri, secret='Return a friendly greeting.')
    spans = exported_spans(packets)
    root = next(span for span in spans if span.parent_id is None)
    model = next(span for span in spans if span.span_type == 'CHAT_MODEL')
    assert 'ingest' in root.name
    assert 'ingest' in model.name and 'fixture-model' in model.name
    assert root.inputs == request_doc['inputs']
    assert root.outputs['brief'] == 'Return a friendly greeting.'
    assert 'Return a friendly greeting.' in json.dumps(model.inputs)
    assert 'Return a friendly greeting.' in json.dumps(model.outputs)


@pytest.mark.parametrize('metadata,expected', [
    ({'usage_metadata': {'input_tokens': 11, 'output_tokens': 7, 'total_tokens': 18,
                        'input_token_details': {'cache_read': 8, 'cache_creation': 2}}},
     {'cache_read_input_tokens': 8, 'cache_creation_input_tokens': 2}),
    ({'response_metadata': {'token_usage': {'prompt_tokens': 11, 'completion_tokens': 7,
         'total_tokens': 18, 'prompt_tokens_details': {'cached_tokens': 6}}}},
     {'cache_read_input_tokens': 6}),
    ({'usage_metadata': {'input_tokens': 11, 'output_tokens': 7, 'total_tokens': 18,
                        'input_token_details': {'cache_read': 0}}},
     {'cache_read_input_tokens': 0}),
])
def test_cached_tokens_reach_model_and_root(request_doc, monkeypatch, exporters, receiver,
                                          metadata, expected):
    uri, packets = receiver
    request_doc['mlflow'] = {'tracking_uri': uri, 'experiment': 'tl3-test'}
    model = FakeMessagesListChatModel(responses=[AIMessage(content=json.dumps(BRIEF), **metadata)])
    monkeypatch.setattr(runtime, 'create_model', lambda _: model)
    runtime.run(request_doc, lambda event: None)
    for child in exporters:
        assert child.wait(timeout=15) == 0
    spans = exported_spans(packets)
    model_span = next(span for span in spans if span.span_type == 'CHAT_MODEL')
    root = next(span for span in spans if span.parent_id is None)
    usage = model_span.get_attribute('mlflow.chat.tokenUsage')
    for key, count in expected.items():
        assert usage.get(key) == count
        assert root.get_attribute('mlflow.traceTag.' + key) == str(count)
    assert usage['input_tokens'] == 11 and usage['total_tokens'] == 18
    if 'cache_creation_input_tokens' not in expected:
        assert 'cache_creation_input_tokens' not in usage
        assert root.get_attribute('mlflow.traceTag.cache_creation_input_tokens') is None


def test_tool_arguments_results_and_model_tool_calls_are_bounded(request_doc, tmp_path,
                                                               monkeypatch, exporters, receiver):
    uri, packets = receiver
    request_doc['mlflow'] = {'tracking_uri': uri, 'experiment': 'tl3-test'}
    body = 'tool-result-marker ' + '文' * 40000
    (tmp_path / 'large.txt').write_text(body)
    model = FakeMessagesListChatModel(responses=[
        AIMessage(content='', tool_calls=[{'name': 'read', 'args': {'path': 'large.txt'}, 'id': 'read1'}]),
        AIMessage(content=json.dumps(BRIEF)),
    ])
    monkeypatch.setattr(runtime, 'create_model', lambda _: model)
    runtime.run(request_doc, lambda event: None)
    for child in exporters:
        assert child.wait(timeout=15) == 0
    spans = exported_spans(packets)
    tool = next(span for span in spans if span.span_type == 'TOOL')
    assert 'read' in tool.name
    assert 'large.txt' in json.dumps(tool.inputs)
    assert 'tool-result-marker' in json.dumps(tool.outputs)
    assert 'truncated' in json.dumps(tool.outputs)
    assert len(json.dumps(tool.outputs, ensure_ascii=False).encode()) <= 65536
    assert any('read1' in json.dumps(span.outputs) for span in spans if span.span_type == 'CHAT_MODEL')


def test_redacts_planted_secrets_without_tracing_reading_files(request_doc, tmp_path,
                                                              monkeypatch, exporters, receiver):
    uri, packets = receiver
    from pathlib import Path
    key = 'opaque-model-key-296-which-has-no-provider-prefix'
    key_path = tmp_path / 'planted-key'
    key_path.write_text(key)
    request_doc['key_file'] = str(key_path)
    secrets = [key, 'sk-test296abcdefghijklmnop', 'ghp_test296abcdefghijklmnop',
               'bearer296abcdefghijklmnop', 'url-password-296', 'private296contents',
               'assigned-password-296']
    planted = (f'{key} sk-test296abcdefghijklmnop ghp_test296abcdefghijklmnop '
               'Bearer bearer296abcdefghijklmnop https://user:url-password-296@example.test '
               '-----BEGIN PRIVATE KEY-----\nprivate296contents\n-----END PRIVATE KEY----- '
               'password=assigned-password-296')
    request_doc['inputs']['issue']['body'] = 'Visible request. ' + planted
    request_doc['trace'] = {'repo': planted}
    reply = {**BRIEF, 'brief': 'Visible reply. ' + planted}
    fake = FakeMessagesListChatModel(responses=[AIMessage(content=json.dumps(reply))])
    monkeypatch.setattr(runtime, 'ProxyChatOpenAI', lambda **kwargs: fake)
    original = Path.read_text
    reads = []
    def read(path, *args, **kwargs):
        if path == key_path:
            reads.append(path)
        return original(path, *args, **kwargs)
    monkeypatch.setattr(Path, 'read_text', read)
    request_doc['mlflow'] = {'tracking_uri': uri, 'experiment': 'tl3-test'}
    runtime.run(request_doc, lambda event: None)
    for child in exporters:
        assert child.wait(timeout=15) == 0
    spans = exported_spans(packets)
    root = next(span for span in spans if span.parent_id is None)
    assert 'Visible request.' in json.dumps(root.inputs)
    assert 'Visible reply.' in json.dumps(root.outputs)
    assert '[REDACTED]' in json.dumps(root.inputs)
    assert reads == [key_path], 'only normal model loading may read the key'
    for secret in secrets:
        assert secret.encode() not in packets[-1][2]
    assert all(not span.events for span in decode(packets))


def test_failed_root_has_readable_safe_output(request_doc, monkeypatch, exporters, receiver):
    uri, packets = receiver
    request_doc['mlflow'] = {'tracking_uri': uri, 'experiment': 'tl3-test'}
    def fail(_):
        raise ValueError('done requires code changes')
    monkeypatch.setattr(runtime, 'create_model', fail)
    with pytest.raises(ValueError):
        runtime.run(request_doc, lambda event: None)
    for child in exporters:
        assert child.wait(timeout=15) == 0
    root = next(span for span in exported_spans(packets) if span.parent_id is None)
    assert root.inputs == request_doc['inputs']
    assert root.outputs['failure_message'] == 'done requires code changes'


def test_export_bounds_each_tool_payload_after_redaction(request_doc, exporters, receiver):
    import mlflow
    from tl3_workers.tracing import observe
    uri, packets = receiver
    request_doc['mlflow'] = {'tracking_uri': uri, 'experiment': 'tl3-test'}
    with observe(request_doc):
        with mlflow.start_span(name='oversized-tool', span_type='TOOL') as span:
            span.set_inputs({'argument': '文' * 40000 + ' sk-test296abcdefghijklmnop'})
            span.set_outputs({'result': 'result-marker ' + '文' * 40000})
    for child in exporters:
        assert child.wait(timeout=15) == 0
    tool = next(span for span in exported_spans(packets) if span.span_type == 'TOOL')
    for payload in [tool.inputs, tool.outputs]:
        assert payload is not None
        text = json.dumps(payload, ensure_ascii=False)
        assert len(text.encode()) <= 65536
        assert 'truncated' in text.lower()
    assert b'sk-test296abcdefghijklmnop' not in packets[-1][2]


def test_export_marks_experiment_for_trace_ui(request_doc, monkeypatch, exporters, receiver):
    uri, packets = receiver
    run_fake(request_doc, monkeypatch, exporters, uri)
    tags = [json.loads(body) for path, headers, body in packets
            if path == '/api/2.0/mlflow/experiments/set-experiment-tag']
    assert {'experiment_id': '12', 'key': 'mlflow.experimentKind',
            'value': 'genai_development'} in tags
