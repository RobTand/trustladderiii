"""Private subprocess transport to the Rust action; no policy or schema lives here."""
import atexit
import json
import os
from pathlib import Path
import shutil
import subprocess
import threading

_client = None
_lock = threading.Lock()


def _process():
    global _client
    if _client is None or _client[0] != os.getpid():
        binary = os.environ.get('TL3_ACTION_BINARY') or shutil.which('tl3-action')
        if not binary and (root := os.environ.get('RUNFILES_DIR') or os.environ.get('TEST_SRCDIR')):
            candidate = Path(root) / '_main/crates/action/tl3-action'
            if candidate.is_file():
                binary = str(candidate)
        if not binary:
            raise RuntimeError('Rust action executable is unavailable')
        _client = (os.getpid(), subprocess.Popen(
            [binary, 'policy'], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, text=True, encoding='utf-8',
            env={key: os.environ[key] for key in ('PATH', 'HOME', 'TMPDIR', 'LANG') if key in os.environ}))
    return _client[1]


def _close():
    if _client is not None and _client[0] == os.getpid():
        process = _client[1]
        process.stdin.close()
        try:
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


atexit.register(_close)


def _request(operation, state=None):
    with _lock:
        process = _process()
        try:
            process.stdin.write(json.dumps({'state': state or {'secrets': [], 'calls': {}, 'truncated': False},
                                            'operation': operation}) + '\n')
            process.stdin.flush()
            response = json.loads(process.stdout.readline())
        except (OSError, ValueError):
            raise RuntimeError('Rust action policy transport failed') from None
        if 'error' in response:
            raise RuntimeError(response['error'])
        return response['ok']


class _Shaped:
    """An opaque Rust value, retained between calls to Python APIs."""
    def __init__(self, wire):
        self.wire = wire


class Redactor:
    def __init__(self, key):
        self.secrets = {key} if key else set()
        self.calls = {}
        self.truncated = False

    def _call(self, op, **facts):
        response = _request({'op': op, **facts}, {'secrets': list(self.secrets), 'calls': self.calls,
                                               'truncated': self.truncated})
        self.calls.clear()
        self.calls.update(response['state']['calls'])
        self.truncated = response['state']['truncated']
        return response['result']['value']

    def text(self, value, limit=8192):
        return self._call('text', value=value, limit=limit)

    def window(self, value, start, end):
        return self._call('window', value=value, start=start, end=end)

    def shape(self, value, limit=65536):
        return _Shaped(self._call('shape', input_json=json.dumps(value), limit=limit))

    def reshape(self, value, limit):
        return _Shaped(self._call('reshape', shaped=value.wire, limit=limit))

    def clean(self, value, limit=65536):
        return json.loads(self._call('clean', shaped=value.wire, limit=limit))

    def payload(self, value, limit=65536):
        return json.loads(self._call('payload', input_json=json.dumps(value), limit=limit))


def size(value):
    operation = {'op': 'size', 'shaped': value.wire} if isinstance(value, _Shaped) else {
        'op': 'measure_json', 'input_json': json.dumps(value)}
    return _request(operation)['result']['value']


def sensitive_path(value):
    return _request({'op': 'sensitive_path', 'input_json': json.dumps(value)})['result']['value']


def file_marker(path):
    return _request({'op': 'file_marker', 'path': path})['result']['value']


def render_rows(rows, *, role, context='reply'):
    return _request({'op': 'render', 'rows': rows, 'role': role, 'context': context})['result']['value']


def validate_rows(rows, *, role):
    _request({'op': 'validate', 'rows': rows, 'role': role})


def permits_rows(rows, before, after, *, role):
    return _request({'op': 'permits', 'rows': rows, 'before': before, 'after': after,
                     'role': role})['result']['value']
