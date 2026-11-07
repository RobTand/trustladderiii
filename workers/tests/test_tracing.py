"""Exercise the real graph and transport against a local MLflow-shaped receiver."""
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import subprocess
import socket
import threading

from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage
import pytest

from tl3_workers import runtime
import test_workers
from test_workers import BRIEF

_socket_connect = socket.socket.connect
_getaddrinfo = socket.getaddrinfo

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


def test_exporter_fixture_ignores_unrelated_children(exporters):
    child = subprocess.Popen(['git', '--version'], stdout=subprocess.DEVNULL)
    assert child.wait(timeout=15) == 0
    assert not exporters, 'only trace shippers belong to the exporter wait set'


def run_fake(request_doc, monkeypatch, exporters, uri, *, secret='sk-fixture296secretvalue0123456789', wait=True,
             trace=None):
    request_doc['mlflow'] = {'tracking_uri': uri, 'experiment': 'tl3-test'}
    request_doc['trace'] = trace or {'repo': 'RobTand/tl3-sandbox', 'issue': 101, 'request': '7',
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


def test_run_produces_tagged_trace(request_doc, monkeypatch, exporters, receiver, tmp_path):
    uri, packets = receiver
    completion = tmp_path / 'facts.json'
    run_fake(request_doc, monkeypatch, exporters, uri, trace={
        'repo': 'RobTand/tl3-sandbox', 'issue': 101, 'request': '7', 'attempt': 2, 'round': 1,
        'completion': str(completion)})
    spans = decode(packets)
    assert packets[-1][0] == '/v1/traces'
    assert packets[-1][1]['x-mlflow-experiment-id'] == '12'
    root = next(span for span in spans if not span.parent_span_id)
    facts = json.loads(completion.read_text())
    assert facts['trace_id'] == 'tr-' + root.trace_id.hex()
    assert (facts['input_tokens'], facts['output_tokens']) == (11, 7)
    attrs = {attr.key: attr.value.string_value for attr in root.attributes}
    expected = {'repo': 'RobTand/tl3-sandbox', 'issue': '101', 'request': '7', 'node': '1',
                'role': 'ingest', 'model': 'fixture-model', 'effort': 'high', 'attempt': '2',
                'round': '1', 'generation': '0', 'outcome': 'succeeded', 'prompt_version': 'packaged',
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



def test_a_run_for_no_request_is_tagged_none_not_unknown(request_doc, monkeypatch, exporters, receiver):
    # #427: the live shape the controller sends for a write that belongs to no request.
    uri, packets = receiver
    request_doc['operation'] = {**request_doc['operation'], 'node': 0, 'generation': 0}
    run_fake(request_doc, monkeypatch, exporters, uri,
             trace={key: 'none' for key in ('repo', 'issue', 'request', 'attempt', 'round')})
    root = next(span for span in decode(packets) if not span.parent_span_id)
    attrs = {attr.key: attr.value.string_value for attr in root.attributes}
    for key in ('repo', 'issue', 'request', 'attempt', 'round'):
        assert attrs['mlflow.traceTag.' + key] == 'none'
    assert attrs['mlflow.traceTag.node'] == '0'


def test_trace_names_the_prompt_version_the_run_used(request_doc, monkeypatch, exporters, receiver):
    uri, packets = receiver
    request_doc['prompt'] = {'version': 12, 'text': 'Version 12 of the ingest prompt.'}
    run_fake(request_doc, monkeypatch, exporters, uri)
    root = next(span for span in decode(packets) if not span.parent_span_id)
    attrs = {attr.key: attr.value.string_value for attr in root.attributes}
    assert attrs['mlflow.traceTag.prompt_version'] == '12'


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
    assert 'truncated' in json.dumps(tool.outputs).lower()
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


def test_proxy_chunk_usage_is_visible_without_changing_accounting(request_doc, tmp_path,
                                                                 monkeypatch, exporters, receiver):
    def connect(sock, address):
        assert address[0] == "127.0.0.1"
        return _socket_connect(sock, address)
    def lookup(host, *args, **kwargs):
        assert host == "127.0.0.1"
        return _getaddrinfo(host, *args, **kwargs)
    monkeypatch.setattr(socket.socket, "connect", connect)
    monkeypatch.setattr(socket, "getaddrinfo", lookup)
    uri, packets = receiver
    usage = {'prompt_tokens': 11, 'completion_tokens': 7, 'total_tokens': 18,
             'prompt_tokens_details': {'cached_tokens': 8, 'cache_write_tokens': 2}, 'cache_write_tokens': 2,
             'unexpected_secret': 'never-export-this-usage-field'}
    class Proxy(BaseHTTPRequestHandler):
        def do_POST(self):
            self.rfile.read(int(self.headers['Content-Length']))
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.end_headers()
            for choices, counts in [
                ([{'index': 0, 'delta': {'role': 'assistant', 'content': json.dumps(BRIEF)},
                   'finish_reason': 'stop'}], usage), ([], usage),
            ]:
                chunk = {'id': 'fixture-stream', 'object': 'chat.completion.chunk', 'created': 1,
                         'model': 'fixture-model', 'choices': choices, 'usage': counts}
                self.wfile.write(('data: ' + json.dumps(chunk) + '\n\n').encode())
                self.wfile.flush()
            self.wfile.write(b'data: [DONE]\n\n')
        def log_message(self, *args):
            pass
    server = ThreadingHTTPServer(('127.0.0.1', 0), Proxy)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        key = tmp_path / 'planted-model-key'
        key.write_text('opaque-proxy-fixture-key')
        request_doc['key_file'] = str(key)
        request_doc['model']['base_url'] = f'http://127.0.0.1:{server.server_port}/v1'
        request_doc['mlflow'] = {'tracking_uri': uri, 'experiment': 'tl3-test'}
        runtime.run(request_doc, lambda event: None)
        for child in exporters:
            assert child.wait(timeout=15) == 0
        span = next(span for span in exported_spans(packets) if span.span_type == 'CHAT_MODEL')
        raw = span.get_attribute('tl3.proxy.usage_chunks')
        assert raw is not None, 'raw chunk counts must remain visible beside LangChain totals'
        assert len(raw) == 2
        expected = {key: value for key, value in usage.items() if key != 'unexpected_secret'}
        assert raw == [expected, expected]
        assert span.get_attribute('mlflow.chat.tokenUsage')['cache_read_input_tokens'] > 0
        assert span.get_attribute('mlflow.chat.tokenUsage')['cache_creation_input_tokens'] > 0
        assert b'never-export-this-usage-field' not in packets[-1][2]
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


# Deliberately fake credentials. Exercise text even when it is nested JSON, as
# repository files and tool results often contain serialized configuration.
from tracing_forms import PLANTED, PLANTED_LINES  # noqa: E402




# Forms a second probe found after the review round. Header values run to the end
# of their line, so these sit on a line of their own.









@pytest.mark.parametrize('surface', ['tool_result', 'model_input', 'usage_chunks'])
def test_review_secrets_never_reach_export(request_doc, exporters, receiver, surface):
    import mlflow
    from tl3_workers.tracing import observe, record_chunk_usage
    uri, packets = receiver
    request_doc['mlflow'] = {'tracking_uri': uri, 'experiment': 'tl3-test'}
    planted = '\n'.join(text for text, _ in PLANTED + PLANTED_LINES)
    with observe(request_doc):
        with mlflow.start_span(name='review-fixture', span_type='CHAT_MODEL') as model:
            if surface == 'model_input':
                model.set_inputs({'messages': [{'content': planted}]})
            if surface == 'usage_chunks':
                model.set_attribute('tl3.proxy.usage_chunks', [{'fixture': planted}])
            record_chunk_usage({'usage': {'prompt_tokens': 7, 'secret': planted,
                                         'completion_tokens': planted}})
        with mlflow.start_span(name='read', span_type='TOOL') as tool:
            tool.set_inputs({'path': 'visible-file'})
            tool.set_outputs(planted if surface == 'tool_result' else 'visible-result')
    for child in exporters:
        assert child.wait(timeout=15) == 0
    for _, secret in PLANTED + PLANTED_LINES:
        assert secret.encode() not in packets[-1][2]
    assert b'visible-file' in packets[-1][2]


def test_every_exported_field_is_free_of_planted_secrets(request_doc, exporters, receiver):
    import mlflow
    from tl3_workers.tracing import observe, record_chunk_usage
    uri, packets = receiver
    marker = 'ghp_every296field0123456789abcdef'
    request_doc['mlflow'] = {'tracking_uri': uri, 'experiment': 'tl3-test'}
    request_doc['trace'] = {'repo': marker, 'issue': marker, 'request': marker,
                            'attempt': marker, 'round': marker}
    request_doc['inputs']['issue']['body'] = marker
    with pytest.raises(RuntimeError):
        with observe(request_doc):
            with mlflow.start_span(name='name-' + marker, span_type='CHAT_MODEL') as model:
                model.set_inputs({'messages': [{'role': 'user', 'content': marker}]})
                model.set_outputs({'content': marker, 'tool_calls': [{'args': {'x': marker}}]})
                model.set_attributes({'tl3.unlisted': marker, 'headers': marker,
                                      'mlflow.chat.tokenUsage': {'input_tokens': 3, marker: 4}})
                record_chunk_usage({'usage': {'prompt_tokens': 3, marker: 1, 'completion_tokens': marker}})
            with mlflow.start_span(name='tool-' + marker, span_type='TOOL') as tool:
                tool.set_inputs({'path': marker, marker: 'value'})
                tool.set_outputs([marker, {'nested': {marker: marker}}])
                tool.set_attribute('tl3.unlisted', marker)
            raise RuntimeError(marker)
    for child in exporters:
        assert child.wait(timeout=15) == 0
    assert marker.encode() not in packets[-1][2]
    assert marker[4:].encode() not in packets[-1][2]






def test_trace_has_total_budget_and_keeps_root(request_doc, exporters, receiver):
    import mlflow
    from tl3_workers.tracing import observe, record_reply
    uri, packets = receiver
    request_doc['mlflow'] = {'tracking_uri': uri, 'experiment': 'tl3-test'}
    with observe(request_doc):
        for index in range(120):
            with mlflow.start_span(name=f'model-{index}', span_type='CHAT_MODEL') as span:
                span.set_inputs({'messages': [{'content': 'history-line\n' * 10000 + f'latest-{index}'}
                                              for _ in range(20)]})
                span.set_outputs({'content': 'reply-line\n' * 10000 + f'reply-{index}'})
        record_reply({'brief': 'final-readable-reply'})
    for child in exporters:
        assert child.wait(timeout=15) == 0
    assert len(packets[-1][2]) <= 1024 * 1024, 'total trace export must fit one MiB'
    spans = exported_spans(packets)
    assert len(spans) >= 121, 'budget keeps span metadata'
    # Older spans shrink to a floor instead of losing all content; the newest keep the most.
    models = [span for span in spans if span.span_type == 'CHAT_MODEL']
    assert 'latest-119' in json.dumps(models[-1].inputs) and 'reply-119' in json.dumps(models[-1].outputs)
    assert all('latest-' in json.dumps(span.inputs) for span in models)
    root = next(span for span in spans if span.parent_id is None)
    assert root.inputs and root.outputs['brief'] == 'final-readable-reply'
    assert root.get_attribute('tl3.trace.truncated') is True


def test_malformed_chunk_cannot_escape_observer(request_doc, exporters, receiver):
    from tl3_workers.tracing import observe, record_chunk_usage
    uri, _ = receiver
    request_doc['mlflow'] = {'tracking_uri': uri, 'experiment': 'tl3-test'}
    with observe(request_doc):
        record_chunk_usage(None)
    for child in exporters:
        assert child.wait(timeout=15) == 0


def test_experiment_tag_failure_does_not_discard_trace(request_doc, monkeypatch, exporters, receiver):
    uri, packets = receiver
    packets.tag_status = 500
    run_fake(request_doc, monkeypatch, exporters, uri)
    assert all(child.wait(timeout=15) == 0 for child in exporters)
    assert packets[-1][0] == '/v1/traces'




# Third review round. The reviewer's 245 planted forms each pass through every
# way a string reaches a span. Only the limits that workers/README.md lists may leak.
from tracing_forms import FORMS, B64, r  # noqa: E402



def leaked(output, secrets, width=8):
    flat = output if isinstance(output, str) else json.dumps(output, ensure_ascii=False)
    return [secret for secret in secrets
            if any(secret[i:i + width] in flat for i in range(max(1, len(secret) - width + 1)))]




def test_planted_forms_never_reach_the_packet(request_doc, exporters, receiver):
    import mlflow
    from tl3_workers.tracing import observe, record_reply
    uri, packets = receiver
    request_doc['mlflow'] = {'tracking_uri': uri, 'experiment': 'tl3-test'}
    forms = [(name, text, secrets) for name, text, secrets, _ in FORMS if name not in DOCUMENTED_LIMITS]
    with observe(request_doc):
        with mlflow.start_span(name='chat', span_type='CHAT_MODEL') as model:
            model.set_inputs({'messages': [[{'type': 'system', 'content': 'MARKER_SYS'}] +
                                           [{'type': 'human', 'content': text} for _, text, _ in forms]]})
        for name, text, _ in forms:
            with mlflow.start_span(name='tool ' + text, span_type='TOOL') as tool:
                tool.set_inputs({'command': text, 'note': 'MARKER_TOOL_IN'})
                tool.set_outputs(text + '\nMARKER_TOOL_OUT')
        record_reply({'verdict': 'MARKER_REPLY'})
    for child in exporters:
        assert child.wait(timeout=15) == 0
    data = packets[-1][2].decode('utf-8', 'replace')
    assert {name for name, _, secrets in forms if leaked(data, secrets)} == set()
    for marker in ('MARKER_SYS', 'MARKER_TOOL_IN', 'MARKER_TOOL_OUT', 'MARKER_REPLY'):
        assert marker in data
















def test_sensitive_file_tool_result_names_the_path(request_doc, exporters, receiver):
    import mlflow
    from tl3_workers.tracing import observe
    uri, packets = receiver
    request_doc['mlflow'] = {'tracking_uri': uri, 'experiment': 'tl3-test'}
    with observe(request_doc):
        with mlflow.start_span(name='read', span_type='TOOL') as tool:
            tool.set_inputs({'path': 'config/.env'})
            tool.set_outputs('PLAIN=visible-env-line\nDB_PASSWORD=plain-env-296')
        with mlflow.start_span(name='read', span_type='TOOL') as tool:
            tool.set_inputs({'path': 'src/ordinary.py'})
            tool.set_outputs('ordinary-source-line')
    for child in exporters:
        assert child.wait(timeout=15) == 0
    tools = [span for span in exported_spans(packets) if span.span_type == 'TOOL']
    assert 'config/.env' in json.dumps(tools[0].outputs)
    assert b'visible-env-line' not in packets[-1][2] and b'plain-env-296' not in packets[-1][2]
    assert tools[1].outputs == 'ordinary-source-line'














def _maker_trace(mlflow):
    for step in range(30):
        with mlflow.start_span(name='chat', span_type='CHAT_MODEL') as span:
            span.set_inputs({'messages': [[{'type': 'human', 'content': f'step{step} line{j} ' + 'm' * 80}
                                           for j in range(400)]]})
            span.set_outputs({'generations': [[{'text': f'REPLY_STEP_{step} ' + 'o' * 3000}]]})
            span.set_attribute('mlflow.chat.tokenUsage', {'input_tokens': 10, 'output_tokens': 5,
                                                          'total_tokens': 15})
        for tool in range(3):
            with mlflow.start_span(name='read', span_type='TOOL') as span:
                span.set_inputs({'path': f'file{step}_{tool}.py'})
                span.set_outputs('\n'.join(f'TOOL_{step}_{tool} line {j} ' + 't' * 60 for j in range(900)))


def test_trace_budget_keeps_the_newest_turns_and_a_floor_for_older_spans(request_doc, exporters, receiver):
    import mlflow
    from tl3_workers.tracing import observe, record_reply
    uri, packets = receiver
    request_doc['mlflow'] = {'tracking_uri': uri, 'experiment': 'tl3-test'}
    request_doc['inputs']['issue']['body'] = 'ROOT_BRIEF ' + 'b' * 20000
    with observe(request_doc):
        _maker_trace(mlflow)
        record_reply({'verdict': 'ROOT_FINAL_REPLY'})
    for child in exporters:
        assert child.wait(timeout=15) == 0
    assert len(packets[-1][2]) <= 1024 * 1024
    spans = exported_spans(packets)
    models = [span for span in spans if span.span_type == 'CHAT_MODEL']
    tools = [span for span in spans if span.span_type == 'TOOL']
    assert len(models) == 30 and len(tools) == 90
    assert 'step29 line399' in json.dumps(models[-1].inputs)
    assert 'REPLY_STEP_29' in json.dumps(models[-1].outputs)
    for index, span in enumerate(tools[-3:]):
        assert f'TOOL_29_{index} line 899' in json.dumps(span.outputs)
        assert len(json.dumps(span.outputs)) > 8000, 'newest tool results keep full content'
    for span in models + tools:
        content = json.dumps([span.inputs, span.outputs])
        assert 'budget exhausted' not in content and ('step' in content or 'TOOL_' in content)
    assert 'REPLY_STEP_0 ' in json.dumps(models[0].outputs)
    root = next(span for span in spans if span.parent_id is None)
    assert 'ROOT_BRIEF' in json.dumps(root.inputs) and root.outputs['verdict'] == 'ROOT_FINAL_REPLY'
    assert root.get_attribute('tl3.trace.truncated') is True


def test_very_long_trace_keeps_its_first_and_last_spans(request_doc, exporters, receiver):
    import mlflow
    from tl3_workers.tracing import observe
    uri, packets = receiver
    request_doc['mlflow'] = {'tracking_uri': uri, 'experiment': 'tl3-test'}
    with observe(request_doc):
        for index in range(4200):
            with mlflow.start_span(name=f'n{index}', span_type='TOOL') as span:
                span.set_inputs({'i': index})
                span.set_outputs('ok')
    for child in exporters:
        assert child.wait(timeout=15) == 0
    spans = exported_spans(packets)
    names = {span.name for span in spans}
    assert len(spans) <= 4096 and {'n0', 'n4199'} <= names
    root = next(span for span in spans if span.parent_id is None)
    assert root.get_attribute('tl3.trace.omitted_spans') == 4201 - len(spans)
    ids = {span.span_id for span in spans}
    assert all(span.parent_id in ids for span in spans if span.parent_id), 'every kept span has a kept parent'


# Fourth round: the third re-check's forms and variants. They keep their own
# seed, so they import after the second round's forms.
from tracing_policy_cases import DOCUMENTED_LIMITS, V3_FORMS, VARIANTS  # noqa: E402











def test_third_recheck_forms_never_reach_the_packet(request_doc, exporters, receiver):
    import mlflow
    from tl3_workers.tracing import observe
    uri, packets = receiver
    request_doc['mlflow'] = {'tracking_uri': uri, 'experiment': 'tl3-test'}
    forms = V3_FORMS + VARIANTS
    with observe(request_doc):
        for name, text, _ in forms:
            with mlflow.start_span(name='tool', span_type='TOOL') as tool:
                tool.set_inputs({'command': text})
                tool.set_outputs(text + '\nMARKER_TOOL_OUT')
    for child in exporters:
        assert child.wait(timeout=15) == 0
    data = packets[-1][2].decode('utf-8', 'replace')
    secrets = {name: [secret] if isinstance(secret, str) else secret for name, _, secret in forms}
    assert {name for name, values in secrets.items() if leaked(data, values)} == set()
    assert 'MARKER_TOOL_OUT' in data










def test_trace_tags_recalled_cited_and_flagged_memories(request_doc, monkeypatch, exporters, receiver):
    # #422: judges score whether memories helped from these tags.
    uri, packets = receiver
    request_doc['mlflow'] = {'tracking_uri': uri, 'experiment': 'tl3-test'}
    request_doc['inputs']['memories'] = [
        {'id': 'b7e1c2d4-0000-4000-8000-000000000001', 'kind': 'lesson', 'content': 'Keep greet().'},
        {'id': 'b7e1c2d4-0000-4000-8000-000000000002', 'kind': 'run', 'content': 'Earlier run.'}]
    (Path(request_doc['worktree']) / 'greeting.py').write_text("def greet(): return 'hello'\n")
    flag = {'id': 'b7e1c2d4-0000-4000-8000-000000000001', 'claim': 'Keep greet().',
            'reason': 'greet() returns hello.',
            'evidence': {'path': 'greeting.py', 'start_line': 1, 'end_line': 1,
                         'quote': "def greet(): return 'hello'"}}
    reply = {**BRIEF, 'memory_ids': ['b7e1c2d4-0000-4000-8000-000000000001'], 'memory_flags': [flag]}
    model = FakeMessagesListChatModel(responses=[AIMessage(content=json.dumps(reply))])
    monkeypatch.setattr(runtime, 'create_model', lambda _: model)
    runtime.run(request_doc, [].append)
    for child in exporters:
        child.wait(timeout=15)
    root = next(span for span in decode(packets) if not span.parent_span_id)
    attrs = {attr.key: attr.value.string_value for attr in root.attributes}
    assert attrs['mlflow.traceTag.memory_recalled'] == (
        'b7e1c2d4-0000-4000-8000-000000000001,b7e1c2d4-0000-4000-8000-000000000002')
    assert attrs['mlflow.traceTag.memory_cited'] == 'b7e1c2d4-0000-4000-8000-000000000001'
    assert attrs['mlflow.traceTag.memory_flagged'] == 'b7e1c2d4-0000-4000-8000-000000000001'


def test_trace_has_no_memory_tags_without_memories(request_doc, monkeypatch, exporters, receiver):
    uri, packets = receiver
    run_fake(request_doc, monkeypatch, exporters, uri)
    root = next(span for span in decode(packets) if not span.parent_span_id)
    keys = {attr.key for attr in root.attributes}
    assert 'mlflow.traceTag.memory_recalled' not in keys
    assert 'mlflow.traceTag.memory_flagged' not in keys


def test_trace_exposes_selected_request_base_and_link_origin(request_doc, monkeypatch, exporters, receiver):
    uri, packets = receiver
    run_fake(request_doc, monkeypatch, exporters, uri, trace={
        'repo': 'RobTand/tl3-sandbox', 'issue': 101, 'request': '7', 'attempt': 2, 'round': 1,
        'base_sha': 'b' * 40, 'request_origin': '6'})
    root = next(span for span in decode(packets) if not span.parent_span_id)
    attrs = {attr.key: attr.value.string_value for attr in root.attributes}
    assert attrs.get('mlflow.traceTag.base_sha') == 'b' * 40
    assert attrs.get('mlflow.traceTag.request_origin') == '6'


def test_t653_controller_input_failure_exports_attribution_without_inputs(request_doc, exporters, receiver):
    from tl3_workers import tracing
    uri, packets = receiver
    request_doc['mlflow'] = {'tracking_uri': uri, 'experiment': 'tl3-test'}
    request_doc['role'] = 'reviewer'
    request_doc['inputs'] = {'role': 'reviewer', 'pointer': '/checks/0/tests/0/phases',
                             'rule': 'additionalProperties'}
    request_doc.pop('key_file', None)
    tracing.input_failure(request_doc)
    for child in exporters:
        assert child.wait(timeout=15) == 0
    root = next(span for span in decode(packets) if not span.parent_span_id)
    attrs = {attr.key: attr.value.string_value for attr in root.attributes}
    assert attrs['mlflow.traceTag.outcome'] == 'failed'
    assert attrs['mlflow.traceTag.failure_class'] == 'worker_input_schema'
    assert attrs['mlflow.traceTag.failure_message'] == (
        'worker inputs break role schema: role=reviewer pointer=/checks/0/tests/0/phases rule=additionalProperties')
    assert root.status.code == 2
    assert not root.events and not root.status.message
