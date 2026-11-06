"""Planner repairs from captured facts, using the shared refusal row contract.

A1 keeps this table in Python until C3 ports it. Constraints here describe edits
of captured declarations, not a new reply or wire schema.
"""

import ast
from copy import deepcopy
from dataclasses import replace
from functools import cache
import json
from pathlib import Path, PurePosixPath

from tl3_workers.evidence import subject_covers
from tl3_workers.rules import (
    Allowed, CantMeet, Edit, Name, Rule, at, evaluate, register,
)


@cache
def inventory():
    return json.loads((Path(__file__).parent.parent / 'plan_rules.json').read_text())


def unique(values):
    return list({json.dumps(value, sort_keys=True): value for value in values}.values())


def fixed(value):
    return Allowed('const', value)


def object_constraint(original, changes):
    properties = {key: {'const': value} for key, value in original.items()}
    properties.update(changes)
    return {'type': 'object', 'properties': properties, 'required': list(properties), 'additionalProperties': False}


def append_constraint(before, additions):
    """Retain the original prefix; every additional identity is named by the facts."""
    additions = [value for value in unique(additions) if value not in before]
    if not additions:
        return {'const': before}
    return {'type': 'array', 'prefixItems': [{'const': value} for value in before],
            'minItems': len(before), 'maxItems': len(before) + len(additions),
            'items': {'enum': additions}, 'uniqueItems': True}


def edit(facts, pointer, allowed):
    try:
        at(facts['draft'], pointer)
        op = 'replace'
    except (KeyError, IndexError):
        op = 'add'
    return Edit(Name('field', pointer), op, allowed)


def terminal(facts):
    return [CantMeet('No bounded declaration edit is supported by the captured facts; return cant_meet.')]


def code(f):
    keys = f['items'].get('keys', [f['items'].get('field')])
    return [Edit(Name('field', '/' + key), 'remove', fixed(None)) for key in keys if key in f['draft']] or terminal(f)


def evidence_empty(f):
    return [edit(f, '/evidence_contract', fixed([]))] if 'evidence_contract' not in f['draft'] else terminal(f)


def proof_fill(f):
    index = f['items'].get('check')
    if type(index) is not int or index >= len(f['draft'].get('checks', [])):
        return terminal(f)
    check = f['draft']['checks'][index]
    proof = check.get('proof', {})
    if not isinstance(proof, dict):
        return terminal(f)
    missing = [key for key in ('subjects', 'observer_support', 'red_message') if key not in proof]
    quotes = unique(goal['proof_obligation']['quote'] for goal in f['inputs']['ingest']['goals']
                    if 'proof_obligation' in goal)
    if 'red_message' in missing and not quotes:
        return [CantMeet('No ingest proof_obligation quote permits red_message; return cant_meet without inventing an assertion.')]
    changes = {key: {'enum': quotes} if key == 'red_message' else {'const': []} for key in missing}
    required = [unit for item in f['diagnostics'] if item.items.get('check') == index
                and inventory()[item.rule]['footprint'] == 'SUBJECT_ADD' for unit in item.items.get('subjects', [])
                if unit.split('::')[0] in f['draft'].get('files', [])]
    if required:
        changes['subjects'] = append_constraint(proof.get('subjects', []), required)
    if not changes:
        return terminal(f)
    return [edit(f, f'/checks/{index}/proof', Allowed('schema', object_constraint(proof, changes)))]


def red_fill(f):
    index = f['items'].get('check')
    quote = f['items'].get('obligation', {}).get('quote')
    if type(index) is not int or not quote or f['draft']['checks'][index].get('proof', {}).get('red_message', '').strip():
        return terminal(f)
    return [edit(f, f'/checks/{index}/proof/red_message', Allowed('enum', [quote]))]


def subjects(f):
    index = f['items'].get('check')
    if type(index) is not int:
        return terminal(f)
    before = f['draft']['checks'][index].get('proof', {}).get('subjects', [])
    required = f['items'].get('subjects', f['items'].get('obligation', {}).get('subjects', []))
    additions = []
    for unit in required:
        path, _, symbol = unit.partition('::')
        if path in f['draft'].get('files', []):
            additions.extend([unit, path])
            if symbol:
                additions.extend(path + '::' + '.'.join(symbol.split('.')[:n]) for n in range(1, len(symbol.split('.'))))
    if not additions:
        return terminal(f)
    return [edit(f, f'/checks/{index}/proof/subjects', Allowed('schema', append_constraint(before, additions)))]


def selectors(f):
    index = f['items'].get('check')
    if type(index) is not int:
        return terminal(f)
    check = f['draft']['checks'][index]
    rejected = f['items'].get('nodes', [f['items'].get('node')])
    changes = []
    for slot, node in enumerate(check['node_ids']):
        if node not in rejected:
            continue
        path = node.split('::')[0]
        source = f.get('base_sources', {}).get(path)
        choices = []
        if source is not None:
            try:
                tree = ast.parse(source)
            except SyntaxError:
                tree = ast.Module(body=[], type_ignores=[])
            for definition in tree.body:
                if isinstance(definition, (ast.FunctionDef, ast.AsyncFunctionDef)):
                    choices.append(path + '::' + definition.name)
                elif isinstance(definition, ast.ClassDef):
                    choices.extend(path + '::' + definition.name + '::' + method.name for method in definition.body
                                   if isinstance(method, (ast.FunctionDef, ast.AsyncFunctionDef)))
        if not choices:
            return terminal(f)
        changes.append(edit(f, f'/checks/{index}/node_ids/{slot}', Allowed('enum', choices)))
    return changes or terminal(f)


def files(f):
    before = f['draft'].get('files')
    if before is None:
        return terminal(f)
    operation = f['footprint']
    source_units = [unit for goal in f['inputs']['ingest']['goals']
                    for unit in goal.get('proof_obligation', {}).get('subjects', [])]
    additions = []
    if operation == 'FILES_FILL' and not before:
        additions = [unit.split('::')[0] for unit in source_units]
    elif operation == 'PROOF_FILE' and any(subject_covers(f['items']['unit'], unit) or subject_covers(unit, f['items']['unit'])
                                           for unit in source_units):
        additions = [f['items']['path']]
    elif operation == 'EDIT_FILE' and f['items']['node'] in f.get('base_nodes', ()):
        additions = [f['items']['path']]
    elif operation == 'FILE_PATH':
        path = f['items'].get('path', '')
        canonical = str(PurePosixPath(path))
        if path and not path.startswith(('/', '-')) and '..' not in PurePosixPath(path).parts and '\\' not in path and canonical != path:
            return [edit(f, '/files', fixed([canonical if value == path else value for value in before]))]
    if not additions:
        return terminal(f)
    return [edit(f, '/files', Allowed('schema', append_constraint(before, additions)))]


def citations(f):
    goals = f['inputs']['ingest']['goals']
    operation, item, draft = f['footprint'], f['items'], f['draft']
    pieces = operation.startswith('PIECE_')
    field = 'pieces' if pieces else 'checks'
    values = draft.get(field, [])
    indexes = range(len(values)) if operation in {'COVERAGE', 'PIECE_COVER'} else [item.get('piece' if pieces else 'check')]
    repairs = []
    covered = {g for value in values for g in value['goal_ids']}
    missing = sorted(set(range(len(goals))) - covered)
    for index in indexes:
        if type(index) is not int or index >= len(values):
            continue
        value = values[index]
        before = value['goal_ids']
        if operation in {'GOAL_DROP', 'PIECE_DROP', 'GOAL_SANITIZE'}:
            result = unique(g for g in before if type(g) is int and 0 <= g < len(goals))
            if result != before:
                repairs.append(edit(f, f'/{field}/{index}/goal_ids', fixed(result)))
            continue
        additions = []
        if operation == 'COVERAGE':
            additions = item['goals']
        elif pieces:
            candidates = missing if operation == 'PIECE_COVER' else range(len(goals)) if not before else []
            additions = [g for g in candidates if goals[g]['description'] in value['brief']
                         and all(unit.split('::')[0] in value['files'] for unit in goals[g].get('proof_obligation', {}).get('subjects', []))]
        elif not before:
            proof = value.get('proof', {})
            additions = [g for g, goal in enumerate(goals) if 'proof_obligation' in goal
                         and proof.get('red_message') == goal['proof_obligation']['quote']
                         and all(any(subject_covers(unit, required) for unit in proof.get('subjects', []))
                                 for required in goal['proof_obligation']['subjects'])]
        if additions:
            repairs.append(edit(f, f'/{field}/{index}/goal_ids', Allowed('schema', append_constraint(before, additions))))
    return repairs or terminal(f)


def split_reasons(f):
    completed, inputs = f['completed'], f['inputs']
    return (["out_of_room"] if inputs.get('must_split') else []) + (
        ["does_not_fit"] if not completed.get('forecast', {}).get('fits', True) else []) + (
        ["execution_budget"] if not completed.get('execution', {}).get('fits', True) else [])


def reason(f):
    choices = split_reasons(f)
    return [edit(f, '/reason', Allowed('enum', choices))] if choices else [
        CantMeet('No split reason is true in the captured forecast, execution and must_split facts; return cant_meet.')]


def lossless_split(f):
    draft = f['draft']
    choices = split_reasons(f)
    if not choices:
        return terminal(f)
    paths = draft.get('files', unique(path for piece in draft.get('pieces', []) for path in piece['files']))
    goal_ids = list(range(len(f['inputs']['ingest']['goals'])))
    piece = {'type': 'object', 'properties': {
        'brief': {'type': 'string', 'minLength': 1}, 'goal_ids': {'type': 'array', 'items': {'enum': goal_ids}},
        'files': {'type': 'array', 'items': {'enum': paths}},
        'estimated_exploration_turns': {'type': 'integer', 'minimum': 0},
        'estimated_tool_turns': {'type': 'integer', 'minimum': 0}},
        'required': ['brief', 'goal_ids', 'files', 'estimated_exploration_turns'], 'additionalProperties': False}
    requirements = [({'files': [path], 'goal_ids': []}) for path in paths]
    for check in draft.get('checks', []):
        required_paths = unique([node.split('::')[0] for node in check['node_ids']] + [
            unit.split('::')[0] for field in ('subjects', 'observer_support') for unit in check.get('proof', {}).get(field, [])])
        requirements.append({'files': required_paths, 'goal_ids': check['goal_ids']})
    requirements.append({'files': [], 'goal_ids': []})
    bounded_pieces = {'type': 'array', 'minItems': 2, 'items': piece,
                      'allOf': [{'contains': {'properties': {
                          field: {'allOf': [{'contains': {'const': value}} for value in values]}
                          for field, values in required.items() if values}}} for required in requirements]}
    frozen = {key: value for key, value in draft.items() if key not in {
        'kind', 'files', 'checks', 'edits_tests', 'no_design_questions', 'lands_on_its_own', 'pieces', 'reason'}}
    constraint = object_constraint(frozen, {'kind': {'const': 'split'}, 'pieces': bounded_pieces, 'reason': {'enum': choices}})
    return [Edit(Name('field', ''), 'replace', Allowed('schema', constraint))]


def deduplicate(f):
    index = f['items'].get('check')
    if type(index) is not int:
        return terminal(f)
    check = f['draft']['checks'][index]
    if f['footprint'] == 'NODE_DEDUP':
        return [edit(f, f'/checks/{index}/node_ids', fixed(unique(check['node_ids'])))]
    proof = check.get('proof', {})
    result = {key: unique(value) if key in {'subjects', 'observer_support'} else value for key, value in proof.items()}
    return [edit(f, f'/checks/{index}/proof', fixed(result))] if result != proof else terminal(f)


def drop_edits(f):
    nodes = [f['items'].get('node')] if 'node' in f['items'] else f['items'].get('nodes', [])
    covered = {node for check in f['draft'].get('checks', []) if check['kind'] == 'demonstrating' for node in check['node_ids']}
    if not set(nodes) <= covered:
        return terminal(f)
    return [edit(f, '/edits_tests', fixed([node for node in f['draft'].get('edits_tests', []) if node not in nodes]))]


def edit_coverage(f):
    draft = f['draft']
    nodes = unique(node for item in f['diagnostics'] if inventory()[item.rule]['footprint'] == 'EDIT_COVER'
                   for node in item.items.get('nodes', []))
    if not nodes:
        return terminal(f)
    # The alternative is a new demonstrating check or dropping an unnecessary
    # uncovered edit. One bounded root edit expresses that choice without
    # requiring the model to perform every alternative at once.
    changes = {}
    for item in f['diagnostics']:
        operation = inventory()[item.rule]['footprint']
        if operation in {'EDIT_COVER', 'SPLIT'}:
            continue
        context = {**f, 'items': item.items, 'footprint': operation}
        for repair in REPAIRS[operation](context):
            if isinstance(repair, Edit):
                changes[repair.pointer.value] = False if repair.op == 'remove' else (
                    repair.allowed.value if repair.allowed.keyword == 'schema' else {repair.allowed.keyword: repair.allowed.value})
    for index, check in enumerate(draft.get('checks', [])):
        if check['kind'] == 'demonstrating':
            changes[f'/checks/{index}/node_ids'] = append_constraint(check['node_ids'], nodes)

    def bounded(value, pointer=''):
        if pointer in changes:
            return changes[pointer]
        if isinstance(value, dict):
            properties = {key: bounded(child, pointer + '/' + key) for key, child in value.items()}
            prefix = pointer + '/'
            for path, constraint in changes.items():
                suffix = path.removeprefix(prefix)
                if path.startswith(prefix) and '/' not in suffix:
                    properties[suffix] = constraint
            return {'type': 'object', 'properties': properties,
                    'required': [key for key, constraint in properties.items() if constraint is not False],
                    'additionalProperties': False}
        if isinstance(value, list):
            return {'type': 'array', 'prefixItems': [bounded(child, pointer + f'/{i}') for i, child in enumerate(value)],
                    'minItems': len(value), 'maxItems': len(value)}
        return {'const': value}

    constraint = bounded(draft)
    original_checks = draft.get('checks', [])
    goals = unique(g for check in original_checks for g in check['goal_ids'])
    new_check = {'type': 'object', 'properties': {
        'kind': {'const': 'demonstrating'},
        'node_ids': {'type': 'array', 'items': {'enum': nodes}, 'minItems': 1, 'uniqueItems': True},
        'goal_ids': {'type': 'array', 'items': {'enum': goals}, 'minItems': 1},
        'proof': {'enum': unique(check['proof'] for check in original_checks if 'proof' in check)},
        'forcing': {'enum': unique(check['forcing'] for check in original_checks if 'forcing' in check)}},
        'required': ['kind', 'node_ids', 'goal_ids'], 'additionalProperties': False}
    checks = constraint['properties']['checks']
    checks.pop('maxItems')
    checks['maxItems'] = len(original_checks) + len(nodes)
    checks['items'] = new_check
    before = draft.get('edits_tests', [])
    # Every retained declaration stays in order; omissions are confined to the
    # uncovered identities. Enumerate the bounded deletion alternatives.
    from itertools import combinations
    optional = [i for i, node in enumerate(before) if node in nodes]
    alternatives = [[node for i, node in enumerate(before) if i not in removed]
                    for count in range(len(optional) + 1) for removed in combinations(optional, count)]
    if 'edits_tests' in draft:
        constraint['properties']['edits_tests'] = {'enum': alternatives}
    return [Edit(Name('field', ''), 'replace', Allowed('schema', constraint))]


def frozen_nodes(f):
    node = f['items'].get('node')
    repairs = []
    for index, check in enumerate(f['draft'].get('checks', [])):
        remaining = [value for value in check['node_ids'] if value != node]
        if check['kind'] in {'regression', 'behavior-preserving'} and remaining and remaining != check['node_ids']:
            repairs.append(edit(f, f'/checks/{index}/node_ids', fixed(remaining)))
    return repairs or terminal(f)


def memory(f):
    draft, item = f['draft'], f['items']
    if f['footprint'] == 'MEMORY_DROP':
        return [edit(f, '/memory_ids', fixed([value for value in draft.get('memory_ids', []) if value not in item.get('ids', [])]))]
    flags = draft.get('memory_flags', [])
    if f['footprint'] == 'FLAG_DROP':
        result = [flag for flag in flags if flag['id'] not in item.get('ids', [])]
    else:
        # Dropping the named flag is always bounded and avoids inventing a passage.
        result = [flag for index, flag in enumerate(flags) if index != item.get('flag')]
    return [edit(f, '/memory_flags', fixed(result))]


def sanitize(f):
    index = f['items'].get('check')
    if type(index) is not int:
        return terminal(f)
    check = f['draft']['checks'][index]
    if f['footprint'] == 'SYMPTOM_FIELDS':
        field = 'declared_symptom'
        result = {key: value for key, value in check.get(field, {}).items() if key in {'node', 'exception', 'phase', 'repeat', 'parallel'}}
        if 'node' not in result and len(check['node_ids']) == 1:
            result['node'] = check['node_ids'][0]
    else:
        field = 'forcing'
        result = deepcopy(check.get(field, {}))
        if f['footprint'] == 'FORCING_KEYS':
            result = {key: value for key, value in result.items() if key in {'symptom', 'environment', 'patches'}}
        else:
            for patch in result.get('patches', []):
                modes = patch.keys() & {'value', 'returns', 'raises', 'delay_seconds'}
                if len(modes) == 1:
                    for key in list(patch):
                        if key not in {'target', *modes}:
                            del patch[key]
    return [edit(f, f'/checks/{index}/{field}', fixed(result))]


def methods(f):
    before = f['draft'].get('evidence_contract', [])
    mutable = {'tool', 'executor', 'runner_features', 'artifacts', 'methods', 'protected_files'}
    repairs = []
    for index, requirement in enumerate(before):
        if requirement != f['items'].get('requirement') or requirement.get('provenance', {}).get('kind') != 'method':
            continue
        alternatives = [{**requirement, **{key: cap[key] for key in mutable}} for cap in f['inputs'].get('tool_capabilities', [])]
        if alternatives:
            repairs.append(edit(f, f'/evidence_contract/{index}', Allowed('enum', alternatives)))
    return repairs or terminal(f)


REPAIRS = {
    'CODE': code, 'EVIDENCE_EMPTY': evidence_empty, 'PROOF_FILL': proof_fill, 'RED_FILL': red_fill,
    'SUBJECT_ADD': subjects, 'EXACT_SELECT': selectors, 'BASE_SELECT': selectors, 'FORCED_SELECT': selectors,
    'FILES_FILL': files, 'PROOF_FILE': files, 'EDIT_FILE': files, 'FILE_PATH': files,
    'COVERAGE': citations, 'GOAL_ADD': citations, 'GOAL_DROP': citations, 'GOAL_SANITIZE': citations,
    'PIECE_GOALS': citations, 'PIECE_DROP': citations, 'PIECE_COVER': citations,
    'SPLIT_REASON': reason, 'SPLIT': lossless_split, 'PROOF_DEDUP': deduplicate, 'NODE_DEDUP': deduplicate,
    'EDIT_DROP': drop_edits, 'EDIT_COVER': edit_coverage, 'FROZEN': frozen_nodes,
    'MEMORY_DROP': memory, 'FLAG_DROP': memory, 'FLAG_CLAIM': memory,
    'SYMPTOM_FIELDS': sanitize, 'PATCH_MODE': sanitize, 'FORCING_KEYS': sanitize, 'METHOD_REPLACE': methods,
    **dict.fromkeys(('NONE', 'FILE_DROP', 'PROTECTED', 'TESTS_FILL', 'FORCING_RULE', 'FORCING_RESERVED',
                     'FORCING_TEXT', 'RECEIPT_SELECT', 'OBSERVER_SELECT', 'EDIT_SELECT'), terminal),
}


def satisfied(reply, repair):
    if isinstance(repair, CantMeet):
        return reply.get('kind') == 'cant_meet'
    try:
        value = at(reply, repair.pointer.value)
    except (KeyError, IndexError):
        return repair.op == 'remove'
    return repair.op != 'remove' and repair.allowed.accepts(value)


def check_row(rule_id, facts):
    if facts['reply'].get('kind') == 'cant_meet':
        return
    for item in facts.get('diagnostics', ()):
        if item.rule != rule_id:
            continue
        context = {**facts, 'items': item.items, 'footprint': inventory()[rule_id]['footprint']}
        repairs = REPAIRS[context['footprint']](context) if item.items else terminal(context)
        for repair in repairs:
            if facts['reply'] == facts['draft'] or not satisfied(facts['reply'], repair):
                pointer = repair.pointer if isinstance(repair, Edit) else Name('field', subject_pointer(context))
                yield {**context, 'repair': repair, 'subject': pointer,
                       'bad_value': item.items}


def subject_pointer(f):
    item = f['items']
    if type(item.get('check')) is int:
        return f"/checks/{item['check']}"
    if type(item.get('piece')) is int:
        return f"/pieces/{item['piece']}"
    field = item.get('field')
    return '/' + field.replace('.', '/') if field else '/reason' if f['footprint'] == 'SPLIT_REASON' else ''


def allowed(f):
    repair = f['repair']
    return repair.allowed if isinstance(repair, Edit) else Allowed('enum', [])


def make_row(rule_id):
    return Rule(rule_id, lambda f: check_row(rule_id, f), lambda f: (f['subject'],), allowed, lambda f: f['repair'])


@cache
def rows():
    # Register the actual row callbacks. An impossible draft is a terminal
    # fixture; interface tests additionally register every bounded edit fixture.
    from tl3_harness.model import named
    result = []
    for rule_id in inventory():
        example = {'reply': {'kind': 'leaf'}, 'draft': {'kind': 'leaf'},
                   'completed': {}, 'inputs': {'ingest': {'goals': []}},
                   'diagnostics': [named(rule_id, '', {})]}
        result.append(register(replace(make_row(rule_id), example=example, witness={'kind': 'cant_meet'}), roles={'planner'}))
    return tuple(result)


def violations(facts):
    return evaluate(rows(), facts, role='planner')
