"""Report plan measurements and rule violations to the controller."""

import ast
from copy import deepcopy
from dataclasses import dataclass, replace
from functools import cache
import json
from pathlib import Path, PurePosixPath
import re
from subprocess import CalledProcessError

from jsonschema import Draft202012Validator, ValidationError
from jsonschema.validators import extend

from tl3_harness.rules import Allowed as MembershipAllowed

from tl3_workers.roles.base import Role
from tl3_workers.evidence import preflight, proof_applicability, subject_covers, planning_refusal
from tl3_workers.forecast import Forecaster, HARD_CAP_BYTES, MAX_EXECUTION_PIECES, execution_forecast
from tl3_harness.model import Diagnostic, named, contextual, validate_symptom
from tl3_harness import Check, Forcing, HarnessInputError, Kind, Proof
from tl3_harness.proof import overlaps
from tl3_harness.limits import validate_check_plan
from tl3_harness.trees import git, test_function_source
from tl3_workers.tools import Worktree
from tl3_workers.generated_scope import configured_outputs
from tl3_workers.recall import check_citations
from tl3_workers.turns import InvalidReply, REPAIR_MESSAGE_BYTES, Unrepairable
from tl3_workers.rules import Allowed as WorkerAllowed, CantMeet, Edit, Name, Rule, at, evaluate, register


# Frozen JSON values retain their wire types, including integers versus floats.
def strict_constant(validator, expected, value, schema):
    if not MembershipAllowed('const', expected).accepts(value):
        yield ValidationError('value differs from the captured JSON constant')


def strict_enum(validator, expected, value, schema):
    if not MembershipAllowed('enum', expected).accepts(value):
        yield ValidationError('value is outside the captured JSON alternatives')


def ordered_values(validator, facts, value, schema):
    """Preserve captured order, allowing only named insertions and deletions."""
    before, additions, removable = (facts[key] for key in ('before', 'add', 'remove'))
    if (not isinstance(value, list) or len(value) > len(before) + len(additions)
            or len(unique(before)) == len(before) and len(unique(value)) != len(value)):
        yield ValidationError('array exceeds the captured identities or multiplicities')
        return
    position = 0
    for item in value:
        if MembershipAllowed('enum', additions).accepts(item):
            continue
        while position < len(before) and not MembershipAllowed('const', before[position]).accepts(item):
            if not MembershipAllowed('enum', removable).accepts(before[position]):
                yield ValidationError('captured array identities must remain in order')
                return
            position += 1
        if position == len(before):
            yield ValidationError('only named additions are permitted')
            return
        position += 1
    if any(not MembershipAllowed('enum', removable).accepts(item) for item in before[position:]):
        yield ValidationError('only named removals are permitted')


def memory_flags(validator, facts, value, schema):
    """Ordered flags with named deletion and exact supplied-passage replacement."""
    if not isinstance(value, list):
        yield ValidationError('memory flags must be an array')
        return
    position = 0
    for index, old in enumerate(facts['before']):
        if position < len(value) and MembershipAllowed('const', old).accepts(value[position]):
            position += 1
            continue
        if index in facts['claims'] and position < len(value) and isinstance(value[position], dict):
            new = value[position]
            claim = new.get('claim')
            if (MembershipAllowed('const', {k: v for k, v in old.items() if k != 'claim'}).accepts(
                    {k: v for k, v in new.items() if k != 'claim'})
                    and isinstance(claim, str) and claim and claim in facts['passages'].get(old['id'], '')):
                position += 1
                continue
        if old['id'] not in facts['drop'] and index not in facts['claims']:
            yield ValidationError('only named flags or their exact supplied claims may change')
            return
    if position != len(value):
        yield ValidationError('flags must retain their captured order and frozen fields')


StrictValidator = extend(Draft202012Validator, {
    'const': strict_constant, 'enum': strict_enum,
    'x-ordered': ordered_values, 'x-memory-flags': memory_flags})


class Allowed(WorkerAllowed):
    def render(self):
        def compact(value):
            if isinstance(value, dict):
                return {key: compact(child) for key, child in value.items()}
            if isinstance(value, list):
                omitted = len(value) > 8 and len(json.dumps(value, ensure_ascii=False)) > 256
                preview = [compact(child) for child in (value[:8] if omitted else value)]
                return preview + ([f'and {len(value) - 8} more (total {len(value)})'] if omitted else [])
            return value
        text = f'{self.keyword} {json.dumps(compact(self.value), ensure_ascii=False, sort_keys=True, separators=(",", ":"))}'
        if '"x-ordered"' in text:
            text += ' x-ordered: keep before in order; only add/remove named values.'
        if '"x-memory-flags"' in text:
            text += ' x-memory-flags: keep order and other fields; drop named IDs or claim indexes, or replace their claim with a nonempty exact passage for the same ID.'
        return text

    def accepts(self, value):
        constraint = self.value if self.keyword == 'schema' else {self.keyword: self.value}
        return StrictValidator(constraint).is_valid(value)


def draft_references(value, draft):
    """Render repeated captured values as pointers without changing membership."""
    def encoded(item):
        return json.dumps(item, ensure_ascii=False, sort_keys=True, separators=(',', ':'))

    references = {}

    def index(item, pointer=''):
        key = encoded(item)
        reference = {'$draft': pointer}
        if len(encoded(reference)) < len(key):
            previous = references.get(key)
            if previous is None or len(pointer) < len(previous['$draft']):
                references[key] = reference
        children = item.items() if isinstance(item, dict) else enumerate(item) if isinstance(item, list) else ()
        for name, child in children:
            index(child, pointer + '/' + str(name).replace('~', '~0').replace('/', '~1'))

    def render(item):
        if encoded(item) in references:
            return references[encoded(item)]
        if isinstance(item, dict):
            if set(item) in ({'$draft'}, {'$literal'}):
                return {'$literal': item}
            return {key: render(child) for key, child in item.items()}
        if isinstance(item, list):
            return [render(child) for child in item]
        return item

    index(draft)
    return render(value)


def expand_draft_references(value, draft):
    if isinstance(value, dict):
        if set(value) == {'$literal'}:
            return deepcopy(value['$literal'])
        if set(value) == {'$draft'}:
            return deepcopy(at(draft, value['$draft']))
        return {key: expand_draft_references(child, draft) for key, child in value.items()}
    if isinstance(value, list):
        return [expand_draft_references(child, draft) for child in value]
    return value


@dataclass(frozen=True, eq=False)
class DraftAllowed(Allowed):
    draft: dict

    def render(self):
        text = Allowed(self.keyword, draft_references(self.value, self.draft)).render()
        if '"$draft"' in text:
            text += ' $draft names a JSON pointer in the original draft.'
        if '"$literal"' in text:
            text += ' $literal is verbatim JSON.'
        return text


@dataclass(frozen=True, eq=False)
class CoverageAllowed(DraftAllowed):
    changes: dict
    appended: dict
    maximum: int

    def render(self):
        description = {'edits': self.changes, 'append_checks': {
            'maximum_total': self.maximum, 'items': self.appended}}
        return (DraftAllowed('schema', description, self.draft).render()
                + ' Apply edits to the original draft; preserve all other fields and existing check order.'
                + ' Appended checks must satisfy items; optional proof/forcing must copy an original value.')


@dataclass(frozen=True, eq=False)
class SplitAllowed(DraftAllowed):
    def render(self):
        properties = self.value['properties']
        piece = properties['pieces']['items']['properties']
        values = {'reason': properties['reason']['enum'], 'files': piece['files']['items']['enum'],
                  'goal_ids': piece['goal_ids']['items']['enum']}
        remove = [key for key in self.draft if key not in properties]
        return (DraftAllowed('schema', values, self.draft).render()
                + ' Return split with >=2 reply-schema pieces using only these files/goal_ids.'
                + " Cover each file; keep each original check's goals and test/proof paths together in one piece."
                + ' Freeze other root fields; remove ' + json.dumps(remove, separators=(',', ':')) + '.')


def coverage_allowed(draft, changes, appended, maximum, *, rendered=False):
    """Expand the rendered changes against the captured draft for exact membership."""
    if rendered:
        changes = expand_draft_references(changes, draft)
        appended = expand_draft_references(appended, draft)

    def bounded(value, pointer=''):
        if pointer in changes:
            return changes[pointer]
        if pointer != '/checks' and not any(path.startswith(pointer + '/') for path in changes):
            return {'const': value}
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
    constraint['properties']['checks'].update({'maxItems': maximum, 'items': appended})
    return CoverageAllowed('schema', constraint, draft, changes, appended, maximum)


def wire(diagnostic):
    if not isinstance(diagnostic, Diagnostic):
        raise TypeError('Unnamed plan-check diagnostic')
    return {'class': inventory()[diagnostic.rule]['class'], 'rule': diagnostic.rule,
            'items': identities(diagnostic.items), 'diagnostic': str(diagnostic)}


def identities(items):
    result = []
    for key, value in sorted(items.items()):
        if key in {'description', 'descriptions', 'source_goals'}:
            if key == 'source_goals':
                result.extend(f'goal {i}' for i in range(len(value)))
            continue
        if key == 'check':
            result.append(f'check-{value + 1}' if type(value) is int else str(value))
        elif key == 'obligation':
            result.extend(identities({k: v for k, v in value.items() if k != 'quote'}))
        elif key == 'goal':
            result.append(f'goal {value}:')
        elif key == 'goals':
            result.extend(f'goal {goal}:' for goal in value)
        else:
            result.append(key + '=' + json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(',', ':')))
    return result or ['plan']


def terminal(origin, latest, completed, candidate, *, attempted, unavailable, malformed=False):
    """Preserve the rejection when correction ends, including malformed corrections."""
    reply = deepcopy(completed if completed is not None else origin.completed_reply)
    remaining = list(latest.objections) if latest is not None else []
    if not remaining and reply['kind'] == 'split':
        remaining = list(origin.objections)
    guards = []
    if attempted:
        if malformed:
            guards = [named('guard.variant', 'Planner correction failed reply validation.', {'field': 'reply'})]
        else:
            guards = origin.contract.check(candidate)
        if reply['kind'] == 'cant_meet':
            remaining = [*origin.objections, *remaining]
        remaining.extend(guards)
    if not remaining:
        return reply, None
    # Original and final diagnostics remain complete even when the prompt clips
    # optional descriptions. The wire's objections are the still-standing set.
    reply['violations'] = list(dict.fromkeys([
        *(str(item) for item in origin.objections), *reply.get('violations', []),
        *(str(item) for item in guards)]))
    if origin.missing_goals or latest is not None and latest.missing_goals:
        reply['violations'] = [item for item in reply['violations']
                               if not item.startswith('Planner coverage correction attempts:')]
        reply['violations'].append(f'Planner coverage correction attempts: {int(attempted)}.')
    if attempted and hasattr(origin.contract, 'manifest'):
        remaining = [named(item.rule, str(item), {**item.items, 'obligation_manifest': origin.contract.manifest}) for item in remaining]
    if not attempted and unavailable == 'message_limit':
        reply['violations'].append('Planner declarations exceed the existing correction message limit.')
    if not attempted and unavailable == 'input_allowance':
        reply['violations'].append('Planner correction unavailable: the existing input allowance has no room for a correction turn.')
    evidence = {'first_objections': [wire(item) for item in origin.objections],
                'final_objections': [wire(item) for item in latest.objections] if latest is not None and not malformed else [],
                'constraint_failures': [wire(item) for item in guards],
                'attempts': int(attempted), 'unavailable_reason': None if attempted else unavailable,
                'binding': {'base_sha': origin.contract.inputs.get('base_sha')}}
    reply['violations'].append('Plan-check evidence: ' + json.dumps(evidence, ensure_ascii=False, sort_keys=True))
    repair = {'status': 'attempted'} if attempted else {'status': 'unavailable', 'reason': unavailable}
    provenance = {'objections': list({json.dumps(wire(item), sort_keys=True): wire(item)
                                     for item in remaining}.values()), 'repair': repair}
    return reply, provenance


# Fresh proposals require estimates; the transport still accepts workers that
# started before a deploy (#372). Use the same requirement in the model prompt.
EXPLORATION_DECLARATION = {
    "if": {"properties": {"kind": {"enum": ["leaf", "split"]}}, "required": ["kind"]},
    "then": {"required": ["estimated_exploration_turns"],
             "properties": {"pieces": {"items": {"required": ["estimated_exploration_turns"]}}}},
}


def execution_budget_details(execution):
    components = ", ".join(f"{name}={value}" for name, value in execution["components"].items())
    return (f"estimated {execution['estimated_tool_turns']} tool turns, "
            f"backstop {execution['allowed_tool_turns']}. "
            f"Components: {components}.")


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


def ordered_constraint(before, additions=(), removable=()):
    additions = [value for value in unique(additions) if not MembershipAllowed('enum', before).accepts(value)]
    if not additions and not removable:
        return {'const': before}
    return {'x-ordered': {'before': before, 'add': additions, 'remove': list(removable)}}


def append_constraint(before, additions):
    return ordered_constraint(before, additions)


def edit(facts, pointer, allowed):
    try:
        at(facts['draft'], pointer)
        op = 'replace'
    except (KeyError, IndexError):
        op = 'add'
    return Edit(Name('field', pointer), op, allowed)


def cannot_repair(facts):
    return [CantMeet('No bounded declaration edit is supported by the captured facts; return cant_meet.')]


def code(f):
    keys = f['items'].get('keys', [f['items'].get('field')])
    return [Edit(Name('field', '/' + key), 'remove', fixed(None)) for key in keys if key in f['draft']] or cannot_repair(f)


def evidence_empty(f):
    return [edit(f, '/evidence_contract', fixed([]))] if 'evidence_contract' not in f['draft'] else cannot_repair(f)


def proof_fill(f):
    index = f['items'].get('check')
    if type(index) is not int or index >= len(f['draft'].get('checks', [])):
        return cannot_repair(f)
    check = f['draft']['checks'][index]
    proof = check.get('proof', {})
    if not isinstance(proof, dict):
        return cannot_repair(f)
    missing = [key for key in ('subjects', 'observer_support', 'red_message') if key not in proof]
    quotes = unique(goal['proof_obligation']['quote'] for goal in f['inputs']['ingest']['goals']
                    if 'proof_obligation' in goal)
    if 'red_message' in missing and not quotes:
        return [CantMeet('No ingest proof_obligation quote permits red_message; return cant_meet without inventing an assertion.')]
    changes = {key: {'enum': quotes} if key == 'red_message' else {'const': []} for key in missing}
    local = [item for item in f['diagnostics'] if item.items.get('check') == index]
    if any(inventory()[item.rule]['footprint'] == 'PROOF_DEDUP' for item in local):
        changes.update({key: {'const': unique(value)} for key, value in proof.items()
                        if key in {'subjects', 'observer_support'} and unique(value) != value})
    red = unique(item.items['obligation']['quote'] for item in local
                 if inventory()[item.rule]['footprint'] == 'RED_FILL' and item.items.get('obligation', {}).get('quote'))
    if red and not proof.get('red_message', '').strip():
        changes['red_message'] = {'enum': red}
    subject_rows = [item for item in f['diagnostics'] if item.items.get('check') == index
                    and inventory()[item.rule]['footprint'] == 'SUBJECT_ADD']
    if subject_rows:
        subject_repair = subjects(f)[0]
        if isinstance(subject_repair, CantMeet):
            return [subject_repair]
        changes['subjects'] = subject_repair.allowed.value
    if not changes:
        return cannot_repair(f)
    return [edit(f, f'/checks/{index}/proof', Allowed('schema', object_constraint(proof, changes)))]


def red_fill(f):
    index = f['items'].get('check')
    quote = f['items'].get('obligation', {}).get('quote')
    if type(index) is not int or not quote or f['draft']['checks'][index].get('proof', {}).get('red_message', '').strip():
        return cannot_repair(f)
    return [edit(f, f'/checks/{index}/proof/red_message', Allowed('enum', [quote]))]


def subjects(f):
    index = f['items'].get('check')
    if type(index) is not int:
        return cannot_repair(f)
    before = f['draft']['checks'][index].get('proof', {}).get('subjects', [])
    if any(item.items.get('check') == index and inventory()[item.rule]['footprint'] == 'PROOF_DEDUP'
           for item in f['diagnostics']):
        before = unique(before)
    required = [unit for item in f['diagnostics'] if item.items.get('check') == index
                and inventory()[item.rule]['footprint'] == 'SUBJECT_ADD'
                for unit in item.items.get('subjects', item.items.get('obligation', {}).get('subjects', []))]
    additions = []
    for unit in required:
        path, _, symbol = unit.partition('::')
        if path in f['draft'].get('files', []):
            additions.extend([unit, path])
            if symbol:
                additions.extend(path + '::' + '::'.join(symbol.split('::')[:n]) for n in range(1, len(symbol.split('::'))))
    if not additions:
        return cannot_repair(f)
    constraint = {'type': 'array', 'prefixItems': [{'const': value} for value in before],
                  'minItems': len(before), 'items': {'enum': unique(additions)}, 'uniqueItems': True}
    return [edit(f, f'/checks/{index}/proof/subjects', Allowed('schema', constraint))]


def selectors(f):
    index = f['items'].get('check')
    if type(index) is not int:
        return cannot_repair(f)
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
            return cannot_repair(f)
        changes.append(edit(f, f'/checks/{index}/node_ids/{slot}', Allowed('enum', choices)))
    return changes or cannot_repair(f)


def files(f):
    before = f['draft'].get('files')
    if before is None:
        return cannot_repair(f)
    source_units = [unit for goal in f['inputs']['ingest']['goals']
                    for unit in goal.get('proof_obligation', {}).get('subjects', [])]
    additions = []
    replacements = {}
    for diagnostic in f['diagnostics']:
        operation, item = inventory()[diagnostic.rule]['footprint'], diagnostic.items
        if operation == 'FILES_FILL' and not before:
            additions.extend(unit.split('::')[0] for unit in source_units)
        elif operation == 'PROOF_FILE' and any(subject_covers(item['unit'], unit) or subject_covers(unit, item['unit'])
                                               for unit in source_units):
            additions.append(item['path'])
        elif operation == 'EDIT_FILE' and item['node'] in f.get('base_nodes', ()):
            additions.append(item['path'])
        elif operation == 'FILE_PATH':
            path = item.get('path', '')
            canonical = str(PurePosixPath(path))
            if path and not path.startswith(('/', '-')) and '..' not in PurePosixPath(path).parts and '\\' not in path and canonical != path:
                replacements[path] = canonical
    canonical = [replacements.get(value, value) for value in before]
    if not additions and canonical == before:
        return cannot_repair(f)
    return [edit(f, '/files', Allowed('schema', append_constraint(canonical, additions)))]


def citations(f):
    goals = f['inputs']['ingest']['goals']
    operation, item, draft = f['footprint'], f['items'], f['draft']
    pieces = operation.startswith('PIECE_')
    field, identity = ('pieces', 'piece') if pieces else ('checks', 'check')
    values = draft.get(field, [])
    indexes = range(len(values)) if operation in {'COVERAGE', 'PIECE_COVER'} else [item.get(identity)]
    missing = sorted(set(range(len(goals))) - {g for value in values for g in value['goal_ids']})
    coverage = [g for diagnostic in f['diagnostics'] if diagnostic.rule == 'goal.coverage'
                for g in diagnostic.items['goals']]
    repairs = []
    for index in indexes:
        if type(index) is not int or index >= len(values):
            continue
        value = values[index]
        before = value['goal_ids']
        local = {inventory()[diagnostic.rule]['footprint'] for diagnostic in f['diagnostics']
                 if diagnostic.items.get(identity) == index}
        retained = before
        if local & {'GOAL_DROP', 'PIECE_DROP'}:
            retained = [g for g in retained if g in range(len(goals))]
        if 'GOAL_SANITIZE' in local:
            retained = unique(g for g in retained if type(g) is int and 0 <= g < len(goals))
        additions = list(coverage) if not pieces else []
        if pieces:
            candidates = missing if any(d.rule == 'split.goal_missing' for d in f['diagnostics']) else []
            if not before and 'PIECE_GOALS' in local:
                candidates = range(len(goals))
            additions = [g for g in candidates if goals[g]['description'] in value['brief']
                         and all(unit.split('::')[0] in value['files'] for unit in goals[g].get('proof_obligation', {}).get('subjects', []))]
        elif not before and 'GOAL_ADD' in local:
            proof = value.get('proof', {})
            additions.extend(g for g, goal in enumerate(goals) if 'proof_obligation' in goal
                             and proof.get('red_message') == goal['proof_obligation']['quote']
                             and all(any(subject_covers(unit, required) for unit in proof.get('subjects', []))
                                     for required in goal['proof_obligation']['subjects']))
        if retained != before or additions:
            repairs.append(edit(f, f'/{field}/{index}/goal_ids', Allowed('schema', append_constraint(retained, additions))))
    return repairs or cannot_repair(f)


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
        return cannot_repair(f)
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
    return [Edit(Name('field', ''), 'replace', SplitAllowed('schema', constraint, draft))]


def deduplicate(f):
    index = f['items'].get('check')
    if type(index) is not int:
        return cannot_repair(f)
    check = f['draft']['checks'][index]
    if f['footprint'] == 'NODE_DEDUP':
        return [edit(f, f'/checks/{index}/node_ids', fixed(unique(check['node_ids'])))]
    return proof_fill(f)


def drop_edits(f):
    nodes = unique(node for item in f['diagnostics'] if inventory()[item.rule]['footprint'] == 'EDIT_DROP'
                   for node in ([item.items['node']] if 'node' in item.items else item.items.get('nodes', [])))
    covered = {node for check in f['draft'].get('checks', []) if check['kind'] == 'demonstrating' for node in check['node_ids']}
    if not set(nodes) <= covered:
        return cannot_repair(f)
    return [edit(f, '/edits_tests', fixed([node for node in f['draft'].get('edits_tests', []) if node not in nodes]))]


def edit_coverage(f):
    draft = f['draft']
    nodes = unique(node for item in f['diagnostics'] if inventory()[item.rule]['footprint'] == 'EDIT_COVER'
                   for node in item.items.get('nodes', []))
    if not nodes:
        return cannot_repair(f)
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
            pointer = f'/checks/{index}/node_ids'
            slots = [changes.get(pointer + f'/{slot}', {'const': node}) for slot, node in enumerate(check['node_ids'])]
            if any(pointer + f'/{slot}' in changes for slot in range(len(slots))):
                changes[pointer] = {'type': 'array', 'prefixItems': slots, 'minItems': len(slots),
                                    'maxItems': len(slots) + len(nodes), 'items': {'enum': nodes}, 'uniqueItems': True}
            else:
                retained = changes.get(pointer, {}).get('const', check['node_ids'])
                changes[pointer] = append_constraint(retained, nodes)

    original_checks = draft.get('checks', [])
    goals = unique(g for check in original_checks for g in check['goal_ids'])
    new_check = {'type': 'object', 'properties': {
        'kind': {'const': 'demonstrating'},
        'node_ids': {'type': 'array', 'items': {'enum': nodes}, 'minItems': 1, 'uniqueItems': True},
        'goal_ids': {'type': 'array', 'items': {'enum': goals}, 'minItems': 1},
        'proof': {'enum': unique(check['proof'] for check in original_checks if 'proof' in check)},
        'forcing': {'enum': unique(check['forcing'] for check in original_checks if 'forcing' in check)}},
        'required': ['kind', 'node_ids', 'goal_ids'], 'additionalProperties': False}
    if 'edits_tests' in draft:
        changes['/edits_tests'] = ordered_constraint(draft['edits_tests'], removable=nodes)
    allowed = coverage_allowed(draft, changes, new_check, len(original_checks) + len(nodes))
    return [Edit(Name('field', ''), 'replace', allowed)]


def frozen_nodes(f):
    nodes = {item.items.get('node') for item in f['diagnostics'] if inventory()[item.rule]['footprint'] == 'FROZEN'}
    repairs = []
    for index, check in enumerate(f['draft'].get('checks', [])):
        remaining = [value for value in check['node_ids'] if value not in nodes]
        if check['kind'] in {'regression', 'behavior-preserving'} and remaining and remaining != check['node_ids']:
            repairs.append(edit(f, f'/checks/{index}/node_ids', fixed(remaining)))
    return repairs or cannot_repair(f)


def memory(f):
    draft = f['draft']
    local = [(inventory()[item.rule]['footprint'], item.items) for item in f['diagnostics']]
    if f['footprint'] == 'MEMORY_DROP':
        removed = {value for operation, item in local if operation == 'MEMORY_DROP' for value in item.get('ids', [])}
        return [edit(f, '/memory_ids', fixed([value for value in draft.get('memory_ids', []) if value not in removed]))]
    constraint = {'x-memory-flags': {
        'before': draft.get('memory_flags', []),
        'drop': unique(value for operation, item in local if operation == 'FLAG_DROP' for value in item.get('ids', [])),
        'claims': unique(item['flag'] for operation, item in local if operation == 'FLAG_CLAIM'),
        'passages': {item['id']: item['content'] for item in f['inputs'].get('memories', [])}}}
    return [edit(f, '/memory_flags', Allowed('schema', constraint))]


def sanitize(f):
    index = f['items'].get('check')
    if type(index) is not int:
        return cannot_repair(f)
    check = f['draft']['checks'][index]
    if f['footprint'] == 'SYMPTOM_FIELDS':
        field = 'declared_symptom'
        result = {key: value for key, value in check.get(field, {}).items() if key in {'node', 'exception', 'phase', 'repeat', 'parallel'}}
        if 'node' not in result and len(check['node_ids']) == 1:
            result['node'] = check['node_ids'][0]
    else:
        field = 'forcing'
        result = deepcopy(check.get(field, {}))
        local = {inventory()[item.rule]['footprint'] for item in f['diagnostics'] if item.items.get('check') == index}
        if 'FORCING_KEYS' in local:
            result = {key: value for key, value in result.items() if key in {'symptom', 'environment', 'patches'}}
        if 'PATCH_MODE' in local:
            for patch in result.get('patches', []):
                modes = patch.keys() & {'value', 'returns', 'raises', 'delay_seconds'}
                if len(modes) == 1:
                    for key in list(patch):
                        if key not in {'target', *modes}:
                            del patch[key]
    try:
        if field == 'declared_symptom':
            validate_symptom(result, check['node_ids'])
        else:
            Forcing.parse(result)
    except HarnessInputError:
        return cannot_repair(f)
    return [edit(f, f'/checks/{index}/{field}', fixed(result))]


def methods(f):
    before = f['draft'].get('evidence_contract', [])
    mutable = {'tool', 'executor', 'runner_features', 'artifacts', 'methods', 'protected_files'}
    repairs = []
    for index, requirement in enumerate(before):
        if requirement != f['items'].get('requirement') or requirement.get('provenance', {}).get('kind') != 'method':
            continue
        alternatives = [object_constraint(requirement, {
            key: {'const': cap[key]} if key in {'tool', 'executor'} else {'type': 'array', 'items': {'enum': cap[key]}}
            for key in mutable}) for cap in f['inputs'].get('tool_capabilities', [])]
        if alternatives:
            repairs.append(edit(f, f'/evidence_contract/{index}', Allowed('schema', {'anyOf': alternatives})))
    return repairs or cannot_repair(f)


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
                     'FORCING_TEXT', 'RECEIPT_SELECT', 'OBSERVER_SELECT', 'EDIT_SELECT'), cannot_repair),
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
        repairs = REPAIRS[context['footprint']](context) if item.items else cannot_repair(context)
        for repair in repairs:
            if isinstance(repair, Edit) and not isinstance(repair.allowed, DraftAllowed):
                repair = replace(repair, allowed=DraftAllowed(repair.allowed.keyword, repair.allowed.value, facts['draft']))
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


def row_names(f):
    names = [f['subject']]
    items = f['items']
    values = [*items.get('nodes', []), *items.get('subjects', []), items.get('node'), items.get('path')]
    if re.fullmatch(r'/checks/\d+/node_ids/\d+', f['subject'].value):
        values = [at(f['draft'], f['subject'].value)]
    for value in values:
        if not isinstance(value, str):
            continue
        path, separator, symbol = value.partition('::')
        try:
            name = Name('symbol', path + '::' + symbol.replace('::', '.')) if separator else Name('path', path)
        except ValueError:
            continue
        if name not in names:
            names.append(name)
    return tuple(names)


def make_row(rule_id):
    return Rule(rule_id, lambda f: check_row(rule_id, f), row_names, allowed, lambda f: f['repair'])


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


class PlanConsistencyError(ValueError):
    """A complete planner refusal, rendered and guarded by registered rows."""

    rule_role = 'planner'
    guard_after_check = True

    def __init__(self, raw, completed, diagnostics, inputs, *, base_nodes=(), base_sources=None):
        from tl3_workers.rules import render_all

        self.raw_draft = deepcopy(raw)
        self.completed_reply = deepcopy(completed)
        self.objections = tuple(diagnostics)
        self.inputs = deepcopy(inputs)
        self.missing_goals = {g: inputs['ingest']['goals'][g]['description']
                              for item in diagnostics if item.rule == 'goal.coverage' for g in item.items['goals']}
        self.node_ids = tuple(identity for item in diagnostics for identity in identities(item.items))
        self.rule_violations = violations({
            'reply': raw, 'draft': raw, 'completed': completed, 'diagnostics': diagnostics,
            'inputs': inputs, 'base_nodes': base_nodes, 'base_sources': base_sources or {}})
        self.repair_text = render_all(self.rule_violations, role='planner')
        self.unavailable_reason = ('disabled' if not inputs.get('repair_plan_consistency', False) else
                                   'message_limit' if len(self.repair_text.encode()) > REPAIR_MESSAGE_BYTES else None)
        self.contract = self
        super().__init__(self.repair_text)

    def check(self, candidate):
        from tl3_workers.rules import PLANNER, permits

        # An honest terminal reply is available after any bounded edit. CantMeet
        # rows select that same registered terminal predicate in permits.
        if PLANNER.predicate(candidate):
            original = self.raw_draft
            paths = original.get('files', [path for piece in original.get('pieces', []) for path in piece['files']])
            if set(candidate.get('files', [])) - set(paths):
                return [named('guard.fields_changed', 'Terminal correction adds files outside the draft footprint.', {'paths': ['/files']})]
            return []
        if permits(self.rule_violations, self.raw_draft, candidate, role='planner'):
            if candidate.get('kind') == 'split' and self.raw_draft.get('kind') == 'leaf':
                original = self.raw_draft
                self.manifest = {
                    'base': self.inputs.get('base_sha'), 'goals': deepcopy(self.inputs['ingest']['goals']),
                    'checks': deepcopy(original.get('checks', [])), 'original_pieces': [],
                    'edits_tests': deepcopy(original.get('edits_tests', [])), 'files': original['files'],
                    'evidence_contract': deepcopy(self.completed_reply.get('evidence_contract', [])),
                    'forecast': deepcopy(self.completed_reply['forecast']), 'execution': deepcopy(self.completed_reply['execution']),
                    'parent_estimates': {k: original[k] for k in ('estimated_tool_turns', 'estimated_exploration_turns') if k in original},
                    'mapping': [{'check': index, 'pieces': [i for i, piece in enumerate(candidate['pieces'])
                                 if set(check['goal_ids']) <= set(piece['goal_ids']) and
                                 {node.split('::')[0] for node in check['node_ids']} <= set(piece['files'])]}
                                for index, check in enumerate(original.get('checks', []))]}
            return []
        changed = ['/' + key for key in sorted(self.raw_draft.keys() | candidate.keys())
                   if json.dumps(self.raw_draft.get(key), sort_keys=True) != json.dumps(candidate.get(key), sort_keys=True)
                   or (key in self.raw_draft) != (key in candidate)]
        # An unchanged rejected draft has no scope violation. Its semantic rows
        # still reject it in the full check and retain their original classes.
        return ([named('guard.fields_changed', 'Correction changes declarations outside the named objections: '
                       + ', '.join(changed), {'paths': changed})] if changed else [])

    def constrain(self, reply, raw=None):
        candidate = raw if raw is not None else {key: value for key, value in reply.items()
            if key not in ROLE.code_only_fields}
        errors = self.check(candidate)
        reply['violations'].extend(str(item) for item in errors)
        return errors


def proof_omissions(reply):
    """The proof schema's required keys are the only declaration omissions admitted here."""
    if not isinstance(reply, dict) or reply.get('kind') != 'leaf' or not isinstance(reply.get('checks'), list):
        return {}
    required = set(PROOF_DECLARATION)
    missing = {}
    for index, check in enumerate(reply['checks']):
        if (not isinstance(check, dict) or check.get('kind') not in {'demonstrating', 'reproducing'}
                or 'declared_symptom' in check or not isinstance(check.get('proof', {}), dict)):
            continue
        absent = required - check.get('proof', {}).keys()
        if absent:
            missing[index] = absent
    return missing


# A validation-only placeholder never enters an accepted plan or a proof run.
PROOF_DECLARATION = Proof((), (), 'undeclared assertion message').as_json()


def proof_for_validation(check):
    proof = check.get('proof', {})
    red = proof.get('red_message', '')
    return {**PROOF_DECLARATION, **proof, **({'red_message': PROOF_DECLARATION['red_message']}
        if isinstance(red, str) and not red.strip() else {})}


def planner_schema_reply(reply):
    """Keep proof omissions and blank messages inside the semantic repair budget."""
    missing = proof_omissions(reply)
    if not isinstance(reply, dict) or reply.get('kind') != 'leaf':
        return reply
    result = deepcopy(reply)
    for index, check in enumerate(result.get('checks', [])):
        if index in missing:
            check['proof'] = proof_for_validation(check)
        if isinstance(check.get('proof'), dict) and isinstance(check['proof'].get('red_message'), str) and not check['proof']['red_message'].strip():
            check['proof']['red_message'] = PROOF_DECLARATION['red_message']
    return result


def check_reply(inputs, reply, *, rehearsals=None, plan_errors=None):
    if rehearsals is not None:
        rehearsals.raise_infrastructure()
    Draft202012Validator(EXPLORATION_DECLARATION).validate(reply)
    raw_draft = deepcopy(reply)
    early = []
    supplied = [key for key in ('forecast', 'violations', 'decision_context', 'forcing_rehearsals', 'evidence_errors') if key in reply]
    if supplied:
        early.append(named('plan.code_evidence', 'only code may supply planning evidence', {'keys': supplied}))
        for key in supplied:
            reply.pop(key)
    preflight(inputs, reply, planning=True, objections=early)
    check_citations(inputs, reply, objections=early)
    goals = set(range(len(inputs["ingest"]["goals"])))
    violations = ([named('plan.execution_owned', "Execution forecasts are supplied by worker code; omit execution from the model reply.", dict(field="execution"))]
                  if reply.pop("execution", None) is not None else [])
    violations[:0] = early
    uncovered = []
    missing_forced_checks = {}
    selectors = {}
    selector_errors = set()
    missing_proof = proof_omissions(reply)
    missing_subjects = {}
    missing_goals = {}
    paths = set()
    base_nodes = set()
    base_sources = {}
    def observed_base(node):
        definition = base_definition(node, inputs.get('base_sha'))
        if getattr(definition, 'source', None) is not None:
            base_sources[node.split('::')[0]] = definition.source
        if definition[1]:
            base_nodes.add(node)
        return definition
    if reply["kind"] == "leaf":
        if inputs["must_split"]:
            violations.append(named('leaf.must_split', "Must split after running out of room.", dict(must_split=inputs["must_split"])))
        if not reply["files"]:
            violations.append(named('leaf.files_empty', "A leaf must declare its files.", dict(files=reply["files"], source_goals=inputs["ingest"]["goals"])))
        if not reply["no_design_questions"]:
            violations.append(named('leaf.design_open', "A leaf must resolve every design question.", dict(no_design_questions=reply["no_design_questions"], source_goals=inputs["ingest"]["goals"])))
        if not reply["lands_on_its_own"]:
            violations.append(named('leaf.not_independent', "A leaf must land on its own.", dict(lands_on_its_own=reply["lands_on_its_own"], source_goals=inputs["ingest"]["goals"])))
        for index, check in enumerate(reply["checks"]):
            if 'proof' in check or index in missing_proof:
                if isinstance(check.get('proof'), dict) and 'red_message' in check['proof'] and not check['proof']['red_message'].strip():
                    obligation = next((inputs['ingest']['goals'][g]['proof_obligation']
                        for g in check['goal_ids'] if type(g) is int and 0 <= g < len(inputs['ingest']['goals'])
                        and 'proof_obligation' in inputs['ingest']['goals'][g]), None)
                    if obligation is None:
                        violations.append(named('proof.red_missing', f'check-{index + 1} requires proof.red_message binding the deterministic assertion.', dict(check=index, field='proof.red_message', obligation={})))
                try:
                    proof = Proof.parse(proof_for_validation(check) if index in missing_proof or not check.get('proof', {}).get('red_message', '').strip() else check['proof'])
                    for unit in (*proof.subjects, *proof.observer_support):
                        if unit.split('::')[0] not in reply['files']:
                            violations.append(named('proof.file_missing', f'Proof source must be in declared files: {unit}', dict(check=index, unit=unit, path=unit.split('::')[0])))
                except HarnessInputError as error:
                    violations.append(contextual(error, check=index, declaration=check))
        covered = set()
        forcings = {}
        exact_nodes = set()
        for index, check in enumerate(reply["checks"]):
            if "forcing" in check:
                # #393: a forced reproduction is the red for a deflake.
                if check["kind"] != "demonstrating":
                    violations.append(named('check.forcing_kind', "Only a demonstrating check can carry a forcing.", dict(check=index, kind=check["kind"], forcing=check["forcing"])))
                try:
                    forcings[index] = Forcing.parse(check["forcing"], check_index=index, role='planner')
                except HarnessInputError as error:
                    cause = contextual(error, check=index, declaration=check["forcing"])
                    violations.append(named(cause.rule, f"Invalid forcing: {error}", cause.items))
            if not check["node_ids"]:
                violations.append(named('check.tests_empty', "Every check must name at least one test.", dict(check=index, goal_ids=check["goal_ids"])))
            if not check["goal_ids"]:
                violations.append(named('check.goals_empty', "Every check must cite at least one goal.", dict(check=index, nodes=check["node_ids"])))
            for node_id in check["node_ids"]:
                try:
                    Check("planned", Kind(check["kind"]), (node_id,))
                except HarnessInputError as error:
                    selector_errors.add(str(error))
                    selectors.setdefault(index, []).append(node_id)
                    continue
                exact_nodes.add(node_id)
                # A forced check runs the old test on the base, so it must exist there too.
                if check["kind"] in {"regression", "behavior-preserving"} or ("forcing" in check and not check.get("proof", {}).get("subjects")) or "declared_symptom" in check:
                    path = node_id.split("::")[0]
                    definition = observed_base(node_id)
                    oversized, exists = definition
                    if oversized:
                        violations.append(named('check.base_file_large', f"Existing check's test file is over the {HARD_CAP_BYTES}-byte limit: {path}", dict(check=index, node=node_id, path=path, bytes=getattr(definition, "size", None), limit=HARD_CAP_BYTES)))
                    elif not exists:
                        if check["kind"] == "demonstrating" and "forcing" in check:
                            missing_forced_checks.setdefault(index, []).append(node_id)
                        else:
                            guidance = (' A new assertion observer needs a demonstrating check; adding it only '
                                        'at head cannot repair an original-base regression.'
                                        if check['kind'] in {'regression', 'behavior-preserving'} else '')
                            violations.append(named('check.base_missing', f"Existing check must name a test defined at the base: {node_id}.{guidance}", dict(check=index, kind=check["kind"], node=node_id, base=inputs.get("base_sha"))))
            cited = set(check["goal_ids"])
            if not cited <= goals:
                violations.append(named('check.goal_unknown', "A check cites an unknown goal.", dict(check=index, unknown=sorted(cited-goals))))
            covered.update(cited)
        applicable = [(index, check) for index, check in enumerate(reply['checks'])
                      if set(check['node_ids']) <= exact_nodes]
        omissions = {}
        applicability_errors = proof_applicability(inputs['ingest']['goals'], [check for _, check in applicable],
            lambda node: observed_base(node)[1], missing_subjects=omissions)
        for error in applicability_errors:
            items = {**error.items, 'check': applicable[error.items['check']][0]}
            text = str(error).replace(f'check-{error.items["check"] + 1} ', f'check-{items["check"] + 1} ', 1) if error.rule == 'proof.red_missing' else str(error)
            violations.append(named(error.rule, text, items))
        missing_subjects = {applicable[index][0]: subjects for index, subjects in omissions.items()}
        edit_errors, uncovered = edit_violations(inputs["ingest"]["goals"], reply, inputs.get('base_sha'), observed=observed_base)
        violations.extend(edit_errors)
        planned_checks = []
        for i, check in enumerate(reply['checks']):
            if not check['node_ids']:
                continue
            try:
                planned_checks.append(Check(f'check-{i + 1}', Kind(check['kind']), tuple(check['node_ids']),
                                            forcings.get(i) if check['kind'] == 'demonstrating' else None,
                                            Proof.parse(proof_for_validation(check))
                                            if 'proof' in check or i in missing_proof else None,
                                            check.get('declared_symptom')))
            except ValueError as error:
                if str(error) not in selector_errors:
                    violations.append(contextual(error, check=i, declaration=check))
        try:
            validate_check_plan(tuple(planned_checks))
        except ValueError as error:
            violations.append(contextual(error, checks=reply['checks']))
        required = {i for i, goal in enumerate(inputs["ingest"]["goals"])
                    if goal.get("stage", "maker") != "publishing"}
        missing_goals = {index: inputs['ingest']['goals'][index]['description']
                         for index in sorted(required - covered)}
        paths.update(reply["files"])
    elif reply["kind"] == "split":
        covered = set()
        for piece_index, piece in enumerate(reply["pieces"]):
            if not piece["goal_ids"]:
                violations.append(named('split.goal_empty', "Every piece must cite at least one goal.", dict(piece=piece_index, source_goals=inputs["ingest"]["goals"])))
            cited = set(piece["goal_ids"])
            if not cited <= goals:
                violations.append(named('split.goal_unknown', "A piece cites an unknown goal.", dict(piece=piece_index, unknown=sorted(cited-goals))))
            covered.update(cited)
            paths.update(piece["files"])
        if len(reply["pieces"]) < 2:
            violations.append(named('split.too_few', "A split must propose at least two pieces.", dict(count=len(reply["pieces"]), minimum=2)))
        if covered != goals:
            violations.append(named('split.goal_missing', "A split must preserve every goal.", dict(missing=sorted(goals-covered), source_goals=inputs["ingest"]["goals"])))
    else:
        paths.update(reply["files"])

    if reply["kind"] in {"leaf", "split"}:
        for path in sorted(paths):
            if any(PurePosixPath(path).full_match(pattern) for pattern in inputs["protected_paths"]):
                violations.append(named('path.protected', f"Declared file matches a protected path: {path}", dict(path=path, patterns=inputs["protected_paths"])))

    if reply["kind"] in {"leaf", "split"} and inputs["forecast"].get("generator"):
        with Worktree(".") as tree:
            outputs = configured_outputs(tree, inputs["forecast"], inputs["protected_paths"])
        paths.update(outputs)
        if reply["kind"] == "leaf":
            reply["files"] = sorted(paths)
        else:
            for piece_index, piece in enumerate(reply["pieces"]):
                piece["files"] = sorted(set(piece["files"]) | set(outputs))

    forecaster = Forecaster(inputs["forecast"])
    forecast = forecaster.measure(inputs["ingest"]["brief"], paths)
    check_count = (len(reply["checks"]) if reply["kind"] == "leaf" else len(goals))
    execution = execution_forecast(paths, check_count, declared=reply.get("estimated_tool_turns", 0),
                                   exploration=reply.get("estimated_exploration_turns"),
                                   config=inputs["forecast"])
    if reply["kind"] == "leaf" and execution["exploration_declared"] and not execution["fits"]:
        violations.append(named('leaf.execution_fit', f"The leaf exceeds the execution budget: {execution_budget_details(execution)} "
                          "Preserve every obligation in bounded pieces with reason execution_budget before maker work.", dict(execution=execution)))
    # A file the tools can't use is a plan problem, not a crash (#321).
    for error in forecaster.problems:
        violations.append(named(error.rule, f"Declared file can't be used: {error}. Leave it out.", dict(path=error.path, reason=str(error.reason), bytes=error.size)))
    if reply["kind"] == "leaf" and not forecast["fits"]:
        violations.append(named('leaf.token_fit', "The leaf forecast does not fit. Propose smaller pieces.", dict(forecast=forecast)))
    if reply["kind"] == "split":
        reason = reply["reason"]
        if reason == "multiple_repos":
            violations.append(named('split.multiple_repos', "This request is in one repository.", dict(reason=reason)))
        if reason == "out_of_room" and not inputs["must_split"]:
            violations.append(named('split.no_out_of_room', "No maker has run out of room.", dict(reason=reason, must_split=inputs["must_split"])))
        if reason == "does_not_fit" and forecast["fits"]:
            violations.append(named('split.forecast_fits', "The forecast fits. Splitting needs a permitted reason.", dict(reason=reason, forecast=forecast)))
        if reason == "execution_budget":
            if execution["fits"]:
                violations.append(named('split.execution_fits', "The execution estimate fits; execution_budget is not a permitted reason for this plan.", dict(reason=reason, execution=execution)))
            if len(reply["pieces"]) > MAX_EXECUTION_PIECES:
                violations.append(named('split.too_many', f"Execution-budget decomposition permits at most {MAX_EXECUTION_PIECES} pieces.", dict(count=len(reply["pieces"]), maximum=MAX_EXECUTION_PIECES)))
        pieces = []
        for piece_index, piece in enumerate(reply["pieces"]):
            child = forecaster.measure(piece["brief"], piece["files"])
            estimate = execution_forecast(piece["files"], len(piece["goal_ids"]),
                                          declared=piece.get("estimated_tool_turns", 0),
                                          exploration=piece.get("estimated_exploration_turns"),
                                          config=inputs["forecast"])
            pieces.append(estimate)
            for error in forecaster.problems:
                violations.append(named(error.rule, f"Declared file can't be used: {error}. Leave it out.", dict(path=error.path, reason=str(error.reason), bytes=error.size)))
            if reason == "execution_budget":
                if not estimate["fits"] or estimate["estimated_tool_turns"] >= execution["estimated_tool_turns"]:
                    violations.append(named('split.execution_piece', "Every execution-budget piece must fit the execution backstop and reduce demand. "
                                      + execution_budget_details(estimate), dict(piece=piece_index, estimate=estimate, parent=execution)))
                if not child["fits"]:
                    violations.append(named('split.token_piece', "Every execution-budget piece must also fit the token forecast.", dict(piece=piece_index, forecast=child)))
            elif child["tokens"] >= forecast["tokens"]:
                violations.append(named('split.not_smaller', "Every proposed piece must have a smaller forecast than its parent.", dict(piece=piece_index, child=child, parent=forecast)))
        execution["pieces"] = pieces
        execution["aggregate_tool_turns"] = sum(piece["estimated_tool_turns"] for piece in pieces)
        execution["aggregate_allowed_tool_turns"] = sum(piece["allowed_tool_turns"] for piece in pieces)
    failed_work = False
    if rehearsals is not None and rehearsals.failure:
        failed_work = True
        violations.append(named(rehearsals.failure.get('rule', 'rehearsal.exhausted'), rehearsals.failure['message'], {'receipt': rehearsals.failure}))
    elif rehearsals is not None and reply['kind'] == 'cant_meet':
        rejected = next((item for item in reversed(rehearsals.candidates) if item['status'] == 'refused'), None)
        if rejected is not None:
            failed_work = True
            violations.append(named(rejected.get('rule', 'rehearsal.missing_receipt'), rejected['message'], {'receipt': rejected}))
    consistency_facts = (
        [('edit.uncovered', {'nodes': [node]}) for node in uncovered]
        + [('forced.base_missing', {'check': i, 'nodes': nodes}) for i, nodes in missing_forced_checks.items()]
        + [('check.selector_inexact', {'check': i, 'nodes': nodes}) for i, nodes in selectors.items()]
        + [('proof.omitted', {'check': i, 'fields': sorted(fields)}) for i, fields in missing_proof.items()]
        + [('proof.subject_missing', {'check': i, 'subjects': sorted(subjects)}) for i, subjects in missing_subjects.items()]
        + ([('goal.coverage', {'goals': sorted(missing_goals), 'descriptions': missing_goals})] if missing_goals else []))
    for rule_id, items in consistency_facts:
        text = rule_id + ': ' + '; '.join(identities(items))
        if rule_id == 'goal.coverage':
            text = 'Required goals have no check coverage: ' + '; '.join(
                f'goal {goal}: {description}' for goal, description in items['descriptions'].items())
        violations.append(named(rule_id, text, items))
    if reply['kind'] == 'leaf':
        valid = [check for check in reply['checks'] if valid_forcing(check)]
        if rehearsals is not None:
            violations.extend(contextual(text, checks=[i for i, check in enumerate(reply['checks']) if valid_forcing(check)])
                              for text in rehearsals.violations(inputs.get('base_sha'), valid))
        elif valid:
            violations.append(named('rehearsal.no_ledger_receipt', 'Q55 check lacks a matching forcing rehearsal receipt for the exact base, selectors and forcing.', dict(checks=[i for i, check in enumerate(reply["checks"]) if valid_forcing(check)], base=inputs.get("base_sha"))))
    if rehearsals is not None and (rehearsals.candidates or rehearsals.failure):
        reply['forcing_rehearsals'] = deepcopy(rehearsals.candidates + ([rehearsals.failure] if rehearsals.failure else []))
    if "decision_context" in inputs:
        reply["decision_context"] = inputs["decision_context"]
    reply["forecast"] = forecast
    reply["execution"] = execution
    reply["violations"] = list(dict.fromkeys(violations))
    if reply.get('evidence_errors'):
        planning_refusal(reply, reply['evidence_errors'])
    if violations:
        # Record before deduplicating human text; identical messages can name
        # different checks, pieces or fields.
        repair_error = PlanConsistencyError(raw_draft, reply, violations, inputs,
                                            base_nodes=base_nodes, base_sources=base_sources)
        if failed_work:
            repair_error.unavailable_reason = 'failed_work'
        if missing_goals:
            reply['violations'].append('Planner coverage correction attempts: 0.')
            repair_error.completed_reply = deepcopy(reply)
        if repair_error.unavailable_reason is None:
            raise repair_error
        if plan_errors is not None:
            plan_errors.append(repair_error)
    return None


class BaseDefinition(tuple):
    def __new__(cls, oversized, exists, size=None, source=None):
        result = super().__new__(cls, (oversized, exists))
        result.size = size
        result.source = source
        return result

    def __reduce__(self):
        return BaseDefinition, (*self, self.size, self.source)


def base_definition(node_id, base_sha=None):
    """Whether the base defines the test, after whether its file is over the hard cap."""
    # The whole module is parsed to find the test, so the
    # declared-file hard cap bounds it, not a page size.
    size = None
    try:
        if base_sha is not None:
            identity = base_sha + ':' + node_id.split('::')[0]
            size = int(git(Path('.'), 'cat-file', '-s', identity))
            if size > HARD_CAP_BYTES:
                return BaseDefinition(True, False, size)
            source = git(Path('.'), 'cat-file', 'blob', identity).decode('utf-8')
        else:
            with Worktree(".") as tree, tree._file(node_id.split("::")[0]) as stream:
                source = stream.read(HARD_CAP_BYTES + 1)
    except (OSError, ValueError, UnicodeError, CalledProcessError):
        return BaseDefinition(False, False, size)
    size = len(source.encode("utf-8"))
    if size > HARD_CAP_BYTES:
        return BaseDefinition(True, False, size)
    return BaseDefinition(False, test_function_source(source, node_id) is not None, size, source)


EDIT_VERBS = frozenset(("extend", "extends", "extending", "patch", "patches", "patching", "update", "updates",
                        "updating", "modify", "modifies", "modifying", "rewrite", "rewrites", "rewriting",
                        "change", "changes", "changing", "edit", "edits", "editing", "fix", "fixes", "fixing",
                        "replace", "replaces", "replacing"))
ADD_VERBS = frozenset(("add", "adds", "adding"))
HOLDS = frozenset(("not", "never", "don't", "don’t", "dont", "nothing", "without", "unchanged", "keep",
                   "keeps", "keeping", "leave", "leaves", "leaving", "preserve", "preserves", "preserving"))
CLAUSES = frozenset(("so", "that", "until", "while", "and", "because", "when", "then", "but", "or", "if"))
WORD = re.compile(r"[a-z][\w'’-]*")


def mentions(text, test):
    """Each mention of a test's name, as the words before and after it in its sentence.
    A path such as `tests/<name>.py` isn't a mention."""
    for sentence in re.split(r"[.;!?](?:\s|$)|\n", text):
        for match in re.finditer(rf"(?<![\w/]){re.escape(test)}(?!\w|\.py)", sentence):
            yield WORD.findall(sentence[:match.start()].lower()), WORD.findall(sentence[match.end():].lower())


def asks_to_edit(before, after):
    """An edit verb within seven words before the name, with no negation or new clause between them,
    none just before the verb, and no "unchanged" just after the name. "Add" counts only as "add ... to"."""
    for index in range(len(before) - 1, max(len(before) - 8, -1), -1):
        word = before[index]
        if word in HOLDS or word in CLAUSES:
            return False
        if word in EDIT_VERBS or word in ADD_VERBS and before[-1] in ("to", "into"):
            return not HOLDS.intersection(before[max(index - 3, 0):index]) and "unchanged" not in after[:3]
    return False


def valid_forcing(check):
    try:
        return "forcing" in check and Forcing.parse(check["forcing"]) is not None
    except HarnessInputError:
        return False


def function(node_id):
    # The harness's edit guard compares a test's function source, so every case shares it.
    return node_id.split("[", 1)[0]


def edit_violations(goals, reply, base_sha=None, *, observed=None):
    """Regression and behavior-preserving checks run tests exactly as the base wrote them,
    so a test the change edits can only be shown by a demonstrating check (#438).
    Return substantive violations separately from uncovered edit declarations."""
    violations = []
    uncovered = []
    frozen, demonstrated = {}, set()
    for index, check in enumerate(reply["checks"]):
        for node_id in check["node_ids"]:
            if check["kind"] == "demonstrating":
                demonstrated.add(function(node_id))
            else:
                frozen.setdefault(function(node_id), (f"check-{index + 1}", check["kind"], node_id))
    subjects = [unit for check in reply['checks'] if check['kind'] == 'demonstrating'
                for unit in check.get('proof', {}).get('subjects', [])]
    edits = reply.get("edits_tests", [])
    edited = set()
    for node_id in edits:
        try:
            Check("edits", Kind.DEMONSTRATING, (node_id,))
        except ValueError:
            violations.append(named('edit.selector', "edits_tests must name exact pytest node IDs.", dict(node=node_id)))
            continue
        edited.add(function(node_id))
        path = node_id.split("::")[0]
        if path not in reply["files"]:
            violations.append(named('edit.file_missing', f"A test the change edits must be in a declared file: `{path}`.", dict(node=node_id, path=path)))
        definition = observed(node_id) if observed is not None else base_definition(node_id, base_sha)
        oversized, exists = definition
        if oversized:
            violations.append(named('edit.file_large', f"edits_tests names a test in a file over the {HARD_CAP_BYTES}-byte limit: {path}", dict(node=node_id, path=path, bytes=getattr(definition, "size", None), limit=HARD_CAP_BYTES)))
        elif not exists:
            violations.append(named('edit.base_missing', f"edits_tests must name a test defined at the base: `{node_id}`. "
                              "A test the change adds belongs only in a demonstrating check.", dict(node=node_id)))
        if function(node_id) in frozen:
            violations.append(named('edit.frozen', f"`{node_id}` is in edits_tests, so a regression or behavior-preserving check "
                              "can't name it: those checks run tests exactly as the base wrote them. "
                              "Name it only in a demonstrating check.", dict(node=node_id, frozen=frozen[function(node_id)])))
        if function(node_id) not in demonstrated and not any(overlaps(function(node_id), unit) for unit in subjects):
            uncovered.append(node_id)
    for goal_id, goal in enumerate(goals):
        for key, (name, kind, node_id) in frozen.items():
            if key in edited or node_id.split("::")[0] not in reply["files"]:
                continue
            test = key.rsplit("::", 1)[-1]
            found = list(mentions(goal["description"], test))
            # A heuristic: an edit verb governing the name says the goal edits the test.
            if any(asks_to_edit(before, after) for before, after in found):
                violations.append(named('edit.goal_frozen', f"Goal {goal_id} names `{test}`, and {name} freezes `{node_id}` as a {kind} "
                                  "check. If the change edits that test, list it in edits_tests and name it only "
                                  "in a demonstrating check. If it leaves the test as written there, guard that "
                                  "behavior with other existing tests.", dict(goal=goal_id, description=goal["description"], check=name, node=node_id)))
    return violations, uncovered


class PlannerContract:
    """The planner's one model-loop contract hook; owns #832 provenance."""

    def __init__(self, inputs, worktree=None):
        self.inputs, self.worktree = inputs, worktree
        self.provenance = None
        self.origin = self.latest = self.completed = self.candidate = None
        self.calls = 0
        self.origin_call = None

    def check(self, reply, *, plan_errors=None):
        from tl3_workers.contracts import validate
        validate('planner-reply', planner_schema_reply(reply))
        check_reply(self.inputs, reply, rehearsals=getattr(self.worktree, 'forcing_rehearsals', None),
                    plan_errors=plan_errors)
        return reply

    def run(self, control, history, ask, repair, check):
        from jsonschema import ValidationError
        from tl3_workers.contracts import read_reply

        ledger = getattr(self.worktree, 'forcing_rehearsals', None)

        def checked(text):
            self.calls += 1
            if ledger is not None:
                ledger.raise_infrastructure()
                if ledger.failure:
                    return refused(ledger.failure)
            captured = []
            try:
                self.candidate = read_reply(text)
                result = check(deepcopy(self.candidate), plan_errors=captured)
            except Unrepairable:
                raise
            except (ValueError, ValidationError) as error:
                if ledger is not None:
                    rejected = next((item for item in reversed(ledger.candidates) if item['status'] == 'refused'), None)
                    if rejected is not None:
                        return refused(rejected)
                if isinstance(error, PlanConsistencyError):
                    capture(error)
                    self.completed = error.completed_reply
                raise
            if captured:
                capture(captured[-1])
            else:
                self.latest = None
            self.completed = result
            return result

        def capture(error):
            self.latest = error
            if self.origin is None:
                self.origin, self.origin_call = error, self.calls

        def refused(failure):
            self.candidate = {'kind': 'cant_meet', 'files': [], 'evidence_contract': [], 'report': failure['message']}
            captured = []
            self.completed = check(deepcopy(self.candidate), plan_errors=captured)
            capture(captured[-1])
            return self.completed

        failed = None
        try:
            reply = repair(control, history, ask, checked)
        except (PlanConsistencyError, InvalidReply) as error:
            if self.origin is None:
                raise
            failed, reply = error, self.completed
        if self.origin is not None:
            attempted = self.origin_call == 1 and control.repair_attempts == 1
            malformed = failed is not None and isinstance(failed, InvalidReply) and not isinstance(failed.errors[-1], PlanConsistencyError)
            guards = self.origin.check(self.candidate) if attempted and not malformed else []
            if self.latest is not None or guards or malformed or reply['kind'] in {'cant_meet', 'split'}:
                unavailable = self.origin.unavailable_reason or control.repair_unavailable or 'format_allowance_consumed'
                reply, self.provenance = terminal(self.origin, self.latest, reply, self.candidate,
                    attempted=attempted, unavailable=unavailable, malformed=malformed)
        return reply


ROLE = Role(
    reply_contract=PlannerContract,
    name="planner",
    prompt="planner.md",
    input_schema="planner-input",
    reply_schema="planner-reply",
    code_only_fields=("forecast", "execution", "violations", "decision_context", "evidence_errors", "forcing_rehearsals"),
    tools={
        "rehearse_forcing": "Rehearse exact existing base tests with a Q55 forcing through PrismaBuild before returning a forced leaf. Completed unusable candidates allow a distinct bounded proposal; identical candidates are cached. Infrastructure exits to controller recovery. Exhaustion or no usable final receipt requires cant_meet. No commands or head execution.",
        "list": "List worktree directories to locate material for the plan.",
        "read": "Read worktree text to identify files and checks for the goals.",
        "glob": "Find worktree paths using relative glob patterns for planning. The path must be a directory.",
        "grep": "Find references in worktree text to check the plan. The pattern is an RE2 regular expression, with no backreferences or lookarounds, matched per line; set literal to true to search text as written. The path may be a file or a directory.",
    },
    check_reply=check_reply,
)
