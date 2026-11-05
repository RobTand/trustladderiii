"""Differential generator journals, changing reads, and crashes at every write."""
import base64
from contextlib import ExitStack
from copy import deepcopy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
from unittest.mock import patch

from tl3_harness import generator
from tl3_workers import pb_tests as pb_evidence, pb_generators as new
from tl3_workers.tools import Worktree


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


old = load('pb_generator_before', sys.argv[1])
old_pb = load('pb_tests_before', sys.argv[2])
old._failure_receipt = old_pb._failure_receipt


def checker(command, document, **kwargs):
    assert 'PRIVATE_NOT_ON_WIRE' not in json.dumps(document)
    print(json.dumps({'request': {'operation': command[1], **document}}), flush=True)
    return json.loads(sys.stdin.readline())


pb_evidence.run_check = checker
KEY = 'a' * 64
IMAGE = 'sha256:' + 'b' * 64
payload = b'# generated\n'
entry = {'sha256': hashlib.sha256(payload).hexdigest(), 'bytes': len(payload), 'mode': 0o644}
budget = generator.budget()
identity = {'implementation': generator.IMPLEMENTATION, 'image': IMAGE, 'budget': budget}
contract = {'command': 'python3 generate.py', 'generated_files': ['**/BUILD.bazel'],
            'protected_paths': [], 'files': ['tests/BUILD.bazel'], 'timeout': 120, 'cpus': 1, 'mem_gb': 2}


class Crash(BaseException):
    pass


class Context:
    def __init__(self, path):
        self.path = str(path)

    def __enter__(self):
        return self.path

    def __exit__(self, *args):
        pass


def execute(module, root, waits, mutation, crash_at):
    checkout = root / 'work'
    checkout.mkdir()
    journal = root / 'journal.json'
    artifact = {'status': 'completed', 'exit_code': 0, 'message': 'Generator completed.',
                'command': contract['command'], 'argv': ['python3', 'generate.py'],
                'identity': identity, 'budget': budget, 'phase': 'completed', 'timings': {},
                'input_manifest': generator.digest({}), 'output_manifest': generator.digest({'tests/BUILD.bazel': entry}),
                'outputs': [{'path': 'tests/BUILD.bazel', 'entry': entry,
                             'data': base64.b64encode(payload).decode()}]}
    if mutation == 'identity':
        artifact['identity'] = {**identity, 'image': 'wrong'}
    if mutation == 'input':
        artifact['input_manifest'] = '0' * 64
    if mutation == 'output':
        artifact['outputs'][0]['entry'] = {**entry, 'sha256': '0' * 64}
    raw = (generator.PREFIX + json.dumps(artifact) + '\n').encode()
    log = root / 'log'
    log.write_bytes(raw)
    ending = {'action_key': KEY, 'status': 'executed', 'exit_code': 0, 'succeeded': True,
              'attempt': 2, 'publication': {'generation': 'f' * 64},
              'diagnostics': {'argv': ['PRIVATE_NOT_ON_WIRE']},
              'result': {'verified': True, 'receipt_sha256': 'd' * 64,
                         'stdout': {'path': str(log), 'bytes': len(raw), 'sha256': hashlib.sha256(raw).hexdigest()}}}
    state = {'phase': 'submitted', 'key': KEY, 'before': {}, 'input_tree': '1' * 40,
             'contract': {**contract, 'image': IMAGE, 'scratch': new.scratch_environment(),
                          'implementation': generator.IMPLEMENTATION, 'budget': budget}}
    journal.write_text(json.dumps(state, sort_keys=True))
    events = []
    writes = 0
    crashed = False
    polls = 0
    manifests = 0
    real_manifest = generator.manifest
    real_open = os.open
    real_fdopen = os.fdopen
    real_fchmod = os.fchmod
    real_replace = os.replace
    real_unlink = os.unlink
    descriptors = {}

    def name(path):
        return str(path).replace(str(root), '$ROOT')

    def changed(kind, *values):
        nonlocal writes, crashed
        if crashed:
            raise Crash()
        events.append((kind, *values))
        writes += 1
        if writes == crash_at:
            crashed = True
            raise Crash()

    def manifest(path):
        nonlocal manifests
        manifests += 1
        events.append(('manifest', manifests))
        result = real_manifest(path)
        if mutation == 'source-race' and manifests == 2:
            result['source.py'] = entry
        return result

    def command(args):
        nonlocal polls
        polls += 1
        events.append(('wait', polls, args[2:]))
        if polls <= waits:
            return 124, {'schema': 'prismabuild.endings.v1', 'complete': False,
                         'endings': [{'action_key': KEY, 'status': ('waiting', 'executed', 'cache_hit')[(polls - 1) % 3]}]}
        return 0, {'schema': 'prismabuild.endings.v1', 'complete': True, 'endings': [ending]}

    def save(path, value):
        if crashed:
            raise Crash()
        path.write_text(json.dumps(value, sort_keys=True))
        changed('journal', deepcopy(value))

    def opened(path, *args, **kwargs):
        if crashed:
            raise Crash()
        descriptor = real_open(path, *args, **kwargs)
        descriptors[descriptor] = name(path)
        if str(path) == '.tl3-generator-import':
            changed('create', str(path))
        return descriptor

    class Stream:
        def __init__(self, stream):
            self.stream = stream

        def __getattr__(self, key):
            return getattr(self.stream, key)

        def __enter__(self):
            self.stream.__enter__()
            return self

        def __exit__(self, *args):
            return self.stream.__exit__(*args)

        def write(self, raw):
            if crashed:
                raise Crash()
            result = self.stream.write(raw)
            changed('write', descriptors.get(self.stream.fileno()), raw.hex())
            return result

        def flush(self):
            if crashed:
                raise Crash()
            self.stream.flush()
            changed('flush', descriptors.get(self.stream.fileno()))

    def fdopen(fd, mode='r', *args, **kwargs):
        return Stream(real_fdopen(fd, mode, *args, **kwargs))

    def fchmod(fd, mode):
        if crashed:
            raise Crash()
        real_fchmod(fd, mode)
        changed('chmod', descriptors.get(fd), mode)

    def fsync(fd):
        if crashed:
            raise Crash()
        # The recorded barrier models durability; both shells use this I/O interface.
        changed('fsync', descriptors.get(fd))

    def replace(src, dst, *args, **kwargs):
        if crashed:
            raise Crash()
        real_replace(src, dst, *args, **kwargs)
        changed('replace', name(src), name(dst))

    def unlink(path, *args, **kwargs):
        if crashed:
            raise Crash()
        return real_unlink(path, *args, **kwargs)

    with Worktree(checkout) as tree:
        tree.test_checkout = checkout
        tree.test_settings = module.TestSettings(IMAGE, root / 'tools')
        tree.generator = contract['command']
        tree.generated_files = contract['generated_files']
        tree.protected_paths = []
        tree.generator_files = contract['files']
        tree.generator_state = journal
        tree.progress_emit = lambda value: events.append(('emit', value))
        with ExitStack() as stack:
            stack.enter_context(patch.object(module, '_json_command', command))
            stack.enter_context(patch.object(module, '_stage', lambda *a, **k: '2' * 40))
            stack.enter_context(patch.object(module, 'save', save))
            stack.enter_context(patch.object(generator, 'manifest', manifest))
            stack.enter_context(patch.object(module.tempfile, 'TemporaryDirectory', lambda **k: Context(root)))
            for key, value in [('open', opened), ('fdopen', fdopen), ('fchmod', fchmod),
                               ('fsync', fsync), ('replace', replace), ('unlink', unlink)]:
                stack.enter_context(patch.object(os, key, value))
            try:
                result = module.make_generator_tool(tree)()
            except Crash:
                result = 'crash'
    files = {path.relative_to(checkout).as_posix(): path.read_bytes().hex()
             for path in sorted(checkout.rglob('*')) if path.is_file()}
    observed = (events, journal.read_bytes().hex(), files, result)
    # Local absolute paths are not part of the contract.
    return json.loads(json.dumps(observed).replace(str(root), '$ROOT')), writes


count = 0
with tempfile.TemporaryDirectory() as temporary:
    parent = Path(temporary)
    for waits in range(5):
        for mutation in ('valid', 'identity', 'input', 'output', 'source-race'):
            before_root = parent / f'old-{count}'
            after_root = parent / f'new-{count}'
            before_root.mkdir()
            after_root.mkdir()
            before, writes = execute(old, before_root, waits, mutation, 0)
            after, after_writes = execute(new, after_root, waits, mutation, 0)
            assert before == after and writes == after_writes, (waits, mutation, before, after)
            count += 1
            for crash in range(1, writes + 1):
                before_root = parent / f'old-{count}'
                after_root = parent / f'new-{count}'
                before_root.mkdir()
                after_root.mkdir()
                before, _ = execute(old, before_root, waits, mutation, crash)
                after, _ = execute(new, after_root, waits, mutation, crash)
                assert before == after, (waits, mutation, crash, before, after)
                count += 1
print(json.dumps({'done': count}), flush=True)
