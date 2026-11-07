"""Observational MLflow capture with bounded, redacted content and detached export."""
import base64
from contextlib import contextmanager
from contextvars import ContextVar
import json
import logging
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import tomllib


active = ContextVar('tl3_trace_capture', default=None)


from tl3_harness.action_policy import Redactor, size, sensitive_path, file_marker, trace_policy


# The run's loaded model key, for redaction outside tracing too (#322).
model_key = ContextVar('tl3_model_key', default=None)


def record_model_key(key):
    model_key.set(key)
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
            if isinstance(reply, dict):
                if cited := reply.get('memory_ids'):
                    capture.tags['memory_cited'] = ','.join(cited)
                if flagged := [flag['id'] for flag in reply.get('memory_flags', [])]:
                    capture.tags['memory_flagged'] = ','.join(flagged)
            capture.root.set_outputs(reply)
        except Exception:
            failure()


def failure():
    print('tl3_workers: MLflow export failed', file=sys.stderr)


class Content:
    __slots__ = ('key', 'value', 'size', 'limit')

    def __init__(self, key, value):
        self.key, self.value, self.size, self.limit = key, value, size(value), 0


class Entry:
    """One ended span: metadata now, and content shaped for redaction at the end."""
    __slots__ = ('proto', 'name', 'kind', 'contents', 'shrunk')

    def __init__(self, proto, name, kind, contents):
        self.proto, self.name, self.kind, self.contents, self.shrunk = proto, name, kind, contents, False


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
        self.entries = []
        self.shrinking = False
        self.omitted = 0
        self.lock = threading.Lock()
        capture = self
        logger = logging.getLogger('mlflow')
        logger.handlers = [logging.NullHandler()]
        logger.propagate = False

        class Exporter(SpanExporter):
            def export(self, spans):
                with capture.lock:
                    for span in spans:
                        try:
                            entry = span_entry(span, capture.redactor, capture.role, capture.model)
                            try:
                                capture.live_record(entry)
                            except Exception:
                                failure()
                            capture.add(entry)
                        except Exception:
                            # Children of a lost span are attached to the root at export.
                            failure()
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
        self.tags.update({key: str(trace[key]) for key in ('base_sha', 'request_origin')
                          if key in trace})
        self.tags.update(node=str(request['operation']['node']),
                         generation=str(request['operation']['generation']),
                         role=request['role'], model=request['model']['name'],
                         effort=request['model']['effort'] or 'default',
                         prompt_version=str(request.get('prompt', {}).get('version', 'packaged')))
        # #422: recalled and cited memory ids let judges score whether memories helped.
        if recalled := [memory['id'] for memory in request['inputs'].get('memories', [])]:
            self.tags['memory_recalled'] = ','.join(recalled)
        self.settings = request['mlflow']
        # #430: live spans are redacted when they end, by their own redactor so
        # the final trace's truncation flag is unchanged. Secrets are shared.
        self.live = Redactor(None)
        self.live.secrets, self.live.calls = self.redactor.secrets, self.redactor.calls
        self.live_bytes = self.live_spans = 0
        self.log = None
        try:
            self.log = SpanLog(trace.get('log'), trace.get('wake'))
            self.log.append(self.header())
            from opentelemetry.proto.trace.v1.trace_pb2 import Span
            # A trace appears once one of its spans is stored, so the run shows
            # before its first model turn ends.
            context, start = self.root._span.context, self.root._span.start_time
            started = Span(trace_id=context.trace_id.to_bytes(16, 'big'), span_id=os.urandom(8),
                           parent_span_id=context.span_id.to_bytes(8, 'big'), name='tl3.run_started',
                           start_time_unix_nano=start, end_time_unix_nano=start)
            started.status.code = 1
            started.attributes.add(key='mlflow.spanType').value.string_value = 'CHAIN'
            self.log.append({'kind': 'span', 'otlp': encode(started)})
            self.log.start_shipper()
        except Exception:
            failure()

    def header(self):
        """The span log's first record: what a flush needs to close the trace."""
        context = self.root._span.context
        return {'kind': 'start', 'v': 1, 'trace_id': context.trace_id.to_bytes(16, 'big').hex(),
                'root_span_id': context.span_id.to_bytes(8, 'big').hex(), 'role': self.role,
                'start_ns': self.root._span.start_time,
                'settings': {key: self.settings.get(key) for key in ('tracking_uri', 'experiment')},
                'tags': self.live_tags()}

    def live_tags(self):
        return {key: self.live.text(value)[:512] for key, value in self.tags.items()}

    def live_record(self, entry):
        """Append an ended span to the span log, redacted now within the live budget."""
        from opentelemetry.proto.trace.v1.trace_pb2 import Span
        from mlflow.tracing.utils.otlp import _set_otel_proto_anyvalue

        decision = self.live.trace({'op': 'live', 'enabled': self.log is not None,
            'root': not bool(entry.proto.parent_span_id), 'spans': self.live_spans,
            'used': self.live_bytes, 'contents': [content.value.wire for content in entry.contents]})
        if decision['values'] is None:
            return
        proto = Span()
        proto.CopyFrom(entry.proto)
        proto.name = self.live.text(entry.name)[:256]
        for content, value in zip(entry.contents, decision['values'], strict=True):
            _set_otel_proto_anyvalue(proto.attributes.add(key=content.key).value, value)
        self.live_bytes, self.live_spans = decision['used'], decision['spans']
        self.log.append({'kind': 'span', 'otlp': encode(proto)})

    def add(self, entry):
        self.entries.append(entry)
        while True:
            decision = trace_policy({'op': 'retain', 'entries': [entry_facts(item) for item in self.entries],
                                     'shrinking': self.shrinking})
            self.omitted += len(self.entries) - len(decision['keep'])
            self.entries = [self.entries[index] for index in decision['keep']]
            self.shrinking = decision['shrinking']
            if decision['shrink'] is None:
                return
            # The decision indexes the request's entries, before retention.
            older = next(item for index, item in zip(decision['keep'], self.entries, strict=True)
                         if index == decision['shrink'])
            for content in older.contents:
                content.value = self.redactor.reshape(content.value, decision['floor_bytes'])
                content.size = size(content.value)
            older.shrunk = True

    def finish(self, outcome):
        try:
            self.close(outcome)
        finally:
            if self.log is not None:
                self.log.finish()

    def close(self, outcome):
        if self.log is None:
            # Live setup failed; an unlinked log still hands the final to a shipper.
            self.log = SpanLog(None)
            self.log.append(self.header())
        self.context.__exit__(None, None, None)
        from mlflow.tracing.trace_manager import InMemoryTraceManager
        from mlflow.tracing.utils.otlp import _set_otel_proto_anyvalue
        import mlflow.langchain

        InMemoryTraceManager.get_instance().pop_trace(self.root._span.context.trace_id)
        mlflow.langchain.autolog(disable=True, silent=True)
        with self.lock:
            entries = list(self.entries)
        root = next(entry for entry in entries if not entry.proto.parent_span_id)
        kept = {entry.proto.span_id for entry in entries}
        for entry in entries:
            if entry.proto.parent_span_id and entry.proto.parent_span_id not in kept:
                entry.proto.parent_span_id = root.proto.span_id
            entry.proto.name = self.redactor.text(entry.name)[:256]
        usage = {'input_tokens': 0, 'output_tokens': 0, 'total_tokens': 0}
        retro = retro_spans(entries)
        for entry in entries:
            if entry.proto.span_id in retro:
                # The maker's retro has its own retro_* tags (#648).
                continue
            for attr in entry.proto.attributes:
                if attr.key == 'mlflow.chat.tokenUsage':
                    for item in attr.value.kvlist_value.values:
                        usage[item.key] = usage.get(item.key, 0) + item.value.int_value
        self.tags.update(outcome=outcome, **{key: str(value) for key, value in usage.items()})
        root.proto.status.code = 1 if outcome == 'succeeded' else 2
        # The outcome is known before the final packet is built, so a failure
        # below still closes the trace with it, never as a crash.
        with self.lock:
            self.log.append({'kind': 'end', 'status': root.proto.status.code, 'tags': self.live_tags()})
        for key, value in self.tags.items():
            root.proto.attributes.add(key='mlflow.traceTag.' + key).value.string_value = \
                self.redactor.text(value)[:512]
        if self.omitted:
            root.proto.attributes.add(key='tl3.trace.omitted_spans').value.int_value = self.omitted
        truncated = self.allocate(root, entries)
        for entry in entries:
            decision = self.redactor.trace({'op': 'clean',
                'contents': [content.value.wire for content in entry.contents],
                'limits': [content.limit for content in entry.contents]})
            for content, value in zip(entry.contents, decision['values'], strict=True):
                _set_otel_proto_anyvalue(entry.proto.attributes.add(key=content.key).value, value)
        if truncated or self.redactor.truncated or self.omitted:
            root.proto.attributes.add(key='tl3.trace.truncated').value.bool_value = True
        from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import ExportTraceServiceRequest
        packet = ExportTraceServiceRequest()
        resource = packet.resource_spans.add()
        add_tags(resource, self.live_tags())
        spans = resource.scope_spans.add().spans
        spans.extend(entry.proto for entry in entries)
        decision = trace_policy({'op': 'packet', 'bytes': packet.ByteSize(), 'entries': [
            {'root': entry is root, 'content_bytes': sum(attr.ByteSize() for attr in entry.proto.attributes
                if attr.key in {'mlflow.spanInputs', 'mlflow.spanOutputs'})} for entry in entries]})
        for index in decision['remove']:
            for attr in list(spans[index].attributes):
                if attr.key in {'mlflow.spanInputs', 'mlflow.spanOutputs'}:
                    spans[index].attributes.remove(attr)
        if decision['root_only']:
            failure()
            del resource.scope_spans[:]
            only = resource.scope_spans.add().spans.add()
            only.CopyFrom(root.proto)
            for attr in list(only.attributes):
                if attr.key in {'mlflow.spanInputs', 'mlflow.spanOutputs'}:
                    only.attributes.remove(attr)
        if not trace_policy({'op': 'fits', 'bytes': packet.ByteSize(), 'next': None})['fits']:
            raise RuntimeError('trace packet exceeds publication budget')
        with self.lock:
            self.log.append({'kind': 'final', 'otlp': encode(packet)})

    def allocate(self, root, entries):
        decision = trace_policy({'op': 'allocate', 'entries': [entry_facts(entry) for entry in entries]})
        for entry, limits in zip(entries, decision['limits'], strict=True):
            for content, limit in zip(entry.contents, limits, strict=True):
                content.limit = limit
        return decision['truncated']


def entry_facts(entry):
    return {'root': not bool(entry.proto.parent_span_id), 'kind': entry.kind,
            'proto_bytes': entry.proto.ByteSize(), 'sizes': [content.size for content in entry.contents],
            'shrunk': entry.shrunk}


def retro_spans(entries):
    """The span ids of the maker's retro span and its descendants."""
    from tl3_workers.retro import SPAN

    parents = {entry.proto.span_id: entry.proto.parent_span_id for entry in entries}
    marked = {entry.proto.span_id for entry in entries if entry.name == SPAN}

    def inside(span_id):
        seen = set()
        while span_id and span_id not in seen:
            if span_id in marked:
                return True
            seen.add(span_id)
            span_id = parents.get(span_id)
        return False

    return {span_id for span_id in parents if inside(span_id)}


def span_entry(span, redactor, role, model):
    from opentelemetry.proto.trace.v1.trace_pb2 import Span
    from mlflow.tracing.utils.otlp import _set_otel_proto_anyvalue

    kind = json.loads(span.attributes.get('mlflow.spanType', '"CHAIN"'))
    if kind not in {'LLM', 'CHAT_MODEL', 'TOOL', 'AGENT', 'CHAIN', 'RETRIEVER'}:
        kind = 'CHAIN'
    result = Span(trace_id=span.context.trace_id.to_bytes(16, 'big'),
                  span_id=span.context.span_id.to_bytes(8, 'big'),
                  parent_span_id=span.parent.span_id.to_bytes(8, 'big') if span.parent else b'',
                  start_time_unix_nano=span.start_time, end_time_unix_nano=span.end_time)
    result.status.code = 2 if span.status.status_code.name == 'ERROR' else 1
    result.attributes.add(key='mlflow.spanType').value.string_value = kind
    result.attributes.add(key='tl3.role').value.string_value = redactor.text(role)[:256]
    if kind in {'LLM', 'CHAT_MODEL'}:
        result.attributes.add(key='mlflow.llm.model').value.string_value = redactor.text(model)[:256]
    # Retain only content and usage. Headers, provider metadata, resources and
    # raw exceptions stay local. Content is bounded now and redacted at export.
    if (raw := span.attributes.get('tl3.proxy.usage_chunks')) is not None:
        # Numeric-only usage has its own boundary; names such as
        # prompt_tokens must survive the general sensitive-key filter.
        value = [chunk_counts(chunk) for chunk in json.loads(raw)[:32]]
        _set_otel_proto_anyvalue(result.attributes.add(key='tl3.proxy.usage_chunks').value, value)
    for key in ('tl3.judge.turn_count', 'tl3.judge.iteration_limit'):
        if (raw := span.attributes.get(key)) is not None:
            value = json.loads(raw)
            if type(value) is int and value >= 0:
                result.attributes.add(key=key).value.int_value = value
    # A grade's repair outcome and failure class (#728): known values only.
    from tl3_workers.judges import FailureClass
    for key, known in (('tl3.judge.repair', {'accepted', 'failed'}),
                       ('tl3.judge.failure_class', {item.value for item in FailureClass})):
        if (raw := span.attributes.get(key)) is not None and isinstance(
                value := json.loads(raw), str) and value in known:
            result.attributes.add(key=key).value.string_value = value
    if span.attributes.get('tl3.proxy.usage_chunks_truncated') is not None:
        result.attributes.add(key='tl3.proxy.usage_chunks_truncated').value.bool_value = True
    if kind in {'LLM', 'CHAT_MODEL'} and (raw := span.attributes.get('mlflow.chat.tokenUsage')):
        usage = json.loads(raw)
        usage = {key: value for key in TOKEN_FIELDS
                 if type(value := usage.get(key)) is int and value >= 0}
        attr = result.attributes.add(key='mlflow.chat.tokenUsage')
        for key, value in usage.items():
            attr.value.kvlist_value.values.add(key=key).value.int_value = value
    contents, inputs = [], None
    for key in ('mlflow.spanInputs', 'mlflow.spanOutputs'):
        if (raw := span.attributes.get(key)) is None:
            continue
        value = json.loads(raw)
        if key == 'mlflow.spanInputs':
            inputs = value
        elif kind == 'TOOL' and (path := sensitive_path(inputs)):
            value = file_marker(path)
        contents.append(Content(key, redactor.shape(value)))
    name = f'tl3_{role}:{model}' if kind in {'LLM', 'CHAT_MODEL'} else span.name
    return Entry(result, name, kind, contents)


def safe_span(span, redactor=None, role="unknown", model="unknown", payload_limit=None):
    from mlflow.tracing.utils.otlp import _set_otel_proto_anyvalue

    redactor = redactor or Redactor(None)
    entry = span_entry(span, redactor, role, model)
    entry.proto.name = redactor.text(entry.name)[:256]
    decision = redactor.trace({'op': 'clean', 'contents': [content.value.wire for content in entry.contents],
                               'limits': [payload_limit for content in entry.contents]})
    for content, value in zip(entry.contents, decision['values'], strict=True):
        _set_otel_proto_anyvalue(entry.proto.attributes.add(key=content.key).value, value)
    return entry.proto


class WorkerStageError(ValueError):
    """A fixed stage name, with optional schema diagnostics and no provider text."""

    STAGES = frozenset({"grade judge failure", "postmortem judge failure",
                        "postmortem repair failure", "postmortem JSON parse failure",
                        "postmortem recommendation contract failure",
                        "postmortem cached reply failure", "postmortem cached judged failure",
                        "postmortem assessment cache failure",
                        "postmortem assessment lookup failure",
                        "postmortem assessment write failure",
                        "postmortem assessment outcome unknown"})

    def __init__(self, stage, error=None, *, judge_failure=None):
        self.stage = stage if stage in self.STAGES else "worker run failed"
        self.validation = error
        self.judge_failure = judge_failure
        super().__init__(self.stage)


class InputSchemaFailure(Exception):
    """A controller-owned diagnostic, with no rejected instance values."""

    def __init__(self, diagnostic):
        self.message = ('worker inputs break role schema: '
                        f"role={diagnostic['role']} pointer={diagnostic['pointer']} rule={diagnostic['rule']}")
        super().__init__(self.message)


def input_failure(request):
    """Export a controller-rejected input's safe diagnostic before a worker starts."""
    safe = {key: request[key] for key in ('protocol', 'operation', 'role', 'model', 'trace') if key in request}
    safe['inputs'] = {key: request['inputs'][key] for key in ('role', 'pointer', 'rule')}
    if 'mlflow' in request:
        safe['mlflow'] = request['mlflow']
    try:
        with observe(safe):
            raise InputSchemaFailure(safe['inputs'])
    except InputSchemaFailure:
        return 0


def failure_tags(error):
    from tl3_workers.reply_check import MakerInputViolation, failure_class
    if isinstance(error, MakerInputViolation):
        return {"failure_class": failure_class(error), "failure_message": "invalid worker data"}
    if type(error) is InputSchemaFailure:
        return {"failure_class": "worker_input_schema", "failure_message": error.message}
    from tl3_workers.prerequisites import MergePrerequisiteError
    from tl3_workers.generated_scope import GeneratedOutputProhibition
    from tl3_workers.roles.planner import PlanConsistencyError
    from tl3_workers.evidence import EvidenceContractError
    from tl3_workers.forcing_rehearsal import ForcingInfrastructureError
    from tl3_workers.tools import WorktreeUnavailable
    from tl3_workers.turns import InvalidReply, Unrepairable
    from tl3_workers.reply_check import ReplyCheckInfrastructureError, ReplyViolation

    if isinstance(error, (ReplyCheckInfrastructureError, ReplyViolation)):
        # Rust emits fixed repair text; infrastructure errors contain no child output or paths.
        return {"failure_class": type(error).__name__, "failure_message": str(error)}

    if type(error) is MergePrerequisiteError:
        return {"failure_class": "MergePrerequisiteError",
                "failure_message": MergePrerequisiteError.message}
    if type(error) is GeneratedOutputProhibition:
        return {"failure_class": "GeneratedOutputProhibition",
                "failure_message": GeneratedOutputProhibition.message}

    if isinstance(error, EvidenceContractError):
        return {"failure_class": "EvidenceContractError", "failure_message": str(error)}

    if isinstance(error, WorktreeUnavailable):
        # The message is a fixed tool name and error class (tools.WorktreeUnavailable).
        return {"failure_class": "WorktreeUnavailable", "failure_message": str(error)[:128]}

    if isinstance(error, ForcingInfrastructureError):
        return {"failure_class": "ForcingInfrastructureError", "failure_message": str(error)[:512]}

    if isinstance(error, PlanConsistencyError):
        # Node IDs are model text. Redact before retaining each draft's diagnostic.
        message = 'planner check inconsistency: ' + ', '.join(error.node_ids)
        return {"failure_class": "PlanConsistencyError",
                "failure_message": Redactor(None).text(message)[:220]}
    if isinstance(error, WorkerStageError):
        from jsonschema import ValidationError
        from tl3_workers.judges import FailureClass, JudgeFailure

        message = error.stage
        if (isinstance(error.judge_failure, JudgeFailure)
                and isinstance(error.judge_failure.category, FailureClass)):
            cause = error.judge_failure
            message += ": " + cause.category.value
            tags = {"failure_class": cause.category.value, "failure_stage": error.stage}
            if type(cause.status) is int and 400 <= cause.status <= 599:
                message += f" HTTP {cause.status}"
                tags["failure_http_status"] = str(cause.status)
            return {**tags, "failure_message": message}
        if isinstance(error.validation, ValidationError):
            message += ": " + failure_tags(error.validation)["failure_message"]
        return {"failure_class": "WorkerStageError", "failure_message": message[:512]}
    if isinstance(error, InvalidReply):
        first, second = (failure_tags(item)["failure_message"] for item in error.errors)
        return {"failure_class": "InvalidReply",
                "failure_message": f"invalid reply: {first}; repaired reply: {second}"[:512]}
    # Only fixed diagnostics may accompany the exception class. Provider errors
    # and validation errors can embed credentials, repository text or replies.
    safe_messages = {
        "done requires code changes", "could not inspect staged changes",
        "maker requires the worktree root", "missing final maker reply",
        "missing final model reply", "reply cites a memory it was not given",
        "reply flags a memory it did not cite", "a flag's claim is not in the memory it flags",
        "only code may supply a forecast", "sized briefs must declare their root files",
        "duplicate JSON key", "non-finite JSON number",
        "reply is not one JSON object", "reply has more than one JSON object",
        "ingest input exceeds exploration budget", "audit input exceeds exploration budget",
        "maker input exceeds exploration budget",
        "maker worktree footprint does not match its input; reconstruct the Worktree with inputs.footprint",
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
        if (type(error) in (ValueError, Unrepairable) and len(error.args) == 1
                and isinstance(error.args[0], str) and error.args[0] in safe_messages):
            message = error.args[0]
    elif isinstance(error, OSError):
        message = "worker I/O failed"
    elif isinstance(error, subprocess.CalledProcessError):
        message = "worker subprocess failed"
    return {"failure_class": type(error).__name__[:64], "failure_message": message[:512]}


def mlflow_settings(request):
    """The run's [mlflow] settings: from the request, else TL3_MLFLOW_CONFIG."""
    if 'mlflow' not in request and (config := os.environ.get('TL3_MLFLOW_CONFIG')):
        try:
            return tomllib.loads(Path(config).read_text())['mlflow']
        except Exception:
            failure()
            return None
    return request.get('mlflow')


@contextmanager
def observe(request):
    capture = None
    if 'mlflow' not in request and (settings := mlflow_settings(request)):
        request = {**request, 'mlflow': settings}
    if request.get('mlflow'):
        try:
            os.environ['MLFLOW_DISABLE_TELEMETRY'] = 'true'
            logger = logging.getLogger('mlflow')
            logger.handlers = [logging.NullHandler()]
            logger.propagate = False
            capture = Capture(request)
            from tl3_workers.facts import current as current_facts
            if facts := current_facts.get():
                facts.trace_id = capture.root.trace_id
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


def encode(proto):
    return base64.b64encode(proto.SerializeToString()).decode()


def add_tags(resource_spans, tags):
    # MLflow stores a batch's resource attributes as tags of its traces, root or
    # not, so a trace carries its tags while it is still in progress.
    for key, value in tags.items():
        resource_spans.resource.attributes.add(key=key).value.string_value = value


class SpanLog:
    """The durable copy of a run's trace (#430): JSON lines, each appended in one
    write and synced before the shipper is woken. Without a path from the
    controller, an unlinked file only bounds handoff to the shipper."""

    def __init__(self, path, wake=None):
        self.path, self.wake_path = path, wake
        if path:
            self.fd = os.open(path, os.O_RDWR | os.O_APPEND | os.O_CREAT | os.O_CLOEXEC, 0o600)
        else:
            self.file = tempfile.TemporaryFile()
            self.fd = self.file.fileno()
        self.wake = self.shipper = None

    def append(self, record):
        line = memoryview(json.dumps(record, separators=(',', ':')).encode() + b'\n')
        while line:
            line = line[os.write(self.fd, line):]
        os.fdatasync(self.fd)
        if self.wake is not None:
            try:
                os.write(self.wake, b'\n')
            except (BlockingIOError, BrokenPipeError):
                pass  # A full pipe already holds a wake-up; a dead shipper leaves the log.

    def start_shipper(self):
        # The shipper owns every network wait; the worker never waits for it.
        read, self.wake = os.pipe()
        os.set_blocking(self.wake, False)
        options = [*(['--log', self.path] if self.path else []),
                   *(['--wake', self.wake_path] if self.wake_path else [])]
        try:
            self.shipper = subprocess.Popen(
                [sys.executable, str(Path(__file__)), 'ship', str(self.fd), *options],
                stdin=read, stdout=subprocess.DEVNULL, stderr=sys.stderr, close_fds=True, pass_fds=(self.fd,))
        finally:
            os.close(read)

    def finish(self):
        if self.shipper is None or self.shipper.poll() is not None:
            # A shipper that never started or died reads the log from the start.
            if self.wake is not None:
                os.close(self.wake)
            self.start_shipper()
        os.close(self.wake)
        self.wake = None


def records(data):
    """Complete records in order, with the offset after each. A torn last line
    from a killed worker is not a record."""
    offset = 0
    while (end := data.find(b'\n', offset)) != -1:
        try:
            record = json.loads(data[offset:end])
        except ValueError:
            record = None
        offset = end + 1
        if isinstance(record, dict):
            yield record, offset


class Shipper:
    """Sends span log records in order. Records are decoded once, as they
    arrive; a failed send keeps them queued for the next wake."""

    def __init__(self, log=None, wake=None, fd=None):
        import httpx
        self.client = httpx.Client(timeout=1, trust_env=False)
        self.log, self.wake, self.fd = log, wake, fd
        self.header, self.experiment, self.failed, self.reported = None, None, False, False
        self.queue, self.end, self.final, self.sent_final = [], None, None, False
        self.buffer = b''

    def feed(self, data):
        """Decode the complete records in data, keeping a torn tail for later."""
        from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import ExportTraceServiceRequest
        from opentelemetry.proto.trace.v1.trace_pb2 import Span

        self.buffer += data
        consumed = 0
        for record, offset in records(self.buffer):
            consumed = offset
            kind = record.get('kind')
            if kind == 'start':
                self.header = record
            elif kind == 'span':
                self.queue.append(Span.FromString(base64.b64decode(record['otlp'])))
            elif kind == 'end':
                self.end = record
            elif kind == 'final':
                self.final = ExportTraceServiceRequest.FromString(base64.b64decode(record['otlp']))
        self.buffer = self.buffer[consumed:] if consumed else self.buffer

    def resolve(self):
        if self.experiment is not None:
            return self.experiment
        import httpx
        settings = self.header['settings']
        base = settings['tracking_uri'].rstrip('/')
        response = self.client.get(base + '/api/2.0/mlflow/experiments/get-by-name',
                                   params={'experiment_name': settings['experiment']})
        if response.status_code == 404:
            response = self.client.post(base + '/api/2.0/mlflow/experiments/create',
                                        json={'name': settings['experiment']})
            # Another run may have created this experiment concurrently.
            if response.status_code == 400:
                response = self.client.get(base + '/api/2.0/mlflow/experiments/get-by-name',
                                           params={'experiment_name': settings['experiment']})
        response.raise_for_status()
        data = response.json()
        experiment = data.get('experiment', data)['experiment_id']
        try:
            self.client.post(base + '/api/2.0/mlflow/experiments/set-experiment-tag',
                             json={'experiment_id': experiment, 'key': 'mlflow.experimentKind',
                                   'value': 'genai_development'})
        except httpx.HTTPError:
            pass  # Cosmetic UI tag must not discard a trace.
        self.experiment = experiment
        return experiment

    def post(self, packet):
        if not trace_policy({'op': 'fits', 'bytes': packet.ByteSize(), 'next': None})['fits']:
            raise RuntimeError('trace packet exceeds publication budget')
        response = self.client.post(self.header['settings']['tracking_uri'].rstrip('/') + '/v1/traces',
                                    content=packet.SerializeToString(),
                                    headers={'Content-Type': 'application/x-protobuf',
                                             'x-mlflow-experiment-id': self.resolve()})
        response.raise_for_status()

    def spans(self, protos):
        """Send spans in packets within the trace bound, tagged with the run's tags."""
        from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import ExportTraceServiceRequest

        def packet():
            packet = ExportTraceServiceRequest()
            resource = packet.resource_spans.add()
            add_tags(resource, self.header['tags'])
            return packet, resource.scope_spans.add().spans

        current, spans = packet()
        for proto in list(protos):
            if spans and not trace_policy({'op': 'fits', 'bytes': current.ByteSize(), 'next': proto.ByteSize()})['fits']:
                self.post(current)
                del protos[:len(spans)]
                current, spans = packet()
            spans.append(proto)
        if spans:
            self.post(current)
            del protos[:len(spans)]

    def send(self, closing=None):
        """Send queued spans, then the final packet, or else a closing root.
        Returns whether the trace is closed in MLflow."""
        if self.header is None:
            return False
        try:
            self.spans(self.queue)
            if self.final is not None and not self.sent_final:
                self.post(self.final)
                self.sent_final = True
            elif self.final is None and closing is not None:
                root = closing(self.header, self.end)
                self.spans([root])
                self.sent_final = True
        except Exception:
            if not self.reported:
                failure()
            self.failed = self.reported = True
            return False
        if self.failed and self.wake:
            # MLflow is back: held logs of ended runs can ship too. A write, not
            # a touch, so the controller's inotify watch sees it.
            try:
                os.close(os.open(self.wake, os.O_WRONLY | os.O_CREAT | os.O_CLOEXEC, 0o600))
            except OSError:
                pass
        self.failed = False
        return self.sent_final

    def shipped(self):
        """Mark the log shipped where it lives now. Retirement may have moved it
        while this process held it open; /proc names the open file's current path."""
        if not self.log:
            return
        log = self.log
        if self.fd is not None:
            try:
                current = os.readlink(f'/proc/self/fd/{self.fd}')
            except OSError:
                current = None
            if current is None or current.endswith(' (deleted)'):
                return  # Removed with its run; nothing is left to resend.
            log = current
        try:
            Path(log + '.shipped').touch()
        except OSError:
            pass


def options(args):
    found, args = {}, list(args)
    while len(args) >= 2 and args[0] in ('--log', '--wake'):
        found[args[0][2:]] = args[1]
        del args[:2]
    if args:
        raise ValueError('unexpected arguments')
    return found


def ship(fd, log=None, wake=None):
    """Send the span log as the worker appends it, waking once per record."""
    import signal

    def expired(*_):
        raise TimeoutError

    offset, closed = 0, False
    try:
        # The first successful send also wakes held flushes.
        shipper = Shipper(log, wake, fd)
        shipper.failed = True
        while True:
            ended = not os.read(0, 4096)
            if ended:
                # The worker has ended. Bound the last sends, as before #430.
                signal.signal(signal.SIGALRM, expired)
                signal.alarm(5)
            size = os.fstat(fd).st_size
            shipper.feed(os.pread(fd, size - offset, offset))
            offset = size
            closed = shipper.send()
            if ended:
                break
        if closed:
            shipper.shipped()
            return 0
    except Exception:
        failure()
    finally:
        signal.alarm(0)
    return 1


def identity():
    stat = Path(f'/proc/{os.getpid()}/stat').read_text().rsplit(')', 1)[1].split()
    return {'pid': os.getpid(), 'machine': Path('/etc/machine-id').read_text().strip(), 'start': int(stat[19]),
            'boot': Path('/proc/sys/kernel/random/boot_id').read_text().strip()}


def claim(path):
    """One flusher per attempt: a link publishes the claim atomically. The
    controller clears a claim whose process has died."""
    temporary = Path(f'{path}.{os.getpid()}.tmp')
    temporary.write_text(json.dumps(identity()))
    try:
        os.link(temporary, path)
        return True
    except FileExistsError:
        return False
    finally:
        temporary.unlink(missing_ok=True)


def flush(path):
    """Run by the controller when an attempt has ended: send the span log, and
    close the trace of a worker that never wrote its final packet."""
    import signal

    def expired(*_):
        raise TimeoutError

    claimed = Path(path + '.flushing')
    try:
        if not claim(claimed):
            return 0
    except OSError:
        claimed = None  # Retirement moved the attempt; send without a claim.
    signal.signal(signal.SIGALRM, expired)
    signal.alarm(10)
    try:
        shipper = Shipper(path, fd=0)
        shipper.feed(sys.stdin.buffer.read())
        if shipper.header is None:
            shipper.shipped()  # Nothing was traced, so there is nothing to retry.
            return 0
        logged = list(shipper.queue)
        if not shipper.send(lambda header, end: closing_root(header, end, logged, path)):
            return 1
        shipper.shipped()
        return 0
    except Exception:
        failure()
        return 1
    finally:
        signal.alarm(0)
        if claimed is not None:
            claimed.unlink(missing_ok=True)


def closing_root(header, end, spans, path):
    """The root span of a worker that ended before sending its final packet.
    Its end is the last logged evidence of work, never the time of this flush.
    A worker that recorded its outcome closes with it; otherwise it was stopped
    by the controller or crashed."""
    from opentelemetry.proto.trace.v1.trace_pb2 import Span

    start = header['start_ns']
    root = Span(trace_id=bytes.fromhex(header['trace_id']), span_id=bytes.fromhex(header['root_span_id']),
                name='tl3_' + header['role'], start_time_unix_nano=start,
                end_time_unix_nano=max([start, *(span.end_time_unix_nano for span in spans)]))
    root.attributes.add(key='mlflow.spanType').value.string_value = 'AGENT'
    root.attributes.add(key='tl3.role').value.string_value = header['role']
    if end is not None:
        root.status.code = 1 if end.get('status') == 1 else 2
        tags = end['tags']
    else:
        root.status.code = 2
        usage = {'input_tokens': 0, 'output_tokens': 0, 'total_tokens': 0}
        for span in spans:
            for attr in span.attributes:
                if attr.key == 'mlflow.chat.tokenUsage':
                    for item in attr.value.kvlist_value.values:
                        usage[item.key] = usage.get(item.key, 0) + item.value.int_value
        attempt = Path(path).parent
        if (attempt / 'stopped.json').exists():
            tags = {'outcome': 'stopped', 'failure_class': 'WorkerStopped',
                    'failure_message': 'stopped by the controller'}
        else:
            message = 'worker ended without an exit record'
            try:
                code = json.loads((attempt / 'exit.json').read_text())['code']
                if type(code) is int:
                    message = f'worker exited with code {code}'
            except (OSError, ValueError, KeyError, TypeError):
                pass
            tags = {'outcome': 'crashed', 'failure_class': 'WorkerCrashed', 'failure_message': message}
        tags = {**header['tags'], **tags, **{key: str(value) for key, value in usage.items()}}
    for key, value in tags.items():
        root.attributes.add(key='mlflow.traceTag.' + key).value.string_value = value
    return root


def main(args):
    if args == ['input-failure']:
        return input_failure(json.load(sys.stdin))
    if args[:1] == ['ship'] and len(args) >= 2:
        return ship(int(args[1]), **options(args[2:]))
    if args[:1] == ['flush'] and len(args) == 2:
        return flush(args[1])
    print('usage: tracing.py ship FD [--log LOG] [--wake WAKE] | flush LOG', file=sys.stderr)
    return 2


if __name__ == '__main__':
    raise SystemExit(main(sys.argv[1:]))
