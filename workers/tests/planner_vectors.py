"""Test-only ordered facts and replay for the planner migration (#820).

Python remains here as the pre-migration oracle, not as a wire type definition.
The corpus stores actual arguments, mutations, exceptions and code-owned fields.
"""
from contextlib import ExitStack
import base64
from copy import deepcopy
import importlib
import inspect
import gzip
import json
from subprocess import CalledProcessError
from pathlib import Path
from unittest.mock import patch

from jsonschema import ValidationError

from tl3_workers.roles import planner

CORPUS = Path(__file__).with_name('planner-vectors.json.gz')
SOURCES = (
    'planner_protocol', 'planner_forecast', 'planner_consistency', 'planner_checks',
    'planner_execution', 'planner_forcing_evidence', 'planner_context',
    'planner_proof_correction', 'planner_applicability_correction',
    'planner_file_selectors', 'planner_frozen_tests', 'proof_obligations',
    'subject_observer_contract', 'merge_prerequisites', 'recall', 'workers',
)

def mismatch_fields():
    # A new constructor defect, including #809 coverage, must enter the wire.
    return tuple(name for name in inspect.signature(planner.PlanConsistencyError).parameters if name != 'draft')


def packed(value):
    """Keep sets/tuples and integer map keys distinct from JSON lists/keys."""
    if isinstance(value, set):
        return {'$set': sorted((packed(item) for item in value), key=repr)}
    if isinstance(value, tuple):
        return {'$tuple': [packed(item) for item in value]}
    if isinstance(value, bytes):
        try:
            return {'$bytes_utf8': value.decode('utf-8')}
        except UnicodeError:
            return {'$bytes': base64.b64encode(value).decode('ascii')}
    if isinstance(value, dict):
        return {'$dict': [[packed(key), packed(item)] for key, item in value.items()]}
    if isinstance(value, list):
        return [packed(item) for item in value]
    if value is None or type(value) in (str, int, float, bool):
        return value
    raise TypeError(f'unsupported corpus value: {type(value).__name__}')


def unpacked(value):
    if isinstance(value, list):
        return [unpacked(item) for item in value]
    if isinstance(value, dict):
        if '$set' in value:
            return set(unpacked(item) for item in value['$set'])
        if '$tuple' in value:
            return tuple(unpacked(item) for item in value['$tuple'])
        if '$bytes' in value:
            return base64.b64decode(value['$bytes'])
        if '$bytes_utf8' in value:
            return value['$bytes_utf8'].encode('utf-8')
        if '$dict' in value:
            return {unpacked(key): unpacked(item) for key, item in value['$dict']}
    return value


def exception(error, *, observation=False):
    result = {'class': type(error).__module__ + '.' + type(error).__name__, 'message': str(error)}
    if observation:
        result['args'] = packed(error.args)
        if isinstance(error, OSError):
            result['os_error'] = {name: getattr(error, name) for name in ('errno', 'strerror', 'filename', 'filename2')}
        if isinstance(error, CalledProcessError):
            result['process_error'] = packed({name: getattr(error, name) for name in ('returncode', 'cmd', 'output', 'stderr')})
    if isinstance(error, ValidationError):
        result.update(path=list(error.absolute_path), schema_path=list(error.absolute_schema_path))
    if isinstance(error, planner.PlanConsistencyError):
        result.update(local=error.local, violations=deepcopy(error.violations),
                      node_ids=packed(error.node_ids), draft=deepcopy(error.draft),
                      mismatch=packed({name: getattr(error, name) for name in mismatch_fields()}))
    if hasattr(error, 'result'):
        result['result'] = deepcopy(error.result)
    return result


def raise_recorded(error):
    module, name = error['class'].rsplit('.', 1)
    cls = getattr(importlib.import_module(module), name)
    if 'process_error' in error:
        raise cls(**unpacked(error['process_error']))
    if 'result' in error:
        raise cls(error['result'])
    instance = cls(*unpacked(error['args']))
    if 'os_error' in error:
        for field, value in error['os_error'].items():
            if value is not None:
                setattr(instance, field, value)
    raise instance


class Tape:
    def __init__(self, rows=None):
        self.recording = rows is None
        self.rows = [] if rows is None else deepcopy(rows)
        self.position = 0

    def call(self, name, args, kwargs, invoke):
        request = {'call': name, 'args': packed(args), 'kwargs': packed(kwargs)}
        if not self.recording:
            assert self.position < len(self.rows), f'extra fact {name}'
            row = self.rows[self.position]
            self.position += 1
            assert_same(request, {key: row[key] for key in request}, f'fact {self.position}: {name}')
            if 'error' in row:
                raise_recorded(row['error'])
            return unpacked(row['result'])
        row = dict(request)
        self.rows.append(row)
        try:
            result = invoke()
        except Exception as error:
            row['error'] = exception(error, observation=True)
            raise
        row['result'] = packed(result)
        return result

    def wrap(self, name, original):
        return lambda *args, **kwargs: self.call(name, args, kwargs, lambda: original(*args, **kwargs))

    def exhausted(self):
        assert self.position == len(self.rows), f'unconsumed facts: {self.position}/{len(self.rows)}'


class ForecastFacts:
    def __init__(self, tape, original, config):
        self.tape = tape
        self.original = None

        def create():
            self.original = original(config)
        tape.call('Forecaster', (config,), {}, create)

    def measure(self, *args):
        return self.tape.call('Forecaster.measure', args, {}, lambda: self.original.measure(*args))

    @property
    def problems(self):
        # Decisions use the rendered diagnostics, not exception identities.
        return self.tape.call('Forecaster.problems', (), {}, lambda: [str(p) for p in self.original.problems])


class RehearsalFacts:
    def __init__(self, tape, original):
        self.tape, self.original = tape, original

    def raise_infrastructure(self):
        return self.tape.call('rehearsals.raise_infrastructure', (), {}, self.original.raise_infrastructure
                              if self.original is not None else lambda: None)

    def violations(self, *args):
        return self.tape.call('rehearsals.violations', args, {}, lambda: self.original.violations(*args))

    def __getattr__(self, name):
        assert name in ('failure', 'candidates'), name
        return self.tape.call('rehearsals.' + name, (), {}, lambda: getattr(self.original, name))


def exit_fact(tape, name, exc, invoke):
    error = exception(exc[1], observation=True) if exc[1] is not None else None
    return tape.call(name, (error,), {}, invoke)


class FileFacts:
    def __init__(self, tape, context):
        self.tape, self.context, self.stream = tape, context, None

    def __enter__(self):
        def enter():
            self.stream = self.context.__enter__()
        self.tape.call('Worktree.file.open', (), {}, enter)
        return self

    def read(self, *args):
        return self.tape.call('Worktree.file.read', args, {}, lambda: self.stream.read(*args))

    def __exit__(self, *exc):
        return exit_fact(self.tape, 'Worktree.file.close', exc, lambda: self.context.__exit__(*exc))


class WorktreeFacts:
    """Serve the I/O object's lifecycle while production base_definition executes."""
    def __init__(self, tape, original, *args, **kwargs):
        self.tape, self.original = tape, None
        def open_tree():
            self.original = original(*args, **kwargs)
        tape.call('Worktree.open', args, kwargs, open_tree)

    def __enter__(self):
        def enter():
            self.original.__enter__()
        self.tape.call('Worktree.enter', (), {}, enter)
        return self

    def _file(self, *args, **kwargs):
        context = None
        def create():
            nonlocal context
            context = self.original._file(*args, **kwargs)
        self.tape.call('Worktree.file', args, kwargs, create)
        return FileFacts(self.tape, context)

    def __getattr__(self, name):
        # configured_outputs owns its directory walk observation as one fact.
        return getattr(self.original, name)

    def __exit__(self, *exc):
        return exit_fact(self.tape, 'Worktree.close', exc, lambda: self.original.__exit__(*exc))


def facts(tape):
    stack = ExitStack()
    git = planner.git
    stack.enter_context(patch.object(planner, 'git', lambda repo, *args:
        tape.call('git', (str(repo), *args), {}, lambda: git(repo, *args))))
    stack.enter_context(patch.object(planner, 'test_function_source',
                                    tape.wrap('test_function_source', planner.test_function_source)))
    worktree = planner.Worktree
    stack.enter_context(patch.object(planner, 'Worktree', lambda *args, **kwargs:
        WorktreeFacts(tape, worktree, *args, **kwargs)))
    stack.enter_context(patch.object(planner, 'execution_forecast', tape.wrap('execution_forecast', planner.execution_forecast)))
    original = planner.Forecaster
    stack.enter_context(patch.object(planner, 'Forecaster', lambda config: ForecastFacts(tape, original, config)))
    outputs = planner.configured_outputs
    stack.enter_context(patch.object(planner, 'configured_outputs', lambda tree, *args:
        tape.call('configured_outputs', args, {}, lambda: outputs(tree, *args))))
    return stack


def outcome(function):
    try:
        return {'return': packed(function())}
    except Exception as error:
        # Tape assertions are verifier failures, never oracle outcomes.
        if isinstance(error, AssertionError):
            raise
        return {'error': exception(error)}



def assert_same(actual, expected, case_id):
    # Python equality conflates True, 1 and 1.0; the JSON wire does not.
    left = json.dumps(actual, ensure_ascii=False, separators=(',', ':'))
    right = json.dumps(expected, ensure_ascii=False, separators=(',', ':'))
    if left != right:
        offset = next((i for i, (a, b) in enumerate(zip(left, right)) if a != b), min(len(left), len(right)))
        raise AssertionError(f'{case_id} at byte/character {offset}: actual={left[max(0, offset-60):offset+200]!r}; '
                             f'expected={right[max(0, offset-60):offset+200]!r}')

def check_case(case, function=None, rehearsals=None):
    inputs, reply = deepcopy(case['inputs']), deepcopy(case['reply'])
    tape = Tape(case.get('facts'))
    with facts(tape):
        ledger = RehearsalFacts(tape, rehearsals) if case['has_rehearsals'] else None
        result = outcome(lambda: (function or planner.check_reply)(inputs, reply, rehearsals=ledger))
    result['reply'] = reply
    if tape.recording:
        case['facts'], case['outcome'] = tape.rows, result
    else:
        assert_same(result, case['outcome'], case['id'])
        tape.exhausted()
    return result


def mismatch_from(value):
    return planner.PlanConsistencyError(
        **unpacked(value['mismatch']), draft=deepcopy(value['draft']))


def correction_case(case, mismatch=None, function=None):
    reply = deepcopy(case['reply'])
    tape = Tape(case.get('facts'))
    with facts(tape):
        mismatch = mismatch or mismatch_from(case['mismatch'])
        result = outcome(lambda: (function or getattr(planner.PlanConsistencyError, case['operation']))(mismatch, reply))
    result['reply'] = reply
    if tape.recording:
        case['facts'], case['outcome'] = tape.rows, result
    else:
        assert_same(result, case['outcome'], case['id'])
        tape.exhausted()
    return result


PURE = {
    'planner': ('base_definition', 'proof_omissions', 'planner_schema_reply', 'proof_for_validation', 'execution_budget_details', 'edit_violations',
                'mentions', 'asks_to_edit', 'valid_forcing', 'function'),
    'evidence': ('preflight', 'proof_applicability', 'subject_covers', 'branch_satisfies',
                 'branch_objections', 'forcing_capabilities', 'forcing_guidance'),
    'generated_scope': ('generator_owned_outputs', 'output_freezes', 'generation_scope_conflicts',
                        'check_ingest_generation_scope', 'audit_generation_scope'),
    'prerequisites': ('check_prerequisites',),
    'recall': ('check_citations',),
    'contracts': ('validate',),
}


def modules():
    return {name: importlib.import_module('tl3_workers.' + ('roles.planner' if name == 'planner' else name))
            for name in PURE}


def pure_case(case, function=None, defined=None):
    args, kwargs = unpacked(case['args']), unpacked(case['kwargs'])
    tape = Tape(case.get('facts'))
    if case.get('defined'):
        kwargs['defined'] = tape.wrap('defined', defined)
    module, name = case['operation'].split('.')
    function = function or getattr(modules()[module], name)
    with facts(tape):
        def invoke():
            result = function(*args, **kwargs)
            return list(result) if case['operation'] == 'planner.mentions' else result
        result = outcome(invoke)
    kwargs.pop('defined', None)
    result.update(args=packed(args), kwargs=packed(kwargs))
    if tape.recording:
        case['facts'], case['outcome'] = tape.rows, result
    else:
        assert_same(result, case['outcome'], case['id'])
        tape.exhausted()
    return result


def replay(case):
    if case['operation'] == 'check_reply':
        check_case(case)
    elif case['operation'] in ('constrain', 'constrain_local'):
        correction_case(case)
    else:
        pure_case(case)


def load():
    if not CORPUS.exists():
        return {'cases': [], 'scenarios': {}}
    return json.loads(gzip.decompress(CORPUS.read_bytes()))


def strings(document):
    """Reviewable table of every observed violation/repair/exception string."""
    table = {}
    for case in document['cases']:
        result = case['outcome']
        values = []
        def diagnostics(value):
            if isinstance(value, dict):
                for key, item in value.items():
                    if key in ('violations', 'evidence_errors', 'objections'):
                        if isinstance(item, list):
                            values.extend(value for value in item if isinstance(value, str))
                    else:
                        diagnostics(item)
            elif isinstance(value, (list, tuple)):
                for item in value:
                    diagnostics(item)
        diagnostics(result.get('reply', {}))
        diagnostics(unpacked(result.get('args')))
        diagnostics(unpacked(result.get('kwargs')))
        error = result.get('error', {})
        values.extend(error.get('violations', []))
        if error:
            values.append(error['message'])
        returned = unpacked(result.get('return'))
        if isinstance(returned, list) and all(isinstance(item, str) for item in returned):
            values.extend(returned)
        if case['operation'] == 'planner.edit_violations' and returned:
            values.extend(returned[0])
        for value in values:
            table.setdefault(value, []).append(case['id'])
    return dict(sorted(table.items()))


def diagnostic_rules():
    """Inventory append/subscript, exception-constructor and class diagnostics."""
    import ast
    import re

    def pattern(node):
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            return re.escape(node.value), len(node.value)
        if isinstance(node, ast.JoinedStr):
            parts = [pattern(value) for value in node.values]
        elif isinstance(node, ast.BinOp) and isinstance(node.op, ast.Add):
            parts = [pattern(node.left), pattern(node.right)]
        else:
            return '.*', 0
        return ''.join(text for text, _ in parts), sum(size for _, size in parts)

    def alternatives(node):
        if isinstance(node, (ast.ListComp, ast.GeneratorExp)):
            yield from alternatives(node.elt)
        elif isinstance(node, (ast.List, ast.Tuple)):
            for item in node.elts:
                yield from alternatives(item)
        elif isinstance(node, ast.IfExp):
            yield from alternatives(node.body)
            yield from alternatives(node.orelse)
        else:
            yield node

    def receiver(node):
        if isinstance(node, ast.Name):
            return node.id
        if isinstance(node, ast.Attribute):
            return node.attr
        if isinstance(node, ast.Subscript) and isinstance(node.slice, ast.Constant):
            return node.slice.value

    rules = {}
    for module_name, module in modules().items():
        if module_name == 'contracts':
            continue
        tree = ast.parse(Path(module.__file__).read_text())
        if module_name == 'recall':
            tree.body = [node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == 'check_citations']
        parents = {child: parent for parent in ast.walk(tree) for child in ast.iter_child_nodes(parent)}
        for node in ast.walk(tree):
            candidates = []
            if isinstance(node, ast.Call):
                if isinstance(node.func, ast.Attribute):
                    if node.func.attr in ('append', 'extend') and receiver(node.func.value) in (
                            'errors', 'violations', 'conflicts', 'objections'):
                        candidates.extend(node.args[:1])
                    if (node.func.attr == '__init__' and isinstance(node.func.value, ast.Call)
                            and isinstance(node.func.value.func, ast.Name) and node.func.value.func.id == 'super'):
                        candidates.extend(node.args[:1])
                if isinstance(node.func, ast.Name) and node.func.id.endswith(('Error', 'Prohibition')):
                    candidates.extend(node.args[:1])
            if isinstance(node, ast.Raise) and isinstance(node.exc, ast.Call):
                candidates.extend(node.exc.args[:1])
            if isinstance(node, ast.Assign):
                if any(receiver(target) in ('violations', 'errors', 'objections') for target in node.targets):
                    candidates.append(node.value)
                if (isinstance(parents.get(node), ast.ClassDef)
                        and any(receiver(target) == 'message' for target in node.targets)):
                    candidates.append(node.value)
            for value in candidates:
                for candidate in alternatives(value):
                    regex, size = pattern(candidate)
                    if size >= 12:
                        key = f'{module_name}:{candidate.lineno}: {ast.unparse(candidate)}'
                        rules[key] = regex
    return rules


def rule_coverage(document):
    import re
    table = strings(document)
    result = {}

    def matches(message, parts):
        # Ordered literal search avoids regex backtracking on aggregate repairs.
        if not message.startswith(parts[0]) or not message.endswith(parts[-1]):
            return False
        position = len(parts[0])
        for part in parts[1:-1]:
            found = message.find(part, position)
            if found < 0:
                return False
            position = found + len(part)
        return position <= len(message) - len(parts[-1])

    for rule, regex in diagnostic_rules().items():
        fragments = [re.sub(r'\\(.)', r'\1', fragment, flags=re.S) for fragment in regex.split('.*')]
        if len(fragments) == 1:
            result[rule] = sorted(set(table.get(fragments[0], [])))
            continue
        longest = max(fragments, key=len)
        result[rule] = sorted({case for message, ids in table.items()
                               if longest in message and matches(message, fragments)
                               for case in ids})
    return result
