"""Exercise the real graph and transport against a local MLflow-shaped receiver."""
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
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
PLANTED = [
    ('eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJmaXh0dXJlIn0.fixtureSignature296',
     'eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJmaXh0dXJlIn0.fixtureSignature296'),
    ('postgresql://u:postgres-password-296@h', 'postgres-password-296'),
    ('redis://u:redis-password-296@h', 'redis-password-296'),
    ('Authorization: Basic Zml4dHVyZTI5NjpwYXNzd29yZA==', 'Zml4dHVyZTI5NjpwYXNzd29yZA=='),
    ('GITHUB_WEBHOOK_SECRET=webhook-env-296', 'webhook-env-296'),
    ('webhook_secret: "webhook-quoted-296"', 'webhook-quoted-296'),
    ('GITHUB_TOKEN=0123456789abcdef296', '0123456789abcdef296'),
    ('{"token": "json-token-296"}', 'json-token-296'),
    ('{"secret": "json-secret-296"}', 'json-secret-296'),
    ('aws_secret_access_key=aws-lower-296', 'aws-lower-296'),
    ('AWS_SECRET_ACCESS_KEY=aws-upper-296', 'aws-upper-296'),
    ('ASIA0123456789ABCDEF', 'ASIA0123456789ABCDEF'),
    ('xapp-fixture296application', 'xapp-fixture296application'),
    ('pypi-fixture296package', 'pypi-fixture296package'),
    (json.dumps(json.dumps({'password': 'nested-password-296'})), 'nested-password-296'),
    ('credential_value="credential-296"', 'credential-296'),
    ('key_value=key-value-296', 'key-value-296'),
]


@pytest.mark.parametrize('planted,secret', PLANTED)
def test_redacts_review_credential_forms(planted, secret):
    from tl3_workers.tracing import Redactor
    result = Redactor(None).payload({'content': 'visible-before ' + planted + ' visible-after'})
    assert secret not in json.dumps(result)
    assert 'visible-after' in json.dumps(result)


# Forms a second probe found after the review round. Header values run to the end
# of their line, so these sit on a line of their own.
PLANTED_LINES = [
    ('Authorization: Token abcdef296token', 'abcdef296token'),
    ('Authorization: Digest username="u", response="deadbeef296"', 'deadbeef296'),
    ('Authorization: AWS4-HMAC-SHA256 Credential=AKIAXXXXXXXXXXXXXXXX/20251101/x, Signature=sig296aws',
     'sig296aws'),
    ('Proxy-Authorization: Negotiate neg296tok', 'neg296tok'),
    ('Cookie: session=cookie296abc; other=1', 'cookie296abc'),
    ('Set-Cookie: sid=setcookie296; Path=/', 'setcookie296'),
    ('{"password": "ab\\"cd296esc"}', 'cd296esc'),
    ('sk_live_abcdefABCDEF296ghij', 'abcdefABCDEF296ghij'),
    ('rk_test_abcdefABCDEF296ghij', 'abcdefABCDEF296ghij'),
    ('AIzaSyA-abcdefghijklmnopqrstuvwxyz01234', 'abcdefghijklmnopqrstuvwxyz01234'),
    ('glpat-abcdefghij296klmnop', 'abcdefghij296klmnop'),
    ('npm_abcdefghijklmnopqrstuvwxyz0123456789', 'abcdefghijklmnopqrstuvwxyz0123456789'),
    ('SG.abcdefghij296.klmnopqrstuv296', 'klmnopqrstuv296'),
    ('hf_abcdefghijklmnopqrstuvwxyz0123', 'abcdefghijklmnopqrstuvwxyz0123'),
    ('-----BEGIN PGP PRIVATE KEY BLOCK-----\nlQOYBF296\n-----END PGP PRIVATE KEY BLOCK-----', 'lQOYBF296'),
    ('https://x.example/a?X-Amz-Signature=sigq296&b=1', 'sigq296'),
    ('curl --password hunter296 http://x', 'hunter296'),
    ('curl -u admin:clipw296 http://x', 'clipw296'),
    ('https://hooks.slack.com/services/T000/B000/slackhook296', 'slackhook296'),
    ('https://discord.com/api/webhooks/123/discordtok296', 'discordtok296'),
    (json.dumps({'k': 'secret:\tjson-tab-296'}), 'json-tab-296'),
    (json.dumps({'k': 'password:\n  json-newline-296'}), 'json-newline-296'),
]


@pytest.mark.parametrize('planted,secret', PLANTED_LINES)
def test_redacts_further_credential_forms(planted, secret):
    from tl3_workers.tracing import Redactor
    result = json.dumps(Redactor(None).payload({'content': 'visible-before\n' + planted + '\nvisible-after'}))
    assert secret not in result
    assert 'visible-before' in result and 'visible-after' in result


@pytest.mark.parametrize('document', [
    json.dumps({'k': json.dumps({'password': 'ab"cd-nested-296'}), 'note': 'kept'}),
    json.dumps({'token': ['list-secret-296', 'other'], 'note': 'kept'}),
    json.dumps(json.dumps({'headers': {'private_key': 12345296}, 'note': 'kept'})),
])
def test_json_text_is_judged_by_field_names_however_deeply_quoted(document):
    # A string that is wholly JSON is parsed, so quoting depth and value shape
    # cannot hide a sensitive field. Text that merely contains JSON relies on
    # the patterns above.
    from tl3_workers.tracing import Redactor
    result = Redactor(None).payload({'content': document})['content']
    for secret in ('cd-nested-296', 'list-secret-296', '12345296'):
        assert secret not in result
    assert 'kept' in result


@pytest.mark.parametrize('key', ['OPENAI_API_KEY', 'x-api-key', 'github_token', 'webhook_secret',
                                 'secret', 'token', 'passwd_hash', 'private_key_id', 'credential_id',
                                 'Cookie', 'Set-Cookie', 'AUTHORIZATION'])
def test_redacts_sensitive_key_substrings(key):
    from tl3_workers.tracing import Redactor
    assert Redactor(None).payload({key: 'opaque-sensitive-value-296'})[key] == '[REDACTED]'


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


def test_payload_preserves_recent_messages_and_argument_tails():
    from tl3_workers.tracing import Redactor
    redactor = Redactor(None)
    messages = [{'role': 'system', 'content': ('old-system-line\n' * 20000)}]
    messages += [{'role': 'user', 'content': f'turn-{i}\n' + 'old\n' * 10000 + f'newest-{i}'}
                 for i in range(8)]
    result = redactor.payload({'messages': messages})
    assert isinstance(result.get('messages'), list), 'keep message structure'
    assert 'newest-7' in json.dumps(result), 'keep the latest turn'
    assert 'truncated' in json.dumps(result).lower()
    args = redactor.payload({'path': 'file.txt', 'argument': 'old\n' * 50000 + 'argument-tail',
                             'result': 'old\n' * 50000 + 'result-tail'})
    assert args['path'] == 'file.txt'
    assert 'argument-tail' in json.dumps(args['argument'])
    assert 'result-tail' in json.dumps(args['result'])
    assert len(json.dumps(args, ensure_ascii=False).encode()) <= 65536


def test_oversized_strings_bound_redaction_work_without_cut_secret_leaks(monkeypatch):
    from tl3_workers.tracing import Redactor
    redactor = Redactor(None)
    original = redactor.text
    seen = []
    def text(value):
        seen.append(len(value))
        return original(value)
    monkeypatch.setattr(redactor, 'text', text)
    secret = 'sk-' + 'A' * 100000
    result = redactor.payload({'content': 'old\n' * 3000000 + secret + '\nlatest-visible-line'})
    assert max(seen) <= 65536, 'regex must never process the full oversized string'
    assert 'latest-visible-line' in json.dumps(result)
    assert 'A' * 32 not in json.dumps(result), 'no partial credential at the cut'
    private = '-----BEGIN PRIVATE KEY-----\n' + 'A\n' * 100000 + '-----END PRIVATE KEY-----\nvisible-tail'
    assert 'A\\n' not in json.dumps(redactor.payload(private))


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


def test_json_model_reply_keeps_readable_tail_without_escaped_secrets():
    from tl3_workers.tracing import Redactor
    reply = json.dumps({'kind': 'brief', 'brief': 'earlier paragraph\n' * 20000 + 'latest-reply-paragraph',
                        'webhook_secret': 'opaque-json-reply-secret-296'})
    result = Redactor(None).payload({'content': reply})
    assert 'latest-reply-paragraph' in result['content']
    assert 'opaque-json-reply-secret-296' not in json.dumps(result)
    assert isinstance(json.loads(result['content']), dict), 'keep the structured model reply readable'
    assert len(json.dumps(result).encode()) <= 65536


# Third review round. The reviewer's 245 planted forms each pass through every
# way a string reaches a span. Only the limits that workers/README.md lists may leak.
from tracing_forms import FORMS, B64, r  # noqa: E402

DOCUMENTED_LIMITS = {'DOC_unquoted_spaces', 'url_no_scheme', 'generic_webhook_path', 'cli_gh_with_token'}
CARRIERS = {
    'text': lambda R, F: R.text(F),
    'payload': lambda R, F: R.payload(F),
    'line': lambda R, F: R.payload('lead line\n' + F + '\ntrailer line that follows'),
    'inline': lambda R, F: R.payload('I looked at ' + F + ' and then moved on.'),
    'dict_value': lambda R, F: R.payload({'note': F}),
    'json': lambda R, F: R.payload(json.dumps({'text': F})),
    'json_twice': lambda R, F: R.payload(json.dumps(json.dumps({'text': F}))),
    'json_thrice': lambda R, F: R.payload(json.dumps(json.dumps(json.dumps({'text': F})))),
    'tool_args': lambda R, F: R.payload({'tool_calls': [{'name': 'bash', 'args': {'command': F}}],
                                         'messages': [{'role': 'user', 'content': F}]}),
    'tool_result_json': lambda R, F: R.payload([{'role': 'tool', 'content': json.dumps({'stdout': F})}]),
    'function_arguments': lambda R, F: R.payload({'tool_calls': [{'function': {
        'name': 'bash', 'arguments': json.dumps({'command': F})}}]}),
}


def leaked(output, secrets, width=8):
    flat = output if isinstance(output, str) else json.dumps(output, ensure_ascii=False)
    return [secret for secret in secrets
            if any(secret[i:i + width] in flat for i in range(max(1, len(secret) - width + 1)))]


@pytest.mark.parametrize('name,text,secrets', [(name, text, secrets) for name, text, secrets, _ in FORMS
                                               if name not in DOCUMENTED_LIMITS],
                         ids=[name for name, *_ in FORMS if name not in DOCUMENTED_LIMITS])
def test_planted_form_never_leaks_through_any_carrier(name, text, secrets):
    from tl3_workers.tracing import Redactor
    redactor = Redactor('/nonexistent/key-file-path')
    leaks = {carrier: leaked(build(redactor, text), secrets) for carrier, build in CARRIERS.items()}
    assert not any(leaks.values()), leaks


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


ORDINARY = [
    'def test_password_reset_sends_email(client):',
    'The password must be at least 12 characters and contain a digit.',
    'Users reset their password by clicking the link in the email.',
    'for token in tokens:\n    count += 1',
    'self.token_count += len(tokens)',
    'max_tokens=4096',
    'sorted(items, key=lambda kv: kv[1])',
    'max(words, key=len)',
    'if token == expected:',
    'def verify(token: str, key: bytes) -> bool:',
    '# TODO: rotate the API key every 90 days',
    'class TokenBucket:\n    def __init__(self, capacity, rate):',
    "tokenizer = AutoTokenizer.from_pretrained('gpt2')",
    "assert 'password' in error_message",
    "git commit -m 'Fix password hashing'",
    "key = d.get('name')",
    'input_tokens: 123, output_tokens: 456',
    'The cache key is derived from the file path.',
    'export PATH=/usr/bin:$PATH',
    'HTTP 401: invalid token',
    'ssh -p 2222 host',
    'pytest -p no:cacheprovider -q workers/tests',
    "python -m pytest -k 'test_token or test_key' -q",
    'primary_key=True, sort_keys=True',
    'https://github.com/RobTand/trustladderiii/pull/302',
    'git@github.com:RobTand/trustladderiii.git',
    'ssh://git@github.com/o/r.git',
    'http://localhost:8080/path@2x.png',
    'email bob@example.com: thanks',
    'let token = self.next_token()?;',
    'for (key, value) in map.iter() {',
    'fn parse_token(s: &str) -> Option<Token> {',
    'The authorization header is required for this endpoint.',
    'Missing password for user bob; ask the requester.',
    '- [ ] rotate secrets quarterly',
    'secret handshake protocol (SHP) v2 is documented in docs/shp.md',
    'a passkey is not a password',
    'use `--token-budget 4000` to cap spend',
    'Cookie banner copy lives in web/consent.html',
    'commit 29280f4b147fd889f848c51300ba3352b9e26d9d and 3f14146feb2902d35819403c0a83bb2a0ead9a36',
    'sha256:81a462f2634e11c5372eb8822209d17cf6ca5ad653cda0315171c3a26b396cfe',
    'uuid 6f59aec1-88d6-170c-0ce7-0fdb4eebbb70',
    'Bearer tokens are described in RFC 6750.',
    'Basic usage: run `tl3 start`.',
    'basic authentication is disabled',
    'a key-value store',
    'keyboard shortcuts: ctrl+s to save',
    'monkey patch the key function',
    'tokens per second: 41.7',
    'trace tr-6f59aec188d6170c0ce70ecdb4fde685, request cdc57fa99768407bb915874b3d9fde54',
    'integrity sha512-Q2xvdWRmbGFyZS1zZWN1cml0eS10ZXN0LXZhbHVlLWZvci1pbnRlZ3JpdHk=',
]


@pytest.mark.parametrize('text', ORDINARY)
def test_ordinary_text_survives_redaction(text):
    from tl3_workers.tracing import Redactor
    redactor = Redactor('/nonexistent/key-file-path')
    assert redactor.text(text) == text
    assert redactor.payload(text) == text


def test_usage_counts_and_harmless_token_fields_survive():
    from tl3_workers.tracing import Redactor
    usage = {'input_tokens': 5, 'output_tokens': 6, 'total_tokens': 11, 'cache_read_input_tokens': 3,
             'cache_creation_input_tokens': 2, 'max_tokens': 4096, 'budget_tokens': 32000,
             'token_count': 12, 'tokenizer': 'gpt2', 'key': 'name', 'primary_key': 'id',
             'input_token_details': {'cache_read': 8, 'cache_creation': 2},
             'usage_metadata': {'input_tokens': 9, 'output_token_details': {'reasoning': 4}}}
    assert Redactor(None).payload(usage) == usage


@pytest.mark.parametrize('document,secret', [
    ({'env': [{'name': 'DB_PASSWORD', 'value': 'k8s-json-env-296'}]}, 'k8s-json-env-296'),
    ({'secret': 123456789012}, '123456789012'),
    ({'auth': 'Ym9iOmh1bnRlcjI5Ng=='}, 'Ym9iOmh1bnRlcjI5Ng=='),
])
def test_structured_secrets_are_redacted_by_field(document, secret):
    from tl3_workers.tracing import Redactor
    assert secret not in json.dumps(Redactor(None).payload(document))


def test_high_entropy_tokens_are_redacted_but_digests_survive():
    from tl3_workers.tracing import Redactor
    redactor = Redactor(None)
    for secret in [r(40, B64), r(24), r(32, '0123456789abcdef'), r(56, B64)]:
        assert leaked(redactor.text(f'the value {secret} was set'), [secret]) == []
        assert 'the value' in redactor.text(f'the value {secret} was set')


SENSITIVE_PATHS = ['.env', 'config/.env.local', 'deploy/server.pem', 'tls/site.key', '/home/u/.ssh/id_ed25519',
                   '.netrc', 'web/.npmrc', '.pypirc', 'aws/credentials', 'k8s/db-secret.yaml',
                   '/home/u/.docker/config.json', 'keys/putty.ppk', 'certs/client.p12', '/home/u/.kube/config',
                   # Third re-check: match path components, not just base names.
                   'config/prod.env', 'deploy/app.env', 'deploy/new.env', 'env.production', 'env', 'id_rsa.bak',
                   'id_rsa.old', 'id_ed25519.orig', 'keystore.jks', 'app.keystore', 'signing.p8', 'key.pk8',
                   'secrets/prod.yml', 'ops/credentials/db.txt', '.dockercfg', 'terraform.tfvars',
                   'prod.auto.tfvars', 'terraform.tfstate', 'terraform.tfstate.backup', '.htpasswd', '.my.cnf',
                   'service-account.json', 'service-account-prod.json', ' .env']


@pytest.mark.parametrize('path', SENSITIVE_PATHS)
def test_content_from_sensitive_files_is_replaced_by_a_marker(path):
    from tl3_workers.tracing import Redactor
    redactor = Redactor(None)
    grep = redactor.payload({'matches': [{'path': path, 'line': 1, 'text': 'visible-sensitive-line'},
                                         {'path': 'src/app.py', 'line': 2, 'text': 'ordinary-line'}]})
    assert 'visible-sensitive-line' not in json.dumps(grep)
    assert path in json.dumps(grep) and 'ordinary-line' in json.dumps(grep)
    messages = redactor.payload({'messages': [
        {'type': 'ai', 'content': '', 'tool_calls': [{'name': 'read', 'args': {'path': path}, 'id': 'call-s'},
                                                    {'name': 'read', 'args': {'path': 'README.md'}, 'id': 'call-o'}]},
        {'type': 'tool', 'tool_call_id': 'call-s', 'content': 'visible-sensitive-line'},
        {'type': 'tool', 'tool_call_id': 'call-o', 'content': 'ordinary-line'}]})
    assert 'visible-sensitive-line' not in json.dumps(messages)
    assert 'ordinary-line' in json.dumps(messages)


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


def _filler(count, width=70):
    return '\n'.join(f'filler {i} ' + 'f' * width for i in range(count))


@pytest.mark.parametrize('begin,end', [
    ('-----BEGIN RSA PRIVATE KEY-----', '-----END RSA PRIVATE KEY-----'),
    ('-----BEGIN OPENSSH PRIVATE KEY-----', '-----END OPENSSH PRIVATE KEY-----'),
    ('-----BEGIN PGP PRIVATE KEY BLOCK-----\nVersion: x\n', '-----END PGP PRIVATE KEY BLOCK-----'),
    ('-----BEGIN ENCRYPTED PRIVATE KEY-----', '-----END ENCRYPTED PRIVATE KEY-----'),
    ('api_key: |', 'next: 1'),
])
def test_multiline_secret_straddling_a_cut_never_leaks(begin, end):
    from tl3_workers.tracing import Redactor
    redactor = Redactor(None)
    indent = '  ' if begin.startswith('api_key') else ''
    body = [r(64, B64) for _ in range(40)]
    key = begin + '\n' + '\n'.join(indent + line for line in body) + '\n' + end
    bad = [fill for fill in range(60, 135)
           if leaked(redactor.text(_filler(150) + '\n' + key + '\n' + _filler(fill, 60)), body)
           or leaked(redactor.payload(_filler(150) + '\n' + key + '\n' + _filler(fill, 60)), body)]
    assert bad == []


def test_single_line_secrets_straddling_any_cut_never_leak():
    from tl3_workers.tracing import Redactor
    redactor = Redactor(None)
    model_key = 'zz' + r(40) + '-mk'
    redactor.secrets.add(model_key)
    password = 'Qx' + r(22)
    for fill in range(60, 135):
        text = _filler(150) + f'\npassword={password}\nx {model_key} y\n' + _filler(fill, 60)
        assert leaked(redactor.text(text), [password, model_key]) == []
    # One oversized line is cut twice, keeping its head and its tail. Sweep the
    # secret across the whole line so it straddles both cuts wherever they fall.
    for offset in range(0, 9000, 5):
        line = 'A' * offset + f' password={password} x {model_key} y ' + 'B' * (9000 - offset)
        assert leaked(redactor.text(line), [password, model_key]) == [], offset


def test_oversized_single_line_keeps_its_head_and_tail():
    from tl3_workers.tracing import Redactor
    redactor = Redactor(None)
    result = redactor.payload('HEAD_MARK ' + 'z' * 20000 + ' FINAL_ONE_LINE_MARK')
    assert result.startswith('HEAD_MARK') and result.endswith('FINAL_ONE_LINE_MARK')
    assert 'truncated' in result
    args = redactor.payload({'command': 'START_ARG ' + 'a' * 30000 + ' ENDARG'})
    assert args['command'].startswith('START_ARG') and args['command'].endswith('ENDARG')
    reply = redactor.payload({'text': 'REPLY_HEAD ' + 'o' * 3000}, 2048)
    assert 'REPLY_HEAD' in json.dumps(reply)


ADVERSARIAL = ['key', 'token', 'token=', 'secret_', 'password', 'credential', 'signature', 'a.', 'a-', 'a:',
               'a://', 'sk-', 'Bearer ', 'authorization:', 'Authorization: Basic ', 'cookie=', '"password": ',
               '\\"', '\\\\\\"', "'", '--password ', '-u a:', '-u ', 'https://u:', 'eyJ.', 'ghp_', 'AKIA',
               '-----BEGIN PRIVATE KEY-----\n', '-----BEGIN A A A ', ' ', '\t', '\\n', 'token\\n', 'key=\\n ',
               '= ', 'x_token_', 'secret secret ', 'key=key=', 'password:  ', 'password="', '| ', '[a]\n',
               'name: a\n', 'x: |\n', 'mysql ', 'Private-Lines: 1\n', 'aB3', '//', ':@',
               # Third re-check: a run of backslashes was quadratic.
               '\\', '\\\\', '\\\\\\', 'password is ', 'token ', 'IDENTIFIED BY ', '<password>',
               'a,password,', '"name": "DB_PASSWORD", ', '\tTRUE\t', 'net use ', '-db-password ']


@pytest.mark.parametrize('unit', ADVERSARIAL)
def test_redaction_is_linear_on_adversarial_text(unit):
    import time
    from tl3_workers.tracing import Redactor
    redactor = Redactor(None)
    text = (unit * (8192 // len(unit) + 1))[:8192]
    started = time.process_time()
    redactor.text(text)
    redactor.text('x\n' + text)
    # The quadratic label pattern this replaces took 2 s per call on dl380g10.
    assert time.process_time() - started < 0.5


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
import tracing_forms3  # noqa: E402

V3_DOCUMENTED = {
    'kubectl_get_secret_jsonpath_decoded',  # a bare value on the line after a command
    'env_uncommon_hex64', 'env_hmac_seed_hex64', 'env_openssl_rand_hex_output', 'hex_in_json_secret_bytes',  # 64-hex
    'unicode_escaped_json',  # a label spelled with JSON escapes inside other text
}
V3_FORMS = [(name, text, secrets) for name, text, secrets, _ in tracing_forms3.FORMS if name not in V3_DOCUMENTED]


@pytest.mark.parametrize('name,text,secrets', V3_FORMS, ids=[name for name, *_ in V3_FORMS])
def test_third_recheck_form_never_leaks_through_any_carrier(name, text, secrets):
    from tl3_workers.tracing import Redactor
    redactor = Redactor('/nonexistent/key-file-path')
    leaks = {carrier: leaked(build(redactor, text), secrets) for carrier, build in CARRIERS.items()}
    assert not any(leaks.values()), leaks


def _variants():
    r, pw = tracing_forms3.r, lambda: tracing_forms3.r(20, tracing_forms3.PWCH)
    alnum, hex_ = tracing_forms3.ALNUM, tracing_forms3.HEX
    found = []

    def v(name, template, secret):
        found.append((name, template % secret, secret))
    v('kubectl_pod_json_env_pretty', '$ kubectl get pod web -o json\n{\n  "spec": {"containers": [{"env": [\n    {\n'
      '      "name": "DB_PASSWORD",\n      "value": "%s"\n    }\n  ]}]}\n}', pw())
    v('json_key_value_inline', 'vars: {"key":"DB_PASSWORD","value":"%s"} end', pw())
    v('json_key_value_escaped', 'out: {\\"name\\": \\"API_TOKEN\\", \\"value\\": \\"%s\\"}', pw())
    v('form_body_pct_leading', 'username=bob&password=%s&client=cli', '%21' + r(16, alnum))
    v('sql_with_password', "CREATE USER orders_svc WITH PASSWORD '%s';", pw())
    v('sql_alter_user', "ALTER USER app WITH PASSWORD '%s';", pw())
    v('sql_create_role_login', "CREATE ROLE svc LOGIN PASSWORD '%s' NOSUPERUSER;", pw())
    v('sql_mysql_set_password', "SET PASSWORD FOR 'app'@'%%' = PASSWORD('%s');", pw())
    v('sql_identified_by', "CREATE USER 'app'@'%%' IDENTIFIED BY '%s';", pw())
    v('xml_element', '<server><id>nexus</id><username>deploy</username><password>%s</password></server>', pw())
    v('xml_maven_settings', '<settings><servers><server>\n  <id>central</id>\n  <username>deploy</username>\n'
      '  <password>%s</password>\n</server></servers></settings>', pw())
    v('xml_hibernate_property', '<property name="hibernate.connection.password">%s</property>', pw())
    v('csv_password_column', 'username,password,role\nbob,%s,admin', pw())
    v('csv_row_with_header_password', 'id,email,password_hash,password\n1,a@b.io,x,%s', pw())
    v('prose_password_is', 'The password for the staging database is %s - please rotate it.', pw())
    v('prose_pw_is', 'the db pw is %s', pw())
    v('prose_use_password', 'You can log in with user admin and password %s.', pw())
    v('prose_password_used', 'FATAL: password authentication failed for user "app" (password used: %s)', pw())
    v('prose_login_pair', 'Login: admin / %s', pw())
    v('redis_requirepass', 'requirepass %s', pw())
    v('redis_auth_command', 'AUTH %s', pw())
    v('go_flag_password', 'myapp -db-password %s -port 8080', pw())
    v('single_dash_flag', 'tool -pass %s', pw())
    v('powershell_flag', 'Connect-Server -Password "%s"', pw())
    v('powershell_securestring', "$p = ConvertTo-SecureString '%s' -AsPlainText -Force", pw())
    v('net_use', 'net use \\\\srv\\share /user:CORP\\bob %s', pw())
    v('htpasswd_apr1', 'deploy:$apr1$h7Gk2dQ1$%s', r(22, alnum))
    v('netscape_cookie_jar', '.acme.io\tTRUE\t/\tTRUE\t1893456000\tsession_id\t%s', r(40, hex_))
    v('airtable_pat', 'token %s', 'pat' + r(14) + '.' + r(64, hex_))
    return found


VARIANTS = _variants()


@pytest.mark.parametrize('name,text,secret', VARIANTS, ids=[name for name, *_ in VARIANTS])
def test_third_recheck_variant_is_redacted(name, text, secret):
    from tl3_workers.tracing import Redactor
    redactor = Redactor(None)
    for carrier in (text, 'lead\n' + text + '\ntrail', [{'role': 'tool', 'content': text}]):
        assert leaked(redactor.payload(carrier), [secret]) == [], carrier


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


@pytest.mark.parametrize('path', ['docs/secret-scanning.md', 'tests/test_secret_redaction.py', 'src/credentials.rs',
                                  'keys/id_rsa.pub', 'src/id_map.py', 'README.md', 'config/settings.toml'])
def test_ordinary_files_keep_their_content(path):
    from tl3_workers.tracing import Redactor
    grep = Redactor(None).payload({'matches': [{'path': path, 'line': 1, 'text': 'ordinary-line'}]})
    assert 'ordinary-line' in json.dumps(grep)


def test_backslash_payloads_stay_linear():
    import time
    from tl3_workers.tracing import Redactor
    redactor = Redactor(None)
    started = time.process_time()
    redactor.payload({f'field{i}': '\\' * 65536 for i in range(8)})
    # Before the fix, one 8 KiB window of backslashes took 0.57 s on dl380g10.
    assert time.process_time() - started < 1.0


ORDINARY_V3 = [
    "reply={'ok': True, 'scope_id': unit, 'token': 'b'*64,",
    'token = "w" * 32',
    'env_path.write_text("GF_SECURITY_ADMIN_PASSWORD=" + password + "\\nGF_USERS_DEFAULT_THEME=dark")',
    'ssh-keygen -q -t ed25519 -N "" -C "pb-smoke3-$stamp" -f "$ctx/id_smoke" || exit 2',
    '"Authorization": self.authorization, "Content-Type": "application/json"})',
    'scope `prismabuild-jobd3c6002f82352acc91a2998304cbe0e2.slice` on Sparklina.',
    "'uuid': 'GPU-b1eceeea-fec7-371e-2cf3-cd10f2e7b705',",
    'receipt      8ff38b7dc24d51c63e4147dbe1587cf1...   cas_receipt.v3',
    '"10.batch|00:00:00|00:00:00|20164608|5600628|33215|1|"',
    'if not password:\n    continue',
    'Called after release with the same attempt token: reports',
    '"unit": "pb-test-inventory-9cb03d044a0faeabe29b.service",',
    'Rotate the token every 90 days from the admin console.',
    'password = getpass("Password: ")',
    'The token expires after 3600 seconds unless refreshed.',
    'Secret rotation runs nightly at 02:00 UTC for every service.',
    'Store the password in the vault, not in the repository.',
    'let token = tokens.next().ok_or(Error::MissingToken)?;',
    'username,password_hash,role',
]


@pytest.mark.parametrize('text', ORDINARY_V3)
def test_ordinary_text_from_the_recheck_corpus_survives(text):
    from tl3_workers.tracing import Redactor
    redactor = Redactor('/nonexistent/key-file-path')
    assert redactor.text(text) == text
    assert redactor.payload(text) == text
