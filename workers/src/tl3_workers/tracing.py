"""Observational MLflow capture with bounded, redacted content and detached export."""
from contextlib import contextmanager
from contextvars import ContextVar
import json
import logging
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import tomllib


active = ContextVar('tl3_trace_capture', default=None)
PAYLOAD_BYTES = 65536
TOKEN_FIELDS = ('input_tokens', 'output_tokens', 'total_tokens',
                'cache_read_input_tokens', 'cache_creation_input_tokens')
SENSITIVE_KEY = re.compile(
    r'(?i)(?:api[_-]?key|authorization|password|passwd|access[_-]?token|refresh[_-]?token|'
    r'client[_-]?secret|private[_-]?key|model[_-]?key|key[_-]?file|credentials?)')
SECRET_PATTERNS = [
    re.compile(r'-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY-----.*?'
               r'(?:-----END (?:[A-Z0-9]+ )*PRIVATE KEY-----|$)', re.DOTALL),
    re.compile(r'(?i)\bBearer\s+[A-Za-z0-9._~+/=-]+'),
    re.compile(r'\b(?:sk-[A-Za-z0-9_-]+|gh[pousr]_[A-Za-z0-9_]+|github_pat_[A-Za-z0-9_]+|'
               r'AKIA[A-Z0-9]{16}|xox[baprs]-[A-Za-z0-9-]+)\b'),
    re.compile(r'(?i)https?://[^/\s:@]+:[^@\s/]+@'),
    re.compile(r'(?i)["\']?(?:api[_-]?key|password|passwd|access[_-]?token|client[_-]?secret)'
               r'["\']?\s*[:=]\s*["\']?[^\s"\',;}&]+'),
]


class Redactor:
    def __init__(self, key_path):
        # The runtime supplies an already-loaded key. Tracing never opens key files.
        self.secrets = {key_path} if key_path else set()

    def text(self, value):
        for secret in sorted(self.secrets, key=len, reverse=True):
            value = value.replace(secret, '[REDACTED]')
        for pattern in SECRET_PATTERNS:
            value = pattern.sub('[REDACTED]', value)
        return value

    def clean(self, value):
        if isinstance(value, str):
            return self.text(value)
        if isinstance(value, dict):
            return {self.text(str(key)): '[REDACTED]' if SENSITIVE_KEY.fullmatch(str(key))
                    else self.clean(item) for key, item in value.items()
                    if key not in {'response_metadata', 'additional_kwargs', 'headers'}}
        if isinstance(value, list):
            return [self.clean(item) for item in value]
        return value

    def payload(self, value):
        value = self.clean(value)
        encoded = json.dumps(value, ensure_ascii=False).encode()
        if len(encoded) > PAYLOAD_BYTES:
            # JSON-escaping this prefix can at most double its size.
            value = {'preview': encoded[:PAYLOAD_BYTES // 4].decode(errors='ignore'),
                     'truncated': True, 'original_bytes': len(encoded)}
        return value


def record_model_key(key):
    if capture := active.get():
        capture.redactor.secrets.add(key)


def record_chunk_usage(chunk):
    if active.get() is None or not chunk.get('usage'):
        return
    try:
        import mlflow
        span = mlflow.get_current_active_span()
        if span is None or span.span_type not in {'LLM', 'CHAT_MODEL'}:
            return
        usage = chunk['usage']
        counts = {key: value for key in ('prompt_tokens', 'completion_tokens', 'total_tokens',
                                         'cache_write_tokens')
                  if type(value := usage.get(key)) is int and value >= 0}
        for key, fields in [('prompt_tokens_details', ('cached_tokens', 'cache_write_tokens')),
                            ('completion_tokens_details', ('reasoning_tokens',))]:
            if isinstance(details := usage.get(key), dict):
                counts[key] = {field: value for field in fields
                               if type(value := details.get(field)) is int and value >= 0}
        chunks = span.get_attribute('tl3.proxy.usage_chunks') or []
        if len(chunks) < 32:
            span.set_attribute('tl3.proxy.usage_chunks', [*chunks, counts])
        else:
            span.set_attribute('tl3.proxy.usage_chunks_truncated', True)
    except Exception:
        failure()


def record_reply(reply):
    if capture := active.get():
        try:
            capture.root.set_outputs(reply)
        except Exception:
            failure()


def failure():
    print('tl3_workers: MLflow export failed', file=sys.stderr)


class Capture:
    def __init__(self, request):
        import mlflow
        import mlflow.langchain
        from mlflow.entities import MlflowExperimentLocation
        from mlflow.tracing import provider
        from mlflow.tracing.processor.mlflow_v3 import MlflowV3SpanProcessor
        from opentelemetry.sdk.trace import TracerProvider
        from opentelemetry.sdk.trace.export import SpanExporter, SpanExportResult

        self.redactor = Redactor(request.get('key_file'))
        self.role = request['role']
        self.model = request['model']['name']
        self.spans = []
        self.dropped = False
        capture = self
        logger = logging.getLogger('mlflow')
        logger.handlers = [logging.NullHandler()]
        logger.propagate = False

        class Exporter(SpanExporter):
            def export(self, spans):
                for span in spans:
                    if len(capture.spans) >= 4096:
                        capture.dropped = True
                        break
                    capture.spans.append(safe_span(span, capture.redactor, capture.role, capture.model))
                return SpanExportResult.SUCCESS

        # MLflow 3.17.2 has no public in-memory destination. Pin this adapter to
        # that release: the stock exporter starts network threads and joins at exit.
        tracer = TracerProvider(shutdown_on_exit=False)
        tracer.add_span_processor(MlflowV3SpanProcessor(Exporter(), export_metrics=False))
        provider._MLFLOW_TRACE_USER_DESTINATION.set(MlflowExperimentLocation(experiment_id='0'))
        provider.provider.once.do_once(lambda: provider.provider.set(tracer))
        provider.provider.set(tracer)
        mlflow.langchain.autolog(silent=True)
        self.context = mlflow.start_span(name='tl3_' + self.role, span_type='AGENT')
        self.root = self.context.__enter__()
        self.root.set_inputs(request['inputs'])
        trace = request.get('trace', {})
        self.tags = {key: str(trace.get(key, 'unknown'))
                     for key in ('repo', 'issue', 'request', 'attempt', 'round')}
        self.tags.update(node=str(request['operation']['node']),
                         generation=str(request['operation']['generation']),
                         role=request['role'], model=request['model']['name'],
                         effort=request['model']['effort'] or 'default')
        self.settings = request['mlflow']

    def finish(self, outcome):
        self.context.__exit__(None, None, None)
        from mlflow.tracing.trace_manager import InMemoryTraceManager
        import mlflow.langchain

        InMemoryTraceManager.get_instance().pop_trace(self.root._span.context.trace_id)
        mlflow.langchain.autolog(disable=True, silent=True)
        if self.dropped:
            failure()
            return
        usage = {'input_tokens': 0, 'output_tokens': 0, 'total_tokens': 0}
        for span in self.spans:
            for attr in span.attributes:
                if attr.key == 'mlflow.chat.tokenUsage':
                    for item in attr.value.kvlist_value.values:
                        usage[item.key] = usage.get(item.key, 0) + item.value.int_value
        self.tags.update(outcome=outcome, **{key: str(value) for key, value in usage.items()})
        root = next(span for span in self.spans if not span.parent_span_id)
        root.status.code = 1 if outcome == 'succeeded' else 2
        for key, value in self.tags.items():
            root.attributes.add(key='mlflow.traceTag.' + key).value.string_value = self.redactor.text(value)[:512]
        from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import ExportTraceServiceRequest
        packet = ExportTraceServiceRequest()
        packet.resource_spans.add().scope_spans.add().spans.extend(self.spans)
        # An unlinked file bounds handoff and avoids pipe backpressure. The export
        # child owns network waits; the worker never waits for it, even at shutdown.
        with tempfile.TemporaryFile() as payload:
            payload.write(json.dumps(self.settings).encode() + b'\n')
            payload.write(packet.SerializeToString())
            payload.seek(0)
            subprocess.Popen([sys.executable, str(Path(__file__))], stdin=payload,
                             stdout=subprocess.DEVNULL, stderr=sys.stderr, close_fds=True)


def safe_span(span, redactor=None, role="unknown", model="unknown"):
    from opentelemetry.proto.trace.v1.trace_pb2 import Span
    from mlflow.tracing.utils.otlp import _set_otel_proto_anyvalue

    redactor = redactor or Redactor(None)
    kind = json.loads(span.attributes.get('mlflow.spanType', '"CHAIN"'))
    if kind not in {'LLM', 'CHAT_MODEL', 'TOOL', 'AGENT', 'CHAIN', 'RETRIEVER'}:
        kind = 'CHAIN'
    result = Span(trace_id=span.context.trace_id.to_bytes(16, 'big'),
                  span_id=span.context.span_id.to_bytes(8, 'big'),
                  parent_span_id=span.parent.span_id.to_bytes(8, 'big') if span.parent else b'',
                  name=redactor.text(
                      f'tl3_{role}:{model}' if kind in {'LLM', 'CHAT_MODEL'} else span.name
                  )[:256], start_time_unix_nano=span.start_time,
                  end_time_unix_nano=span.end_time)
    result.status.code = 2 if span.status.status_code.name == 'ERROR' else 1
    result.attributes.add(key='mlflow.spanType').value.string_value = kind
    result.attributes.add(key='tl3.role').value.string_value = redactor.text(role)[:256]
    if kind in {'LLM', 'CHAT_MODEL'}:
        result.attributes.add(key='mlflow.llm.model').value.string_value = redactor.text(model)[:256]
    # Retain only content and usage. Headers, provider metadata, resources and
    # raw exceptions stay local. Apply redaction before truncating or serializing.
    for key in ('mlflow.spanInputs', 'mlflow.spanOutputs',
                'tl3.proxy.usage_chunks', 'tl3.proxy.usage_chunks_truncated'):
        if (raw := span.attributes.get(key)) is not None:
            value = redactor.payload(json.loads(raw))
            _set_otel_proto_anyvalue(result.attributes.add(key=key).value, value)
    if kind in {'LLM', 'CHAT_MODEL'} and (raw := span.attributes.get('mlflow.chat.tokenUsage')):
        usage = json.loads(raw)
        usage = {key: value for key in TOKEN_FIELDS
                 if type(value := usage.get(key)) is int and value >= 0}
        attr = result.attributes.add(key='mlflow.chat.tokenUsage')
        for key, value in usage.items():
            attr.value.kvlist_value.values.add(key=key).value.int_value = value
    return result


def failure_tags(error):
    # Only fixed diagnostics may accompany the exception class. Provider errors
    # and validation errors can embed credentials, repository text or replies.
    safe_messages = {
        "done requires code changes", "could not inspect staged changes",
        "maker requires the worktree root", "missing final maker reply",
        "missing final model reply", "reply cites a memory it was not given",
        "only code may supply a forecast", "sized briefs must declare their root files",
        "duplicate JSON key", "non-finite JSON number",
        "ingest input exceeds exploration budget", "audit input exceeds exploration budget",
        "maker input exceeds exploration budget",
        "declared files must use canonical relative paths",
        "declared file exceeds the 1048576-byte limit",
        "declared file must be UTF-8 text without binary control bytes",
        "o200k_base tokenizer data must be preloaded in TIKTOKEN_CACHE_DIR",
        "fit limit must be smaller than context size",
    }
    message = "worker run failed"
    from jsonschema import ValidationError
    if isinstance(error, json.JSONDecodeError):
        message = f"invalid JSON at line {error.lineno}, column {error.colno}"
    elif isinstance(error, ValidationError):
        # Validator names and schema paths come from our schemas, never instance values.
        path = "/".join(str(part) for part in error.absolute_schema_path)
        message = f"schema validation failed: {error.validator} at schema {path}"
    elif isinstance(error, ValueError):
        message = "invalid worker data"
        if (type(error) is ValueError and len(error.args) == 1
                and isinstance(error.args[0], str) and error.args[0] in safe_messages):
            message = error.args[0]
    elif isinstance(error, OSError):
        message = "worker I/O failed"
    elif isinstance(error, subprocess.CalledProcessError):
        message = "worker subprocess failed"
    return {"failure_class": type(error).__name__[:64], "failure_message": message[:512]}


@contextmanager
def observe(request):
    capture = None
    if 'mlflow' not in request and (config := os.environ.get('TL3_MLFLOW_CONFIG')):
        try:
            request = {**request, 'mlflow': tomllib.loads(Path(config).read_text())['mlflow']}
        except Exception:
            failure()
    if request.get('mlflow'):
        try:
            os.environ['MLFLOW_DISABLE_TELEMETRY'] = 'true'
            logger = logging.getLogger('mlflow')
            logger.handlers = [logging.NullHandler()]
            logger.propagate = False
            capture = Capture(request)
        except Exception:
            failure()
    token = active.set(capture)
    outcome = 'failed'
    try:
        yield
        outcome = 'succeeded'
    except BaseException as error:
        if capture is not None:
            diagnostics = failure_tags(error)
            capture.tags.update(diagnostics)
            record_reply(diagnostics)
        raise
    finally:
        if capture is not None:
            try:
                capture.finish(outcome)
            except Exception:
                failure()
        active.reset(token)


def export():
    import signal
    import httpx

    def expired(*_):
        raise TimeoutError

    signal.signal(signal.SIGALRM, expired)
    signal.alarm(5)
    try:
        settings = json.loads(sys.stdin.buffer.readline())
        packet = sys.stdin.buffer.read()
        with httpx.Client(base_url=settings['tracking_uri'], timeout=1, trust_env=False) as client:
            response = client.get('/api/2.0/mlflow/experiments/get-by-name',
                                  params={'experiment_name': settings['experiment']})
            if response.status_code == 404:
                response = client.post('/api/2.0/mlflow/experiments/create',
                                       json={'name': settings['experiment']})
                # Another run may have created this experiment concurrently.
                if response.status_code == 400:
                    response = client.get('/api/2.0/mlflow/experiments/get-by-name',
                                          params={'experiment_name': settings['experiment']})
            response.raise_for_status()
            data = response.json()
            experiment = data.get('experiment', data)['experiment_id']
            response = client.post('/api/2.0/mlflow/experiments/set-experiment-tag',
                                   json={'experiment_id': experiment, 'key': 'mlflow.experimentKind',
                                         'value': 'genai_development'})
            response.raise_for_status()
            response = client.post('/v1/traces', content=packet,
                                   headers={'Content-Type': 'application/x-protobuf',
                                            'x-mlflow-experiment-id': experiment})
            response.raise_for_status()
        return 0
    except Exception:
        failure()
        return 1
    finally:
        signal.alarm(0)


if __name__ == '__main__':
    raise SystemExit(export())
