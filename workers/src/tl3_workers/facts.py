"""Durable worker completion facts, independent of trace export."""
from contextlib import contextmanager
from contextvars import ContextVar
import json
import os
from pathlib import Path
import re
from threading import Lock
import time
from uuid import uuid4

from langchain_core.callbacks import BaseCallbackHandler
from langchain_core.tracers.context import register_configure_hook


current = ContextVar('tl3_worker_facts', default=None)
register_configure_hook(current, inheritable=True)


def record_prismabuild_fallback(runtime):
    """Record fixed capability evidence without retaining probe text or paths."""
    from tl3_workers.tracing import active

    diagnostic = {'phase': 'prismabuild_capability', 'cause': 'machine_record_unsupported',
                  'fallback_used': True, 'transport': 'stdout'}
    if capture := active.get():
        capture.tags['prismabuild_fallback'] = json.dumps(diagnostic, sort_keys=True)
    if capture := current.get():
        with capture.lock:
            evidence = {**diagnostic, 'runtime': runtime}
            if evidence not in capture.prismabuild:
                capture.prismabuild.append(evidence)
                capture.save('running')


def boundary_failure(error, request):
    """Attribute only the boundary we observed. Never retain exception text or bodies."""
    from openai import APIError, APIStatusError
    from tl3_workers.tracing import Redactor

    for _ in range(4):
        if isinstance(error, APIError):
            break
        cause = error.__cause__ or error.__context__
        if cause is None:
            break
        error = cause
    origin = 'worker'
    status, request_id = None, None
    if isinstance(error, APIError):
        # This client talks to CLIProxyAPI. Its HTTP failure cannot prove which
        # upstream provider failed behind it.
        origin = 'proxy'
        if isinstance(error, APIStatusError):
            status = error.status_code
            request_id = error.response.headers.get('x-request-id')
            from urllib.parse import urlsplit
            if error.response.request.url.host != urlsplit(request['model']['base_url']).hostname:
                origin = 'provider'
    elif isinstance(error, (ConnectionError, TimeoutError)):
        origin = 'unknown'
    redactor = Redactor(None)
    from tl3_workers.tracing import model_key
    if key := model_key.get():
        redactor.secrets.add(key)
    if not (isinstance(request_id, str) and re.fullmatch(r'[A-Za-z0-9_.:-]{1,128}', request_id)
            and redactor.text(request_id) == request_id):
        request_id = None
    # Classes are library identifiers. Unrecognized classes become a fixed label.
    from tl3_workers.reply_check import failure_class
    name = failure_class(error)
    if (not re.fullmatch(r'[A-Za-z][A-Za-z0-9_]{0,63}', name)
            or redactor.text(name) != name):
        name = 'WorkerError'
    return {'origin': origin, 'class': name, 'status': status, 'request_id': request_id}


def publish(path, row):
    temporary = path.with_name(path.name + '.' + uuid4().hex + '.tmp')
    try:
        with temporary.open('x') as output:
            json.dump(row, output, allow_nan=False)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
        directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        temporary.unlink(missing_ok=True)


class Facts(BaseCallbackHandler):
    """Count completed model calls and the last usage of an interrupted stream once."""
    run_inline = True

    def __init__(self, request):
        self.request = request
        self.started_ms = None
        self.trace_id = None
        self.session_id = None
        self.lock = Lock()
        self.pending = {}
        self.prismabuild = []
        self.calls = self.ended = 0
        self.missing_usage = False
        self.totals = {key: None for key in ('input_tokens', 'cached_tokens', 'output_tokens')}
        # The maker's retro (#648), with its own usage outside the totals.
        self.retro = None

    def row(self, outcome, finished_ms=None, failure=None):
        row = {
            'started_ms': self.started_ms, 'finished_ms': finished_ms, 'outcome': outcome,
            'release': self.request.get('trace', {}).get('release'),
            'model': self.request['model']['name'], **self.totals,
            'usage_complete': self.calls > 0 and self.calls == self.ended and not self.missing_usage,
            'trace_id': self.trace_id, 'session_id': self.session_id, 'failure': failure,
            **({'prismabuild': list(self.prismabuild)} if self.prismabuild else {}),
        }
        if self.retro is not None:
            row['retro'] = self.retro
        return row

    def save(self, outcome, finished_ms=None, failure=None):
        if path := self.request.get('trace', {}).get('completion'):
            publish(Path(path), self.row(outcome, finished_ms, failure))

    def on_chat_model_start(self, serialized, messages, *, run_id, **kwargs):
        with self.lock:
            self.calls += 1
            self.save('running')

    def on_llm_new_token(self, token, *, run_id, chunk=None, **kwargs):
        if chunk is not None and (usage := getattr(chunk.message, 'usage_metadata', None)):
            with self.lock:
                self.pending[run_id] = usage

    def add(self, usage):
        self.ended += 1
        self.missing_usage |= not all(type(usage.get(key)) is int and usage[key] >= 0
                                      for key in ('input_tokens', 'output_tokens'))
        values = {key: usage.get(key) for key in ('input_tokens', 'output_tokens')}
        values['cached_tokens'] = usage.get('input_token_details', {}).get('cache_read')
        for key, value in values.items():
            if type(value) is int and value >= 0:
                self.totals[key] = (self.totals[key] or 0) + value
        self.save('running')

    def on_llm_end(self, response, *, run_id, **kwargs):
        with self.lock:
            last = self.pending.pop(run_id, None)
            message = response.generations[0][0].message if response.generations else None
            usage = getattr(message, 'usage_metadata', None) or last
            self.add(usage or {})

    def on_llm_error(self, error, *, run_id, **kwargs):
        with self.lock:
            self.missing_usage = True
            self.add(self.pending.pop(run_id, None) or {})


@contextmanager
def observe(request):
    capture = Facts(request)
    token = current.set(capture)
    capture.started_ms = time.time_ns() // 1_000_000
    outcome, failure = 'succeeded', None
    try:
        capture.save('running')
        yield capture
    except BaseException as error:
        from tl3_workers.quota import ending_ms
        outcome = ('quota' if ending_ms(error, time.time(), request.get('role')) is not None
                   else 'failed')
        failure = boundary_failure(error, request)
        raise
    finally:
        finished = time.time_ns() // 1_000_000
        current.reset(token)
        capture.save(outcome, finished, failure)
