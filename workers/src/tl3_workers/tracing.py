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
TRACE_BYTES = 1024 * 1024
STRING_CHARS = 8192
ITEM_BYTES = 16384
TOKEN_FIELDS = ('input_tokens', 'output_tokens', 'total_tokens',
                'cache_read_input_tokens', 'cache_creation_input_tokens')
SENSITIVE_KEY = re.compile(
    r'(?i)(?:secret|token|password|passwd|api[_-]?key|authorization|credential|private[_-]?key|'
    r'model[_-]?key|key[_-]?file|cookie)')
# JSON inside JSON puts a literal backslash before each quote and whitespace escape.
SPACE = r'(?:\s|\\[nrt])*'
QUOTED = r"""(?:\\+"[^"\r\n]*"|"(?:[^"\\\r\n]|\\.)*"|\\*'[^'\r\n]*')"""
ASSIGN = r"""(?:\\*["'])?""" + SPACE + r'[:=]' + SPACE
SECRET_PATTERNS = [
    re.compile(r'-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY(?: BLOCK)?-----.*?'
               r'(?:-----END (?:[A-Z0-9]+ )*PRIVATE KEY(?: BLOCK)?-----|$)', re.DOTALL),
    re.compile(r'(?i)\b(?:Bearer|Basic)\s+[A-Za-z0-9._~+/=-]+'),
    # These headers carry a scheme and parameters, so an unquoted value runs to the
    # end of its line.
    re.compile(r'(?i)\b(?:(?:proxy-)?authorization|(?:set-)?cookie)\b' + ASSIGN +
               '(?:' + QUOTED + r'|(?:(?!\\[nr])[^\r\n])*)'),
    re.compile(r'\beyJ[A-Za-z0-9_-]*\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\b'),
    re.compile(r'\b(?:sk-[A-Za-z0-9_-]+|(?:sk|rk)_(?:live|test)_[A-Za-z0-9]+|'
               r'gh[pousr]_[A-Za-z0-9_]+|github_pat_[A-Za-z0-9_]+|glpat-[A-Za-z0-9_-]+|'
               r'(?:AKIA|ASIA)[A-Z0-9]{16}|xox[baprs]-[A-Za-z0-9-]+|xapp-[A-Za-z0-9-]+|'
               r'pypi-[A-Za-z0-9_-]+|npm_[A-Za-z0-9]{20,}|hf_[A-Za-z0-9]{20,}|'
               r'AIza[A-Za-z0-9_-]{20,}|SG\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+)\b'),
    re.compile(r'(?i)\b(?:hooks\.slack\.com/services|discord(?:app)?\.com/api/webhooks)/[^\s"\'\\]+'),
    re.compile(r'(?i)\b[a-z][a-z0-9+.-]*://[^/\s:@]*:[^@\s/]+@'),
    re.compile(r'(?i)(?<![\w-])--[\w-]*(?:password|passwd|secret|token|key|credential)[\w-]*[= \t]+\S+'),
    re.compile(r'(?<![\w-])-u[ \t]+[^\s:]+:\S+'),
    # Both identifier affixes are bounded so that a long run of word characters
    # cannot make the scan quadratic.
    re.compile(r'(?i)[\w-]{0,128}(?:secret|token|password|passwd|key|credential|signature)[\w-]{0,128}'
               + ASSIGN + '(?:' + QUOTED + r"""|[^\s"'\\,;}&]+)"""),
]


class Redactor:
    def __init__(self, key_path):
        # The runtime supplies an already-loaded key. Tracing never opens key files.
        self.secrets = {key_path} if key_path else set()

    def text(self, value):
        value = self.tail(value, STRING_CHARS)
        for secret in sorted(self.secrets, key=len, reverse=True):
            value = value.replace(secret, '[REDACTED]')
        for pattern in SECRET_PATTERNS:
            value = pattern.sub('[REDACTED]', value)
        return value

    def tail(self, value, limit):
        if len(value) <= limit:
            return value
        # Discard the entire line crossing the cut, including any credential
        # prefix. A single oversized line is omitted rather than leaking a suffix.
        cut = value.find('\n', len(value) - limit)
        if cut == -1:
            return '[truncated: oversized line omitted]'
        cut += 1
        # PEM bodies and the already-loaded model key can span multiple lines.
        # These literal scans avoid running regexes over the discarded content.
        begin = value.rfind('-----BEGIN ', 0, cut)
        end = value.rfind('-----END ', 0, cut)
        if begin > end and 'PRIVATE KEY-----' in value[begin:begin + 100]:
            end = value.find('-----END ', cut)
            cut = value.find('\n', end) + 1 if end != -1 else len(value)
            if not cut:
                cut = len(value)
        for secret in self.secrets:
            if not secret:
                continue
            start = value.find(secret, max(0, cut - len(secret)), cut + len(secret))
            if start != -1 and start < cut < start + len(secret):
                cut = start + len(secret)
        return '[truncated: earlier text omitted]\n' + value[cut:]

    def clean(self, value, limit=PAYLOAD_BYTES, depth=0):
        if limit < 256 or depth > 20:
            return '[truncated: content budget]'
        if isinstance(value, str):
            if value[:128].lstrip().startswith(('{', '[', '"')):
                # Model replies and tool arguments often contain JSON on one
                # physical line. Bound oversized fields without discarding the
                # reply, and judge any JSON by its field names, however deeply
                # quoted. Unchanged short JSON keeps its original layout.
                try:
                    decoded = json.loads(value)
                except (ValueError, RecursionError):
                    pass
                else:
                    cleaned = self.clean(decoded, limit // 2, depth + 1)
                    if len(value) > limit // 6 - 64 or cleaned != decoded:
                        return json.dumps(cleaned, ensure_ascii=False)
            # Six bytes covers JSON's worst-case escaping of a character. Each
            # message/argument/result is bounded before pattern matching.
            return self.text(self.tail(value, min(STRING_CHARS, limit // 6 - 64)))
        if isinstance(value, dict):
            result = {}
            remaining = limit - 256
            for key in reversed(value):
                if key in {'response_metadata', 'additional_kwargs', 'headers'}:
                    continue
                if remaining < 512:
                    result['tl3.truncated'] = 'earlier fields omitted'
                    break
                key_text = str(key)
                safe_key = self.text(self.tail(key_text, 128))
                item = ('[REDACTED]' if len(key_text) > 128 or SENSITIVE_KEY.search(key_text) else
                        self.clean(value[key], min(ITEM_BYTES, remaining - 256), depth + 1))
                remaining -= len(json.dumps({safe_key: item}, ensure_ascii=False).encode()) + 2
                result[safe_key] = item
            return dict(reversed(list(result.items())))
        if isinstance(value, list):
            result = []
            remaining = limit - 256
            for item in reversed(value):
                if remaining < 512:
                    result.append({'truncated': True, 'note': 'earlier items omitted'})
                    break
                item = self.clean(item, min(ITEM_BYTES, remaining), depth + 1)
                remaining -= len(json.dumps(item, ensure_ascii=False).encode()) + 2
                result.append(item)
            return list(reversed(result))
        return value

    def payload(self, value, limit=PAYLOAD_BYTES):
        return self.clean(value, min(PAYLOAD_BYTES, limit))


def record_model_key(key):
    if capture := active.get():
        capture.redactor.secrets.add(key)


def chunk_counts(usage):
    if not isinstance(usage, dict):
        return {}
    counts = {key: value for key in ('prompt_tokens', 'completion_tokens', 'total_tokens',
                                     'cache_write_tokens')
              if type(value := usage.get(key)) is int and value >= 0}
    for key, fields in [('prompt_tokens_details', ('cached_tokens', 'cache_write_tokens')),
                        ('completion_tokens_details', ('reasoning_tokens',))]:
        if isinstance(details := usage.get(key), dict):
            counts[key] = {field: value for field in fields
                           if type(value := details.get(field)) is int and value >= 0}
    return counts


def record_chunk_usage(chunk):
    try:
        if active.get() is None or not chunk.get('usage'):
            return
        import mlflow
        span = mlflow.get_current_active_span()
        if span is None or span.span_type not in {'LLM', 'CHAT_MODEL'}:
            return
        counts = chunk_counts(chunk['usage'])
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
        # Reserve space for root content, tags and protobuf framing. Child
        # content stops consuming redaction work once this shared budget is full.
        self.remaining = TRACE_BYTES - PAYLOAD_BYTES
        self.content_remaining = TRACE_BYTES // 2
        self.truncated = False
        self.omitted = 0
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
                    reserve = 2 * PAYLOAD_BYTES if span.parent else 0
                    available = max(0, capture.remaining - reserve)
                    limit = max(0, (available - 2048) // 4)
                    if span.parent:
                        limit = min(limit, capture.content_remaining // 4)
                    safe = safe_span(span, capture.redactor, capture.role, capture.model,
                                     payload_limit=limit)
                    if safe.ByteSize() > available:
                        capture.truncated = True
                        capture.omitted += 1
                        continue
                    capture.remaining -= safe.ByteSize()
                    if span.parent:
                        content = sum(attr.ByteSize() for attr in safe.attributes
                                      if attr.key in {'mlflow.spanInputs', 'mlflow.spanOutputs'})
                        capture.content_remaining = max(0, capture.content_remaining - content)
                    capture.truncated |= limit < PAYLOAD_BYTES
                    capture.spans.append(safe)
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
        if self.truncated:
            root.attributes.add(key='tl3.trace.truncated').value.bool_value = True
        if self.omitted:
            root.attributes.add(key='tl3.trace.omitted_spans').value.int_value = self.omitted
        for key, value in self.tags.items():
            root.attributes.add(key='mlflow.traceTag.' + key).value.string_value = self.redactor.text(value)[:512]
        from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import ExportTraceServiceRequest
        packet = ExportTraceServiceRequest()
        packet.resource_spans.add().scope_spans.add().spans.extend(self.spans)
        if packet.ByteSize() > TRACE_BYTES:
            failure()
            return
        # An unlinked file bounds handoff and avoids pipe backpressure. The export
        # child owns network waits; the worker never waits for it, even at shutdown.
        with tempfile.TemporaryFile() as payload:
            payload.write(json.dumps(self.settings).encode() + b'\n')
            payload.write(packet.SerializeToString())
            payload.seek(0)
            subprocess.Popen([sys.executable, str(Path(__file__))], stdin=payload,
                             stdout=subprocess.DEVNULL, stderr=sys.stderr, close_fds=True)


def safe_span(span, redactor=None, role="unknown", model="unknown", payload_limit=PAYLOAD_BYTES):
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
    # raw exceptions stay local. Bound work before redaction and redact before
    # export. Budget-exhausted spans keep metadata and numeric token counts.
    for key in ('mlflow.spanInputs', 'mlflow.spanOutputs',
                'tl3.proxy.usage_chunks', 'tl3.proxy.usage_chunks_truncated'):
        if (raw := span.attributes.get(key)) is not None:
            if key == 'tl3.proxy.usage_chunks':
                # Numeric-only usage has its own boundary; names such as
                # prompt_tokens must survive the general sensitive-key filter.
                value = [chunk_counts(chunk) for chunk in json.loads(raw)[:32]]
            else:
                value = (redactor.payload(json.loads(raw), payload_limit) if payload_limit >= 512
                         else {'truncated': True, 'note': 'trace content budget exhausted'})
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
            try:
                client.post('/api/2.0/mlflow/experiments/set-experiment-tag',
                            json={'experiment_id': experiment, 'key': 'mlflow.experimentKind',
                                  'value': 'genai_development'})
            except httpx.HTTPError:
                pass  # Cosmetic UI tag must not discard a trace.
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
