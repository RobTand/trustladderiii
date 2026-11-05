"""Test-only ordered facts and replay for the planner migration (#820).

Python remains here as the pre-migration oracle, not as a wire type definition.
The corpus stores actual arguments, mutations, exceptions and code-owned fields.
"""
from contextlib import ExitStack, nullcontext
from copy import deepcopy
import importlib
import gzip
import json
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
MISMATCH_FIELDS = ('uncovered', 'missing_forced', 'selectors', 'missing_proof', 'missing_subjects')


def packed(value):
    """Keep sets/tuples and integer map keys distinct from JSON lists/keys."""
    if isinstance(value, set):
        return {'$set': sorted((packed(item) for item in value), key=repr)}
    if isinstance(value, tuple):
        return {'$tuple': [packed(item) for item in value]}
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
        if '$dict' in value:
            return {unpacked(key): unpacked(item) for key, item in value['$dict']}
    return value


def exception(error):
    result = {'class': type(error).__module__ + '.' + type(error).__name__, 'message': str(error)}
    if isinstance(error, ValidationError):
        result.update(path=list(error.absolute_path), schema_path=list(error.absolute_schema_path))
    if isinstance(error, planner.PlanConsistencyError):
        result.update(local=error.local, violations=deepcopy(error.violations),
                      node_ids=packed(error.node_ids), draft=deepcopy(error.draft),
                      mismatch=packed({name: getattr(error, name) for name in MISMATCH_FIELDS}))
    if hasattr(error, 'result'):
        result['result'] = deepcopy(error.result)
    return result


def raise_recorded(error):
    module, name = error['class'].rsplit('.', 1)
    cls = getattr(importlib.import_module(module), name)
    raise cls(error['result'] if 'result' in error else error['message'])


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
            assert request == {key: row[key] for key in request}, (self.position, request, row)
            if 'error' in row:
                raise_recorded(row['error'])
            return unpacked(row['result'])
        row = dict(request)
        self.rows.append(row)
        try:
            result = invoke()
        except Exception as error:
            row['error'] = exception(error)
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


def facts(tape):
    stack = ExitStack()
    stack.enter_context(patch.object(planner, 'base_definition', tape.wrap('base_definition', planner.base_definition)))
    stack.enter_context(patch.object(planner, 'execution_forecast', tape.wrap('execution_forecast', planner.execution_forecast)))
    original = planner.Forecaster
    stack.enter_context(patch.object(planner, 'Forecaster', lambda config: ForecastFacts(tape, original, config)))
    outputs = planner.configured_outputs
    stack.enter_context(patch.object(planner, 'configured_outputs', lambda tree, *args:
        tape.call('configured_outputs', args, {}, lambda: outputs(tree, *args))))
    # Opening this object performs no reads used by decisions; configured_outputs
    # owns the filesystem observation and its ordered arguments/result above.
    if not tape.recording:
        stack.enter_context(patch.object(planner, 'Worktree', lambda *_: nullcontext()))
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
    'planner': ('proof_omissions', 'planner_schema_reply', 'proof_for_validation', 'execution_budget_details', 'edit_violations',
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
    """Derive diagnostic templates from source; keep branch coverage reviewable."""
    import ast
    import re

    def pattern(node):
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            return re.escape(node.value), len(node.value)
        if isinstance(node, ast.JoinedStr):
            parts = [pattern(value) for value in node.values]
        elif isinstance(node, ast.BinOp) and isinstance(node.op, ast.Add):
            parts = [pattern(node.left), pattern(node.right)]
        elif isinstance(node, (ast.ListComp, ast.GeneratorExp)):
            return pattern(node.elt)
        elif isinstance(node, ast.List) and len(node.elts) == 1:
            return pattern(node.elts[0])
        else:
            return '.*', 0
        return ''.join(text for text, _ in parts), sum(size for _, size in parts)

    rules = {}
    for module_name, module in modules().items():
        if module_name == 'contracts':
            continue
        tree = ast.parse(Path(module.__file__).read_text())
        if module_name == 'recall':
            tree.body = [node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == 'check_citations']
        for node in ast.walk(tree):
            candidates = []
            if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute):
                target = ast.unparse(node.func.value)
                if node.func.attr in ('append', 'extend') and target in ('errors', 'violations', 'self.violations'):
                    candidates.extend(node.args[:1])
            if isinstance(node, ast.Raise) and isinstance(node.exc, ast.Call):
                candidates.extend(node.exc.args[:1])
            if isinstance(node, ast.Assign) and any(ast.unparse(target) in ('violations', 'self.violations') for target in node.targets):
                value = node.value
                candidates.append(value.body if isinstance(value, ast.IfExp) else value)
            for candidate in candidates:
                regex, size = pattern(candidate)
                if size >= 12:
                    key = f'{module_name}:{candidate.lineno}: {ast.unparse(candidate)}'
                    rules[key] = regex
    return rules


def rule_coverage(document):
    import re
    table = strings(document)
    result = {}
    for rule, regex in diagnostic_rules().items():
        if '.*' not in regex:
            literal = re.sub(r'\\(.)', r'\1', regex)
            result[rule] = sorted(set(table.get(literal, [])))
            continue
        fragments = [re.sub(r'\\(.)', r'\1', fragment) for fragment in regex.split('.*') if fragment]
        longest = max(fragments, key=len)
        result[rule] = sorted({case for message, ids in table.items()
                               if '\n' not in message and len(message) <= 8192
                               and longest in message and re.fullmatch(regex, message, re.S)
                               for case in ids})
    return result
