"""Call the selected worker release's Rust renderer at the original call point."""
from contextvars import ContextVar
import os
from pathlib import Path

from tl3_workers.reply_check import ReplyCheckInfrastructureError, run_check


# Selection is lazy: no new read precedes the worker's original startup reads.
# A missing binary keeps Python rendering for the whole run, even during deploy.
selection = ContextVar('prompt_renderer', default=None)


def render(kind, document):
    state = selection.get()
    if state is None or 'binary' not in state:
        path = os.environ.get('TL3_RELEASE_BINARY')
        binary = path if path and Path(path).is_file() else None
        if state is not None:
            state['binary'] = binary
    else:
        binary = state['binary']
    if binary is None:
        return None
    result = run_check(['render-prompt', kind], document, name='prompt', binary=binary)
    if not isinstance(result, dict):
        raise ReplyCheckInfrastructureError('prompt validator returned invalid output')
    if kind in {'role', 'generation', 'serialize'}:
        valid = set(result) == {'text'} and isinstance(result['text'], str)
    elif kind == 'cache':
        if set(result) == {'encoding_error'} and isinstance(result['encoding_error'], str):
            result['encoding_error'].encode('utf-8')
        valid = set(result) == {'identity'} and isinstance(result['identity'], str)
    else:
        prefix = result.get('prefix')
        valid = set(result) == {'prefix'} and (prefix is None or (
            isinstance(prefix, dict) and prefix.get('type') == 'text'
            and isinstance(prefix.get('text'), str)
            and set(prefix) in ({'type', 'text'}, {'type', 'text', 'cache_control'})
            and ('cache_control' not in prefix or prefix['cache_control'] == {'type': 'ephemeral'})))
    if not valid:
        raise ReplyCheckInfrastructureError('prompt validator returned invalid output')
    return result
