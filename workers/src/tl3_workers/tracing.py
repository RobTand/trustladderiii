"""Observational MLflow capture. Only an allowlisted payload leaves the worker."""
from contextlib import contextmanager
import json
import logging
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib


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
                    capture.spans.append(safe_span(span))
                return SpanExportResult.SUCCESS

        # MLflow 3.17.2 has no public in-memory destination. Pin this adapter to
        # that release: the stock exporter starts network threads and joins at exit.
        tracer = TracerProvider(shutdown_on_exit=False)
        tracer.add_span_processor(MlflowV3SpanProcessor(Exporter(), export_metrics=False))
        provider._MLFLOW_TRACE_USER_DESTINATION.set(MlflowExperimentLocation(experiment_id='0'))
        provider.provider.once.do_once(lambda: provider.provider.set(tracer))
        provider.provider.set(tracer)
        mlflow.langchain.autolog(silent=True)
        self.context = mlflow.start_span(name='tl3_agent', span_type='AGENT')
        self.root = self.context.__enter__()
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
        if self.dropped:
            failure()
            return
        usage = {'input_tokens': 0, 'output_tokens': 0, 'total_tokens': 0}
        for span in self.spans:
            for attr in span.attributes:
                if attr.key == 'mlflow.chat.tokenUsage':
                    for key, value in json.loads(attr.value.string_value).items():
                        usage[key] += value
        self.tags.update(outcome=outcome, **{key: str(value) for key, value in usage.items()})
        root = next(span for span in self.spans if not span.parent_span_id)
        root.status.code = 1 if outcome == 'succeeded' else 2
        for key, value in self.tags.items():
            root.attributes.add(key='mlflow.traceTag.' + key).value.string_value = value
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


def safe_span(span):
    from opentelemetry.proto.trace.v1.trace_pb2 import Span

    kind = json.loads(span.attributes.get('mlflow.spanType', '"CHAIN"'))
    if kind == 'CHAT_MODEL':
        kind = 'LLM'
    if kind not in {'LLM', 'TOOL', 'AGENT', 'CHAIN', 'RETRIEVER'}:
        kind = 'CHAIN'
    result = Span(trace_id=span.context.trace_id.to_bytes(16, 'big'),
                  span_id=span.context.span_id.to_bytes(8, 'big'),
                  parent_span_id=span.parent.span_id.to_bytes(8, 'big') if span.parent else b'',
                  name='tl3_' + kind.lower(), start_time_unix_nano=span.start_time,
                  end_time_unix_nano=span.end_time)
    result.status.code = 2 if span.status.status_code.name == 'ERROR' else 1
    result.attributes.add(key='mlflow.spanType').value.string_value = json.dumps(kind)
    # Prompts, replies, tool arguments/results, events, headers, provider metadata,
    # resource attributes and exception messages are deliberately never serialized.
    if kind == 'LLM' and (raw := span.attributes.get('mlflow.chat.tokenUsage')):
        usage = json.loads(raw)
        usage = {key: value for key in ('input_tokens', 'output_tokens', 'total_tokens')
                 if type(value := usage.get(key)) is int and value >= 0}
        result.attributes.add(key='mlflow.chat.tokenUsage').value.string_value = json.dumps(usage)
    return result


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
    outcome = 'failed'
    try:
        yield
        outcome = 'succeeded'
    finally:
        if capture is not None:
            try:
                capture.finish(outcome)
            except Exception:
                failure()


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
