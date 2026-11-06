"""Test-only recording plugin. No source from the served repository is executed."""
import ast
import base64
from dataclasses import fields, is_dataclass
import functools
import hashlib
import json
import os
from pathlib import Path
import sys
import time

import pytest

_ROWS = []
_DEPTH = 0
_NODE = '<collection>'


def serial(value):
    if hasattr(value, 'as_json'):
        return value.as_json()
    if hasattr(value, 'value'):
        return value.value
    if is_dataclass(value):
        return {field.name: serial(getattr(value, field.name)) for field in fields(value)}
    if isinstance(value, dict):
        return {key: serial(item) for key, item in value.items()}
    if isinstance(value, (tuple, list)):
        return [serial(item) for item in value]
    if hasattr(value, '__dict__'):
        return {key: serial(item) for key, item in vars(value).items()}
    return value


def files(tree):
    return {path.relative_to(tree).as_posix(): {
        'b64': base64.b64encode(path.read_bytes()).decode(),
        'mode': path.stat().st_mode & 0o777,
    } for path in sorted(tree.rglob('*')) if path.is_file()}


def install():
    from tl3_harness import proof
    original = proof.assemble_proof
    if getattr(original, '_corpus', False):
        return

    @functools.wraps(original)
    def record(base, head, destination, check, config, **kwargs):
        global _DEPTH
        row = {'node': _NODE, 'input': {
            'base': files(base), 'head': files(head),
            'destination_exists': destination.exists(), 'check': serial(check),
            'config': serial(config), 'kwargs': serial(kwargs)}}
        started = time.perf_counter_ns()
        _DEPTH += 1
        try:
            receipt = original(base, head, destination, check, config, **kwargs)
            row['python_ns'] = time.perf_counter_ns() - started
            row['python'] = {'accepted': True, 'receipt': json.loads(json.dumps(receipt)),
                             'staged': files(destination), 'staged_sha256': proof.tree_sha256(destination)}
            return receipt
        except proof.ProofRefusal as error:
            row['python_ns'] = time.perf_counter_ns() - started
            row['python'] = {'accepted': False, 'rule': error.rule, 'module': error.module,
                             'symbol': error.symbol, 'violations': error.as_json()}
            raise
        except Exception as error:
            row['python'] = {'error': type(error).__name__, 'message': str(error)}
            raise
        finally:
            _DEPTH -= 1
            _ROWS.append(row)
    record._corpus = True
    # Patch aliases already imported by execution/staging and test modules.
    for module in tuple(sys.modules.values()):
        if module is None or not getattr(module, '__name__', '').startswith(('tl3_harness', 'test_')):
            continue
        for name, value in tuple(vars(module).items()):
            if value is original:
                setattr(module, name, record)
    proof.assemble_proof = record


def pytest_configure(config):
    install()


def pytest_collection_modifyitems(items):
    install()


@pytest.hookimpl(hookwrapper=True)
def pytest_runtest_protocol(item):
    global _NODE
    _NODE = item.nodeid
    yield


def pytest_sessionfinish(session, exitstatus):
    output = Path(os.environ['PROOF_CORPUS_OUT'])
    output.mkdir(parents=True, exist_ok=True)
    with (output / 'calls.jsonl').open('w') as stream:
        for index, row in enumerate(_ROWS):
            row['id'] = index
            stream.write(json.dumps(row, sort_keys=True) + '\n')
    # This independent parser probe records Python facts, never supplies Rust decisions.
    sources = {}
    for row in _ROWS:
        for side in ('base', 'head'):
            for name, data in row['input'][side].items():
                if name.endswith('.py'):
                    text = base64.b64decode(data['b64']).decode('utf-8', errors='replace')
                    sources.setdefault(text, name)
    with (output / 'sources.jsonl').open('w') as stream:
        for source, name in sources.items():
            row = {'source': source, 'name': name, 'sha256': hashlib.sha256(source.encode()).hexdigest()}
            try:
                tree = ast.parse(source, filename=name)
                row['unparse'] = ast.unparse(tree)
                row['dump'] = ast.dump(tree)
            except SyntaxError as error:
                row['error'] = error.msg
            stream.write(json.dumps(row, sort_keys=True) + '\n')
    summary = {'calls': len(_ROWS), 'sources': len(sources), 'pytest_exit': exitstatus,
               'python': sys.version, 'accepted': sum(row['python'].get('accepted', False) for row in _ROWS),
               'refused': sum(row['python'].get('accepted') is False for row in _ROWS)}
    (output / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    print('PROOF_CORPUS_SUMMARY ' + json.dumps(summary), flush=True)
