"""Record the existing Python PB contract; run this oracle on BuildBuddy only.

No served code or real PB records are used. Only external reads/submissions are
replaced; decisions and serialization execute in their production functions.
"""
import argparse
import ast
import base64
from copy import deepcopy
import hashlib
import inspect
import json
import os
import re
from pathlib import Path
import tempfile
import textwrap
from types import SimpleNamespace
from unittest.mock import patch

from tl3_harness import generator
from tl3_harness.model import Forcing
from tl3_harness.selector_identity import selector_identity
from tl3_harness.supervision import ExecutionFailure
from tl3_harness.test_tool import OUTCOMES, PREFIX, Summary
from tl3_workers import forcing_rehearsal as rehearsal
from tl3_workers import pb_generators, pb_tests, tools
from tl3_workers.tools import Evidence, Worktree

DIRECTORY = Path(__file__).with_name('pb-evidence-vectors')
KINDS = ('ending-summary', 'failure-receipt', 'post-wait-consistency',
         'machine-record-fallback', 'generator-artifact', 'rehearsal-identity', 'evidence-ledger')
KEY = 'a' * 64
IMAGE = 'sha256:' + 'b' * 64
NODE = 'tests/test_old.py::test_old'
BASE = 'c' * 40
FORCING = {'symptom': 'Old node fails', 'environment': {},
           'patches': [{'target': 'subject.adopt', 'returns': False}]}
SYMPTOM = {'node': NODE, 'exception': 'ValueError', 'message': 'broken',
           'phase': 'call', 'repeat': 2, 'parallel': 0}


def encoded(value):
    return json.dumps(value, ensure_ascii=False, indent=2) + '\n'


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def completed():
    summary = Summary()
    summary.add(NODE, 'passed')
    return summary.result


def receipt():
    return {'action_key': KEY, 'status': 'executed', 'exit_code': 0,
            'receipt_sha256': 'd' * 64, 'log_sha256': 'e' * 64}


def ending(raw):
    return {'action_key': KEY, 'status': 'executed', 'exit_code': 0, 'succeeded': True,
            'attempt': 2, 'publication': {'generation': 'f' * 64},
            'result': {'verified': True, 'receipt_sha256': 'd' * 64,
                       'stdout': {'path': '$LOG', 'bytes': len(raw), 'sha256': digest(raw)}}}


def log_input(value, prefix=PREFIX):
    raw = (prefix + json.dumps(value, ensure_ascii=False) + '\n').encode()
    return {'ending': ending(raw), 'log': raw.decode()}


def assign(value, path, replacement):
    parts = path.split('.')
    for part in parts[:-1]:
        value = value[int(part)] if isinstance(value, list) else value[part]
    if isinstance(value, list):
        value[int(parts[-1])] = replacement
    else:
        value[parts[-1]] = replacement


def variants(original, mutations):
    for name, path, value in mutations:
        item = deepcopy(original)
        assign(item, path, value)
        yield name, item


def cases(kind):
    if kind == 'ending-summary':
        good = log_input(completed())
        yield 'executed', good
        for name, value in variants(good, [
            ('cache-hit', 'ending.status', 'cache_hit'),
            ('key', 'ending.action_key', '0' * 64), ('status', 'ending.status', 'waiting'),
            ('exit', 'ending.exit_code', 1), ('succeeded', 'ending.succeeded', False),
            ('verified', 'ending.result.verified', False),
            ('receipt-sha', 'ending.result.receipt_sha256', 'Z' * 64),
            ('receipt-type', 'ending.result.receipt_sha256', None),
            ('log-size-negative', 'ending.result.stdout.bytes', -1),
            ('log-size-bool', 'ending.result.stdout.bytes', True),
            ('log-size-limit', 'ending.result.stdout.bytes', 1_048_577),
            ('log-size-changed', 'ending.result.stdout.bytes', 1),
            ('log-sha', 'ending.result.stdout.sha256', '0' * 64),
            ('attempt-bool-ignored', 'ending.attempt', True),
            ('generation-invalid-ignored', 'ending.publication.generation', 'bad'),
        ]):
            yield name, value
        for name, raw in [('no-summary', 'noise\n'), ('two-summaries', good['log'] * 2),
                          ('json', PREFIX + '{\n'), ('missing-log', None)]:
            item = deepcopy(good)
            item['log'] = raw
            if raw is not None:
                item['ending'] = ending(raw.encode())
            yield name, item
        for name, value in variants(completed(), [
            ('shape-status', 'status', 'passed'), ('shape-tests', 'tests', {}),
            ('shape-note', 'note', None), ('shape-totals', 'totals', []),
            ('totals-keys', 'totals', {'passed': 1}),
            ('totals-negative', 'totals.failed', -1), ('totals-bool', 'totals.failed', True),
            ('totals-float', 'totals.failed', 0.0), ('test-shape', 'tests.0', None),
            ('test-name', 'tests.0.test', 1), ('test-outcome', 'tests.0.outcome', 'unknown'),
            ('totals-not-equal-node-count-accepted', 'totals.passed', 99),
        ]):
            yield name, log_input(value)
        # These permissive Python comparisons are part of the oracle, not fixes.
        yield 'exit-bool-accepted', {**good, 'ending': {**good['ending'], 'exit_code': False}}
        yield 'succeeded-truthy-accepted', {**good, 'ending': {**good['ending'], 'succeeded': 1}}
        run = {'run': 1, 'records': [{'test': NODE, 'outcome': 'symptom'}]}
        run['sha256'] = digest(json.dumps(run['records'], sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode())
        summary = {**completed(), 'declared_symptom': SYMPTOM,
                   'symptom_runs': [{k: v for k, v in run.items() if k != 'records'}]}
        for name, records in [('symptom-valid', [run]), ('symptom-missing', []),
                              ('symptom-run-index', [{**run, 'run': 2}]),
                              ('symptom-run-sha', [{**run, 'sha256': '0' * 64}]),
                              ('symptom-run-summary', [{**run, 'extra': True}])]:
            raw = ''.join('TL3_SYMPTOM_RUN ' + json.dumps(r) + '\n' for r in records)
            raw += PREFIX + json.dumps(summary) + '\n'
            yield name, {'ending': ending(raw.encode()), 'log': raw}
    elif kind == 'failure-receipt':
        good = {'action_key': KEY, 'status': 'failed', 'exit_code': None,
                'reason': 'worker_lost', 'attempt': 1,
                'publication': {'generation': 'f' * 64, 'published_unix': 42},
                'diagnostics': {'returncode': 137, 'resource_profile': {'scope': {'memory_peak_bytes': 0}}}}
        yield 'missing-record', {'ending': good}
        record = {'action_key': KEY, 'published_unix': 42,
                  'detail': {'termination_reason': 'memory_limit_oom', 'private_metadata': 'must-not-copy'}}
        yield 'published-record', {'ending': good, 'record': record}
        for name, r in variants(record, [('wrong-key', 'action_key', '0' * 64),
                                         ('wrong-publication', 'published_unix', 43)]):
            yield name, {'ending': good, 'record': r}
        yield 'invalid-record-json', {'ending': good, 'record_raw': '{'}
        yield 'record-over-limit', {'ending': good, 'record_padding': 2_097_153}
        for name, e in variants(good, [
            ('diagnostic-oom', 'diagnostics.termination_reason', 'memory_limit_oom'),
            ('exit-precedence', 'exit_code', 3), ('reason-unsafe', 'reason', 'private/path'),
            ('reason-empty', 'reason', ''), ('reason-long', 'reason', 'x' * 129),
            ('attempt-bool', 'attempt', True), ('attempt-zero', 'attempt', 0),
            ('generation-case', 'publication.generation', 'F' * 64),
            ('peak-bool', 'diagnostics.resource_profile.scope.memory_peak_bytes', True),
            ('peak-negative', 'diagnostics.resource_profile.scope.memory_peak_bytes', -1),
            ('peak-positive', 'diagnostics.resource_profile.scope.memory_peak_bytes', 123456),
        ]):
            yield name, {'ending': e}
    elif kind == 'post-wait-consistency':
        good = {'operation': 'wait', 'result': {**completed(), 'proof': {'mode': 'head-observer'},
                'forcing': Forcing.parse(FORCING).as_json(), 'selectors': [NODE], 'declared_symptom': SYMPTOM},
                'proof': {'mode': 'head-observer'}, 'forcing': FORCING,
                'declared_symptom': SYMPTOM, 'base': True}
        yield 'consistent', good
        for name, item in variants(good, [
            ('proof-mismatch', 'result.proof', {}), ('forcing-mismatch', 'result.forcing', {}),
            ('selection-mismatch', 'result.selectors', ['other.py']),
            ('symptom-mismatch', 'result.declared_symptom', {}),
            ('infrastructure-skips-proof-forcing', 'result.status', 'infrastructure_error'),
        ]):
            yield name, item
        compact = deepcopy(good)
        compact['result']['selectors'] = selector_identity([NODE])
        yield 'compact-selection', compact
        for name, value in [('empty', []), ('deduplicated', [NODE, NODE]),
                            ('unicode', ['tests/test_é.py::test_雪']), ('not-list', NODE),
                            ('too-many', ['t.py'] * 257), ('blank', ['']), ('absolute', ['/x.py']),
                            ('parent', ['a/../x.py']), ('git', ['.git/a.py']), ('option', ['-x']),
                            ('nul', ['x\0.py']), ('backslash', ['a\\b.py']), ('newline', ['x\n.py']),
                            ('carriage-return', ['x\r.py']), ('too-long', ['x' * 2049]),
                            ('node-without-file', ['::test_x'])]:
            yield 'selectors-' + name, {'operation': 'selectors', 'value': value}
        for value in (None, -10, 0, -11, 1, True, 0.0, '0'):
            yield 'priority-' + repr(value), {'operation': 'priority', 'value': value}
        settings = {'image': IMAGE, 'tools_path': '/fixture/tools'}
        yield 'settings-default', {'operation': 'settings', 'value': settings}
        for name, item in variants(settings, [('mutable-image', 'image', 'ubuntu:latest'),
                                             ('relative-tools', 'tools_path', 'relative'),
                                             ('cpus-zero', 'cpus', 0), ('mem-bool', 'mem_gb', True),
                                             ('quiet-float', 'quiet_seconds', 1.0)]):
            yield 'settings-' + name, {'operation': 'settings', 'value': item}
    elif kind == 'machine-record-fallback':
        capability = {'schema': 'prismabuild.machine_record_capability.v1', 'machine_record_fd': 1}
        good = {'capability': json.dumps(capability) + '\n', 'capability_rc': 0,
                'private': '{"source":"private"}\n', 'stdout': '{"source":"stdout"}\n', 'rc': 0}
        yield 'supported-private-wins', good
        for name, value in [('absent', ''), ('wrong-schema', '{"schema":"wrong","machine_record_fd":1}\n'),
                            ('bool', json.dumps({**capability, 'machine_record_fd': True}) + '\n'),
                            ('float', json.dumps({**capability, 'machine_record_fd': 1.0}) + '\n'),
                            ('zero', json.dumps({**capability, 'machine_record_fd': 0}) + '\n'),
                            ('extra-field', json.dumps({**capability, 'extra': 1}) + '\n'),
                            ('no-newline', json.dumps(capability)), ('two-lines', '{}\n{}\n'),
                            ('oversized', ' ' * 4097)]:
            yield name, {**good, 'capability': value}
        yield 'exit-2-fallback', {**good, 'capability_rc': 2}
        for name, value in [('empty', ''), ('unterminated', '{}'), ('not-object', '[]\n'),
                            ('invalid-json', '{\n'), ('multiple', '{}\n{}\n')]:
            yield 'private-' + name, {**good, 'private': value}
        yield 'private-oversized', {**good, 'private_padding': 2_097_153}
        yield 'private-killed', {**good, 'rc': -9}
        yield 'submission-refused', {**good, 'private': '', 'rc': 2, 'detach': True}
        for name, value in [('invalid-json', '{'), ('not-object', '[]'), ('no-newline-accepted', '{}')]:
            yield 'stdout-' + name, {**good, 'capability_rc': 2, 'stdout': value}
        yield 'stdout-killed', {**good, 'capability_rc': 2, 'rc': -9}
        yield 'stdout-refused', {**good, 'capability_rc': 2, 'stdout': '', 'rc': 2, 'detach': True}
        yield 'stdout-oversized', {**good, 'capability_rc': 2, 'stdout_padding': 2_097_153}
        yield 'probe-io-fallback', {**good, 'probe_io_error': True}
        yield 'private-io', {**good, 'action_io_error': True}
        yield 'stdout-io', {**good, 'capability_rc': 2, 'action_io_error': True}
    elif kind == 'generator-artifact':
        contract = {'command': 'python3 generate.py', 'generated_files': ['**/BUILD.bazel'],
                    'protected_paths': [], 'files': ['tests/BUILD.bazel']}
        raw = b'# generated\n'
        entry = {'sha256': digest(raw), 'bytes': len(raw), 'mode': 0o644}
        selected = generator.budget(1, 2, generator.TIMEOUT)
        artifact = {'status': 'completed', 'exit_code': 0, 'message': 'Generator completed.',
                    'command': contract['command'], 'argv': ['python3', 'generate.py'],
                    'identity': {'implementation': generator.IMPLEMENTATION, 'image': IMAGE, 'budget': selected},
                    'budget': selected, 'phase': 'completed', 'timings': {}, 'input_manifest': generator.digest({}),
                    'output_manifest': generator.digest({'tests/BUILD.bazel': entry}),
                    'outputs': [{'path': 'tests/BUILD.bazel', 'entry': entry, 'data': base64.b64encode(raw).decode()}]}
        good = {**log_input(artifact, generator.PREFIX), 'contract': contract, 'before': {}, 'artifact': artifact}
        yield 'verified-import', good
        for name, item in variants(good, [
            ('key', 'ending.action_key', '0' * 64), ('status', 'ending.status', 'failed'),
            ('exit', 'ending.exit_code', 1), ('success-bool-required', 'ending.succeeded', 1),
            ('unverified', 'ending.result.verified', False), ('receipt-sha', 'ending.result.receipt_sha256', 'bad'),
            ('empty-log', 'ending.result.stdout.bytes', 0),
            ('oversized-log', 'ending.result.stdout.bytes', generator.ARTIFACT_BYTES + 1),
            ('log-size', 'ending.result.stdout.bytes', 1), ('log-sha', 'ending.result.stdout.sha256', '0' * 64),
        ]):
            yield name, item
        for name, log in [('no-artifact', 'noise\n'), ('two-artifacts', good['log'] * 2)]:
            yield name, {**good, 'log': log, 'ending': ending(log.encode())}
        for name, a in variants(artifact, [
            ('stale-input', 'input_manifest', '0' * 64), ('argv', 'argv', ['python3', 'other.py']),
            ('count', 'outputs', {}), ('digest', 'outputs.0.entry.sha256', '0' * 64),
            ('mode', 'outputs.0.entry.mode', 0o777), ('output-manifest', 'output_manifest', '0' * 64),
            ('path-traversal', 'outputs.0.path', '../BUILD.bazel'),
            ('reserved-path', 'outputs.0.path', '.tl3-generator-import'),
            ('undeclared', 'outputs.0.path', 'source.py'),
            ('identity', 'identity.image', 'sha256:' + '0' * 64), ('budget', 'budget.cpus', 2),
            ('command', 'command', 'python3 other.py'),
        ]):
            yield name, {**good, **log_input(a, generator.PREFIX), 'artifact': a}
        duplicate = {**artifact, 'outputs': artifact['outputs'] * 2}
        yield 'duplicate-output', {**good, **log_input(duplicate, generator.PREFIX), 'artifact': duplicate}
        yield 'protected-output', {**good, 'contract': {**contract, 'protected_paths': ['**/BUILD.bazel']}}
        yield 'unchanged-output', {**good, 'before': {'tests/BUILD.bazel': entry},
                                  **log_input({**artifact, 'input_manifest': generator.digest({'tests/BUILD.bazel': entry})}, generator.PREFIX)}
    elif kind == 'rehearsal-identity':
        forcing = {**FORCING, 'symptom': 'café 雪 😀', 'patches': [
            {'target': 'subject.one', 'returns': 1}, {'target': 'subject.two', 'returns': 1.0}]}
        good = {'base_sha': BASE, 'selectors': [NODE, 'tests/test_é.py::test_雪', NODE], 'forcing': forcing}
        yield 'unicode-int-float', {'candidate': good}
        yield 'key-order', {'candidate': {**good, 'forcing': dict(reversed(list(forcing.items())))}}
        yield 'selector-and-independent-patch-order', {'candidate': {
            **good, 'selectors': list(reversed(good['selectors'])),
            'forcing': {**forcing, 'patches': list(reversed(forcing['patches']))}}}
        for name, patches in [('alias-attributes', [{'target': 'a.same', 'returns': 0}, {'target': 'b.same', 'returns': 1}]),
                              ('parent-child', [{'target': 'a.parent', 'returns': None}, {'target': 'a.parent.child', 'returns': 1}]),
                              ('negative-zero', [{'target': 'a.value', 'returns': -0.0}]),
                              ('exponent', [{'target': 'a.value', 'returns': 1e-7}])]:
            yield name, {'candidate': {**good, 'forcing': {**forcing, 'patches': patches}}}
        yield 'declared-symptom', {'candidate': {**good, 'declared_symptom': SYMPTOM}}
        for name, result in [('valid', {**completed(), 'receipts': [receipt()]}),
                             ('missing-receipts', completed()), ('incomplete', {**completed(), 'receipts': [receipt()], 'totals': dict.fromkeys(OUTCOMES, 0)}),
                             ('missing-node', {**completed(), 'receipts': [receipt()], 'tests': [{'test': 'other.py', 'outcome': 'passed'}]}),
                             ('wrong-outcome', {**completed(), 'receipts': [receipt()], 'tests': [{'test': NODE, 'outcome': 'failed'}]})]:
            yield 'unusable-' + name, {'result': result, 'selectors': [NODE], 'outcome': 'passed'}
        wanted = {'base_sha': BASE, 'selectors': [NODE], 'forcing': FORCING}
        for name, receipts in [('matched', [wanted]), ('absent', []),
                               ('wrong-base', [{**wanted, 'base_sha': '0' * 40}])]:
            yield 'violations-' + name, {'candidate': wanted, 'receipts': receipts}
        yield 'violations-terminal-failure', {'candidate': wanted, 'receipts': [], 'failure': {'message': 'candidate budget exhausted'}}
    elif kind == 'evidence-ledger':
        success = {**completed(), 'tree': 'e' * 40, 'receipts': [receipt()]}
        infra = {'status': 'infrastructure_error', 'action_key': KEY}
        selectors = [False, [NODE]]
        def event(result, selected=selectors, **extra):
            return {'result': result, 'selectors': selected, **extra}
        yield 'infra-retirement-and-roundtrip', {'events': [event(infra),
            event({'status': 'infrastructure_error'}, tool_error=True), event(success)],
            'cites': [{'action_key': KEY}, {'tool_error': 'tool-error-1'}]}
        yield 'base-does-not-retire-head', {'events': [event(infra), event(success, [True, [NODE]])], 'cites': [{'action_key': KEY}]}
        yield 'test-run-origin-not-citable', {'events': [event({**infra, 'origin': 'test_run'})], 'cites': [{'action_key': KEY}]}
        yield 'completed-without-receipt-retires', {'events': [event(infra), event(completed())], 'cites': [{'action_key': KEY}]}
        yield 'cache-hit', {'events': [event({**success, 'receipts': [{**receipt(), 'status': 'cache_hit'}]})]}
        yield 'ten-entries-newest-kept', {'events': [event({**success, 'tree': f'{n:040x}'}) for n in range(12)]}
        yield 'byte-bound-and-selector-identity', {'events': [event(success, [False, [f'test_{i}_{n}_' + 'x' * 1900 for n in range(256)]]) for i in range(5)]}
        yield 'base-proof-and-repeat', {'events': [event({**success, 'proof': {'mode': 'head-observer'},
            'base_semantics': 'base_subject_head_observer', 'runs': [{'run': 1, 'status': 'completed', 'totals': success['totals']}]},
            [True, [NODE]], options={'repeat': 2, 'parallel': 2})]}
        yield 'non-dict-ignored', {'events': [event(None)]}
        yield 'spend-retires', {'events': [event(infra), event({'status': 'infrastructure_error'})], 'spend': True,
                               'cites': [{'action_key': KEY}, {'tool_error': 'tool-error-1'}]}
    else:
        raise AssertionError(kind)


def evaluate(kind, data, root):
    data = deepcopy(data)
    if kind in ('ending-summary', 'generator-artifact'):
        log = root / 'log'
        if data['log'] is not None:
            log.write_bytes(data['log'].encode())
        data['ending']['result']['stdout']['path'] = str(log)
        if kind == 'ending-summary':
            return pb_tests._summary(data['ending'], KEY)
        artifact, rec = pb_generators._artifact(data['ending'], KEY)
        # Invoke the real worker shell for its identity check and final import.
        work = root / 'work'
        work.mkdir()
        for name in data['before']:
            target = work / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(b'# generated\n')
        with Worktree(work) as tree:
            tree.test_checkout = work
            tree.test_settings = pb_tests.TestSettings(IMAGE, root / 'tools')
            tree.generator = data['contract']['command']
            tree.generated_files = data['contract']['generated_files']
            tree.protected_paths = data['contract']['protected_paths']
            tree.generator_files = data['contract']['files']
            selected = generator.budget(1, 2, generator.TIMEOUT)
            identity = {**data['contract'], 'timeout': selected['timeout_s'], 'cpus': 1, 'mem_gb': 2,
                        'image': IMAGE, 'scratch': pb_generators.scratch_environment(),
                        'implementation': generator.IMPLEMENTATION, 'budget': selected}
            # make_generator_tool adds timeout/cpus/memory to its contract.
            tree.generator_checkpoint = {'phase': 'submitted', 'contract': identity, 'key': KEY,
                                         'before': data['before'], 'input_tree': '1' * 40}
            waited = {'schema': 'prismabuild.endings.v1', 'complete': True, 'endings': [data['ending']]}
            with patch.object(pb_generators, '_json_command', return_value=(0, waited)), \
                    patch.object(pb_generators, '_stage', return_value='2' * 40):
                result = pb_generators.make_generator_tool(tree)()
            return {'artifact': artifact, 'receipt': rec, 'result': result,
                    'files': {name: (work / name).read_text() for name in generator.manifest(work)}}
    if kind == 'failure-receipt':
        tools = root / 'repo' / 'tools'
        tools.mkdir(parents=True)
        target = root / 'pb-queue' / 'failed' / (KEY + '.json')
        if any(key in data for key in ('record', 'record_raw', 'record_padding')):
            target.parent.mkdir(parents=True)
            target.write_text(' ' * data['record_padding'] if 'record_padding' in data else
                              data.get('record_raw', json.dumps(data.get('record'))))
        return pb_tests._failure_receipt(data['ending'], pb_tests.TestSettings(IMAGE, tools))
    if kind == 'post-wait-consistency':
        op = data['operation']
        if op == 'selectors':
            return list(pb_tests._selectors(data['value']))
        if op == 'priority':
            return pb_tests._priority(data['value'])
        if op == 'settings':
            return vars(pb_tests.TestSettings(**data['value']))
        work = SimpleNamespace(check_base_sha=BASE, test_checkout=root)
        def stage(tree, directory, base, **kwargs):
            (directory / 'proof.json').write_text(json.dumps(data['proof']))
            return '1' * 40
        waited = {'schema': 'prismabuild.endings.v1', 'complete': True,
                  'endings': [{'action_key': KEY, 'succeeded': True}]}
        with patch.object(pb_tests, '_stage', side_effect=stage), \
                patch.object(pb_tests, '_git', return_value='2' * 40), \
                patch.object(pb_tests, '_summary', return_value=data['result']), \
                patch.object(pb_tests, '_json_command', side_effect=[
                    (0, {'schema': 'prismabuild.submission.v1', 'action_key': KEY}), (0, waited)]):
            return pb_tests.make_test_tool(work, pb_tests.TestSettings(IMAGE, root),
                forcing=Forcing.parse(data['forcing']), declared_symptom=data['declared_symptom'],
                _checks=())([NODE], base=data['base'])
    if kind == 'machine-record-fallback':
        calls, fallbacks = [], []
        def run(args, **kwargs):
            probe = '--machine-record-capability' in args
            calls.append('probe' if probe else 'private' if '--machine-record-fd' in args else 'stdout')
            if data.get('probe_io_error') and probe or data.get('action_io_error') and not probe:
                raise OSError('fixture I/O unavailable')
            if probe:
                kwargs['stdout'].write(data['capability'].encode())
                return SimpleNamespace(returncode=data['capability_rc'])
            private = '--machine-record-fd' in args
            raw = (' ' * data['private_padding'] if private and 'private_padding' in data else
                   ' ' * data['stdout_padding'] if not private and 'stdout_padding' in data else
                   data['private'] if private else data['stdout']).encode()
            if private:
                os.write(int(args[args.index('--machine-record-fd') + 1]), raw)
            else:
                kwargs['stdout'].write(raw)
            return SimpleNamespace(returncode=data['rc'])
        pb_tests._machine_record_supported.cache_clear()
        from tl3_workers import facts
        with patch.object(pb_tests.subprocess, 'run', side_effect=run), \
                patch.object(facts, 'record_prismabuild_fallback', side_effect=fallbacks.append):
            try:
                value = pb_tests._json_command(['python3', str(root / 'pbrun.py'), *(['--detach'] if data.get('detach') else [])])
                # The probe must be cached, including when unsupported.
                supported = pb_tests._machine_record_supported(str(root / 'pbrun.py'))
                return {'value': value, 'supported': supported, 'calls': calls, 'fallbacks': fallbacks}
            except ExecutionFailure as error:
                return {'error': failure(error), 'calls': calls, 'fallbacks': fallbacks}
            finally:
                pb_tests._machine_record_supported.cache_clear()
    if kind == 'rehearsal-identity':
        if 'result' in data:
            return rehearsal.unusable(data['result'], data['selectors'], data['outcome'])
        candidate = data['candidate']
        parsed = Forcing.parse(candidate['forcing'])
        value = rehearsal.identity(candidate['base_sha'], candidate['selectors'], parsed, candidate.get('declared_symptom'))
        result = {'identity': value, 'canonical': rehearsal.canonical(value),
                  'bound_identity': rehearsal.bound_identity(value), 'execution_identity': rehearsal.execution_identity(value),
                  'python_sorted_default': json.dumps(value, sort_keys=True, ensure_ascii=False)}
        if 'receipts' in data:
            ledger = object.__new__(rehearsal.ForcingRehearsals)
            ledger.failure = data.get('failure')
            ledger.receipts = [rehearsal.identity(r['base_sha'], r['selectors'], Forcing.parse(r['forcing']), r.get('declared_symptom')) for r in data['receipts']]
            result['violations'] = ledger.violations(candidate['base_sha'], [{'node_ids': candidate['selectors'], 'forcing': candidate['forcing']}])
        return result
    if kind == 'evidence-ledger':
        ledger, steps = Evidence(), []
        for event in data['events']:
            selected = event['selectors'][0], tuple(event['selectors'][1])
            result = deepcopy(event['result'])
            ledger.test_result(result, selected, event.get('options'))
            state = ledger.state()
            steps.append({'entries': len(state['test_runs']), 'bytes': len(json.dumps(state['test_runs']).encode()),
                          'infrastructure_keys': state['infrastructure_keys'], 'tool_errors': state['tool_errors']})
        if data.get('spend'):
            ledger.spend()
        state = json.loads(json.dumps(ledger.state()))
        restored = Evidence()
        restored.restore(state)
        return {'steps': steps, 'state': state, 'restored': restored.state(),
                'citations': [{'detail': restored.detail(c), 'cites': restored.cites(c)} for c in data.get('cites', [])]}
    raise AssertionError(kind)


def failure(error):
    result = {'type': type(error).__name__, 'message': error.message() if isinstance(error, ExecutionFailure) else str(error)}
    if isinstance(error, ExecutionFailure):
        result['execution'] = error.as_json()
    if error.__cause__ is not None:
        result['cause'] = failure(error.__cause__)
    return result


def observe(kind, data):
    with tempfile.TemporaryDirectory(prefix='pb-evidence-vector-') as directory:
        root = Path(directory)
        try:
            value = {'return': evaluate(kind, data, root)}
        except Exception as error:
            value = {'raise': failure(error)}
        # No temporary or machine-specific paths enter the corpus.
        return json.loads(json.dumps(value, default=str).replace(str(root), '$ROOT'))


# Scope is the evidence decisions, not generator process lifecycle or staging.
COVERED = (pb_tests._summary, pb_tests.TestSettings.__post_init__, pb_tests._selectors, pb_tests._priority,
           pb_tests._json_command, pb_tests._stdout_json_command, pb_generators._artifact,
           generator.validate_artifact, generator.text, generator.changed, generator.path_parts,
           generator._check_interpreter, generator.budget, tools._selectors, Evidence.restore)

REQUIRED_CASES = {
    'ending-summary': {'result-null', 'result-list', 'result-string', 'result-empty',
                       'stdout-null', 'stdout-list', 'stdout-string', 'stdout-empty',
                       'root-null', 'root-list', 'root-string', 'root-int', 'root-bool',
                       'summary-bytes-below', 'summary-bytes-at', 'summary-bytes-above'},
    'machine-record-fallback': {'descriptor-unavailable'},
    'generator-artifact': {'file-bytes-below', 'file-bytes-at', 'file-bytes-above',
                           'total-bytes-below', 'total-bytes-at', 'total-bytes-above',
                           'outputs-below', 'outputs-at', 'outputs-above', 'control-text', 'invalid-utf8'},
    'evidence-ledger': {'invalid-selectors-base', 'invalid-selectors-test',
                        'invalid-state-key', 'invalid-state-tool', 'invalid-state-runs', 'invalid-state-run'},
}


def refusal_patterns():
    """Derive literal refusals, f-string templates, and transport cause tokens."""
    result = set()
    for function in COVERED:
        tree = ast.parse(textwrap.dedent(inspect.getsource(function)))
        for node in ast.walk(tree):
            values = []
            if isinstance(node, ast.Raise) and isinstance(node.exc, ast.Call):
                # ExecutionFailure(outcome, phase, reason), ValueError(message).
                values = node.exc.args[2:3] if len(node.exc.args) == 3 else node.exc.args[:1]
            if isinstance(node, ast.Assign) and any(isinstance(t, ast.Name) and t.id == 'cause' for t in node.targets):
                values = [node.value]
            for value in values:
                if isinstance(value, ast.Constant) and isinstance(value.value, str):
                    result.add(re.escape(value.value))
                elif isinstance(value, ast.JoinedStr):
                    result.add(''.join(re.escape(v.value) if isinstance(v, ast.Constant) else '.+' for v in value.values))
    return result


def check_coverage(vectors):
    observed = {text for items in vectors.values() for case in items for text in strings(case['output'])}
    missing = {pattern for pattern in refusal_patterns() if not any(re.fullmatch(pattern, text) for text in observed)}
    absent = {kind: sorted(names - {case['case'] for case in vectors[kind]})
              for kind, names in REQUIRED_CASES.items() if names - {case['case'] for case in vectors[kind]}}
    assert not missing and not absent, f'refusal/bound coverage missing: refusals={sorted(missing)}, cases={absent}'
    for kind in KINDS:
        assert vectors[kind], f'{kind}: no cases'


def strings(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for item in value.values():
            yield from strings(item)
    elif isinstance(value, list):
        for item in value:
            yield from strings(item)


def read_vectors(directory=DIRECTORY):
    result = {}
    for kind in KINDS:
        document = json.loads((directory / (kind + '.json')).read_text())
        assert document['kind'] == kind
        assert document['version'] == 1
        result[kind] = document['cases']
    return result


def check_vectors(directory=DIRECTORY):
    vectors = read_vectors(directory)
    for kind in KINDS:
        wanted = list(cases(kind))
        assert [case['case'] for case in vectors[kind]] == [name for name, _ in wanted], f'{kind}: case inventory drift'
        for case, (name, data) in zip(vectors[kind], wanted):
            assert encoded(case['input']) == encoded(data), f'{kind}/{name}: input drift'
            actual = observe(kind, data)
            assert encoded(case['output']) == encoded(actual), f'{kind}/{name}: Python drift\nexpected={str(case["output"])[:1000]}\nactual={str(actual)[:1000]}'
    check_coverage(vectors)
    return vectors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    parser.add_argument('--output-dir', type=Path, default=DIRECTORY)
    args = parser.parse_args()
    if args.check:
        vectors = check_vectors(args.output_dir)
    else:
        args.output_dir.mkdir(parents=True, exist_ok=True)
        vectors = {kind: [{'case': name, 'input': data, 'output': observe(kind, data)} for name, data in cases(kind)] for kind in KINDS}
        check_coverage(vectors)
        for kind, items in vectors.items():
            (args.output_dir / (kind + '.json')).write_text(encoded({'version': 1, 'kind': kind, 'cases': items}))
    print(json.dumps({kind: len(items) for kind, items in vectors.items()}, sort_keys=True))


if __name__ == '__main__':
    main()
