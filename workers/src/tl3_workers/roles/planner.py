"""Report plan measurements and rule violations to the controller."""

from copy import deepcopy
from functools import cache
import json
from pathlib import Path, PurePosixPath
import re
from subprocess import CalledProcessError

from jsonschema import Draft202012Validator

from tl3_workers.roles.base import Role
from tl3_workers.evidence import preflight, proof_applicability, subject_covers, planning_refusal
from tl3_workers.forecast import Forecaster, HARD_CAP_BYTES, MAX_EXECUTION_PIECES, execution_forecast
from tl3_harness.model import Diagnostic, named, contextual
from tl3_harness import Check, Forcing, HarnessInputError, Kind, Proof
from tl3_harness.proof import overlaps
from tl3_harness.limits import validate_check_plan
from tl3_harness.trees import git, test_function_source
from tl3_workers.tools import Worktree
from tl3_workers.generated_scope import configured_outputs
from tl3_workers.recall import check_citations
from tl3_workers.turns import InvalidReply, REPAIR_MESSAGE_BYTES, Unrepairable


@cache
def rules():
    return json.loads((Path(__file__).parent.parent / 'plan_rules.json').read_text())


def same(left, right):
    # Python equality would allow True and 1, or 1.0 and 1, to exchange identities.
    return type(left) is type(right) and (
        left.keys() == right.keys() and all(same(left[k], right[k]) for k in left)
        if isinstance(left, dict) else
        len(left) == len(right) and all(same(a, b) for a, b in zip(left, right))
        if isinstance(left, (list, tuple)) else left == right)


def wire(diagnostic):
    if not isinstance(diagnostic, Diagnostic):
        raise TypeError('Unnamed plan-check diagnostic')
    return {'class': rules()[diagnostic.rule]['class'], 'rule': diagnostic.rule,
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


CONSTRAINTS = {
    'NONE': 'No bounded declaration edit is supported; return cant_meet.',
    'CODE': 'Delete only named code-owned keys.',
    'COVERAGE': 'Only add named uncovered goal ids to existing checks; retain citations in order.',
    'RED_FILL': 'Fill only blank proof.red_message with the exact supported assertion; freeze existing proof values.',
    'PROOF_FILL': 'Fill only absent proof keys; inferred subjects/observer_support must be []; freeze old values.',
    'SUBJECT_ADD': 'Append only named missing subjects or covering definitions; retain old subjects in order.',
    'EXACT_SELECT': 'Replace only named inexact selectors in the same declared files with exact definitions.',
    'BASE_SELECT': 'Replace only named absent selectors with exact base controls supporting the same goals and kind.',
    'FORCED_SELECT': 'Replace only named absent forced selectors; preserve forcing, symptom and receipt identity.',
    'EDIT_COVER': 'Named edits have no demonstrating check. Cover only those edits, retaining kinds, goals and forcing.',
    'SPLIT': 'Preserve every original obligation and estimate; report cant_meet if no lossless bounded split is supported.',
    'FILES_FILL': 'Fill empty files only with named source-required subject paths.',
    'GOAL_ADD': 'Fill empty citations only from an unchanged source-supported proof.',
    'GOAL_DROP': 'Remove only named unknown goal ids; retain valid citations in order.',
    'GOAL_SANITIZE': 'Remove only invalid or duplicate citations, retaining each valid first occurrence.',
    'PIECE_GOALS': 'Fill only empty citations supported by this unchanged piece brief and files.',
    'PIECE_DROP': 'Remove only unknown piece goal ids; retain valid citations in order.',
    'PIECE_COVER': 'Add only missing goal ids supported by each unchanged piece brief and files.',
    'PROOF_FILE': 'Add only the named source-required path of an existing proof unit; freeze proof values.',
    'EDIT_FILE': 'Add only the named file of an already-observed necessary base-test edit.',
    'EDIT_DROP': 'Drop only the named absent-base edit already covered by an unchanged demonstrating check.',
    'FROZEN': 'Remove only named edited nodes from frozen checks; retain kind and independent controls.',
    'EVIDENCE_EMPTY': 'Add only missing evidence_contract as []; inherited requirements remain code-owned.',
    'METHOD_REPLACE': 'Replace only named method choices with supplied capabilities; freeze source requirements.',
    'MEMORY_DROP': 'Remove only named unsupplied memory ids; retain all other citations.',
    'FLAG_DROP': 'Remove only named uncited flags.',
    'FLAG_CLAIM': 'Drop the named flag or replace only its claim with an exact supplied passage for the same memory.',
    'SYMPTOM_FIELDS': 'Remove extra symptom keys or fill node from the sole original selector; freeze execution identity.',
    'PATCH_MODE': 'Remove extra patch keys only when one original mode exists; freeze that mode and argument.',
    'FORCING_KEYS': 'Remove only extra top-level forcing keys; freeze all mutations and symptom.',
    'PROOF_DEDUP': 'Remove only identical repeated proof units; retain each first occurrence.',
    'NODE_DEDUP': 'Remove only duplicate selectors; retain each first occurrence.',
    'FILE_PATH': 'Replace only a named spelling with its unique confined relative path.',
    'SPLIT_REASON': 'Replace only the invalid split reason with an already-true permitted reason.',
}


def message(objections):
    ordered = sorted(objections, key=lambda o: (o.rule, identities(o.items)))
    prefix = 'Use only already-read facts. Freeze every unnamed declaration. Return cant_meet if needed.\n'
    fixed = []
    descriptions = []
    grouped = {}
    for item in ordered:
        key = (item.rule, json.dumps(identities(item.items), ensure_ascii=False))
        grouped.setdefault(key, item)
    for item in grouped.values():
        operation = rules()[item.rule]['footprint']
        if item.rule == 'goal.coverage':
            for goal in item.items['goals']:
                fixed.append(item.rule + f' goal {goal}: Only add this citation to existing checks; freeze all other fields.')
                descriptions.append(item.items['descriptions'][goal])
            continue
        fixed.append(item.rule + ' ' + '; '.join(identities(item.items)) + ': '
                     + CONSTRAINTS.get(operation, CONSTRAINTS['NONE']))
        descriptions.append(item.items.get('description', str(item)))
    def render(texts):
        return prefix + '\n'.join(line + (' ' + text if text else '') for line, text in zip(fixed, texts))
    full = render(descriptions)
    if len(full.encode()) <= REPAIR_MESSAGE_BYTES:
        return full
    mandatory = render([''] * len(fixed))
    if len(mandatory.encode()) > REPAIR_MESSAGE_BYTES:
        return None
    cap = (REPAIR_MESSAGE_BYTES - len(mandatory.encode())) // max(len(fixed), 1)
    def clip(text):
        if len(text.encode()) + 1 <= cap:
            return text
        marker = ' [...]'
        if cap <= len(marker.encode()) + 1:
            return ''
        return text.encode()[:cap-len(marker.encode())-1].decode(errors='ignore') + marker
    return render([clip(text) for text in descriptions])


def insertion(before, after, allowed):
    """Retained elements keep their identities and order; additions consume named identities."""
    remaining = list(before)
    extras = []
    for value in after:
        if remaining and same(value, remaining[0]):
            remaining.pop(0)
        else:
            extras.append(value)
    return not remaining and all(any(same(value, item) for item in allowed) for value in extras) and len(extras) == len({json.dumps(v) for v in extras})


def deletion(before, after, removable):
    return insertion(after, before, removable)


class Correction:
    """Compare raw drafts before code-owned expansion, inheritance or forecast fields."""
    def __init__(self, draft, objections, inputs, legacy):
        self.draft = deepcopy(draft)
        self.objections = deepcopy(objections)
        self.inputs = deepcopy(inputs)
        self.legacy = legacy
        self.base_nodes = set(getattr(legacy, "base_nodes", ()))
        self.base_sources = dict(getattr(legacy, 'base_sources', {}))
        self.permissions = {}
        for item in objections:
            operation = rules()[item.rule]['footprint']
            self.permissions.setdefault(operation, []).append(item.items)

    def check(self, candidate):
        if candidate.get('kind') == 'cant_meet':
            original_files = self.draft.get('files', [])
            if self.draft.get('kind') == 'split':
                original_files = list(dict.fromkeys(p for piece in self.draft['pieces'] for p in piece['files']))
            added = [p for p in candidate['files'] if p not in original_files]
            return self.failure(['/files']) if added else []
        if candidate.get('kind') == 'split' and all(rules()[item.rule]['class'] == 'size_execution_fit' for item in self.objections):
            return self.split(candidate)
        if candidate.get('kind') != self.draft.get('kind'):
            return self.failure(['/kind'])
        original = self.draft
        bad = []
        for key in set(original) | set(candidate):
            if key not in {'checks', 'edits_tests', 'pieces'} and (
                    (key in original) != (key in candidate) or not same(original.get(key), candidate.get(key))):
                if not self.top_field(key, candidate):
                    bad.append('/' + key)
        if original.get('kind') == 'leaf':
            bad.extend(self.checks(candidate))
            if not same(original.get('edits_tests'), candidate.get('edits_tests')) or ('edits_tests' in original) != ('edits_tests' in candidate):
                old, new = original.get('edits_tests', []), candidate.get('edits_tests', [])
                removable = [*self.legacy.uncovered, *(item['node'] for item in self.permissions.get('EDIT_DROP', [])
                    if any(item['node'] in check['node_ids'] and check['kind'] == 'demonstrating'
                           for check in original['checks']))]
                if not (self.permissions.get('EDIT_COVER') or self.permissions.get('EDIT_DROP')) or not deletion(old, new, removable):
                    bad.append('/edits_tests')
        elif original.get('kind') == 'split':
            bad.extend(self.pieces(candidate))
        return self.failure(sorted(set(bad)))

    def split(self, candidate):
        """A size repair records every original obligation; it starts no child."""
        original = self.draft
        pieces = candidate['pieces']
        old_files = (original['files'] if original['kind'] == 'leaf' else
                     list(dict.fromkeys(path for piece in original['pieces'] for path in piece['files'])))
        new_files = {path for piece in pieces for path in piece['files']}
        failures = []
        if new_files != set(old_files):
            failures.append('/pieces/files')
        invariant = set(original) - {'kind', 'files', 'checks', 'edits_tests', 'no_design_questions',
                                     'lands_on_its_own', 'pieces', 'reason'}
        for field in invariant | (set(candidate) - {'kind', 'pieces', 'reason'}):
            if (field in original) != (field in candidate) or not same(original.get(field), candidate.get(field)):
                failures.append('/' + field)
        mappings = []
        for index, check in enumerate(original.get('checks', [])):
            paths = {node.split('::')[0] for node in check['node_ids']}
            paths.update(unit.split('::')[0] for field in ('subjects', 'observer_support')
                         for unit in check.get('proof', {}).get(field, []))
            owners = [i for i, piece in enumerate(pieces)
                      if set(check['goal_ids']) <= set(piece['goal_ids']) and paths <= set(piece['files'])]
            if not owners:
                failures.append(f'/checks/{index}/piece')
            mappings.append({'check': index, 'pieces': owners})
        if failures:
            return self.failure(failures)
        self.manifest = {
            'base': self.inputs.get('base_sha'), 'goals': deepcopy(self.inputs['ingest']['goals']),
            'checks': deepcopy(original.get('checks', [])), 'original_pieces': deepcopy(original.get('pieces', [])),
            'edits_tests': deepcopy(original.get('edits_tests', [])), 'files': old_files,
            'evidence_contract': deepcopy(self.legacy.completed_reply.get('evidence_contract', [])),
            'forecast': deepcopy(self.legacy.completed_reply['forecast']),
            'execution': deepcopy(self.legacy.completed_reply['execution']),
            'parent_estimates': {k: original[k] for k in ('estimated_tool_turns', 'estimated_exploration_turns') if k in original},
            'mapping': mappings}
        return []

    def top_field(self, key, candidate):
        original = self.draft
        a, b = original.get(key), candidate.get(key)
        if any(item.get('field') == key or key in item.get('keys', []) for item in self.permissions.get('CODE', [])):
            return key not in candidate
        if key == 'files':
            source_units = [unit for goal in self.inputs['ingest']['goals']
                            for unit in goal.get('proof_obligation', {}).get('subjects', [])]
            additions = [item['path'] for item in self.permissions.get('PROOF_FILE', [])
                         if any(subject_covers(item['unit'], unit) or subject_covers(unit, item['unit']) for unit in source_units)]
            additions.extend(item['path'] for item in self.permissions.get('EDIT_FILE', [])
                             if item['node'] in self.base_nodes)
            if self.permissions.get('FILES_FILL') and not a:
                additions.extend(unit.split('::')[0] for unit in source_units)
            if a is not None and b is not None and insertion(a, b, additions):
                return True
            renames = {item['path']: str(PurePosixPath(item['path'])) for item in self.permissions.get('FILE_PATH', [])
                       if not item['path'].startswith(('/', '-')) and '..' not in PurePosixPath(item['path']).parts
                       and '\\' not in item['path']}
            return bool(renames) and same([renames.get(path, path) for path in a], b)
        if key == 'evidence_contract':
            if 'evidence_contract' not in original and self.permissions.get('EVIDENCE_EMPTY'):
                return b == []
            return self.methods(a, b)
        if key == 'memory_ids' and self.permissions.get('MEMORY_DROP'):
            removed = [value for item in self.permissions.get('MEMORY_DROP', []) for value in item['ids']]
            return deletion(a or [], b or [], removed)
        if key == 'memory_flags' and (self.permissions.get('FLAG_DROP') or self.permissions.get('FLAG_CLAIM')):
            return self.flags(a or [], b or [])
        if key == 'reason' and self.permissions.get('SPLIT_REASON'):
            facts = self.legacy.completed_reply
            allowed = set()
            if self.inputs['must_split']:
                allowed.add('out_of_room')
            if not facts['forecast']['fits']:
                allowed.add('does_not_fit')
            if not facts['execution']['fits']:
                allowed.add('execution_budget')
            return b in allowed
        return False

    def methods(self, before, after):
        if not isinstance(before, list) or not isinstance(after, list) or len(before) != len(after):
            return False
        named_requirements = [item['requirement'] for item in self.permissions.get('METHOD_REPLACE', [])]
        mutable = {'tool', 'executor', 'runner_features', 'artifacts', 'methods', 'protected_files'}
        for old, new in zip(before, after):
            if same(old, new):
                continue
            if (old not in named_requirements or old.get('provenance', {}).get('kind') != 'method'
                    or not same({k: v for k, v in old.items() if k not in mutable},
                                {k: v for k, v in new.items() if k not in mutable})):
                return False
            if not any(all(new[k] == cap[k] if k in {'tool', 'executor'} else set(new[k]) <= set(cap[k])
                           for k in mutable) for cap in self.inputs.get('tool_capabilities', [])):
                return False
        return True

    def flags(self, before, after):
        removable = {value for item in self.permissions.get('FLAG_DROP', []) for value in item['ids']}
        claims = {item['flag'] for item in self.permissions.get('FLAG_CLAIM', [])}
        memories = {item['id']: item['content'] for item in self.inputs.get('memories', [])}
        position = 0
        for index, old in enumerate(before):
            if position < len(after) and same(old, after[position]):
                position += 1
            elif index in claims and position < len(after) and after[position].get('id') == old['id']:
                new = after[position]
                if (set(new) != set(old) or any(not same(old[k], new[k]) for k in old if k != 'claim')
                        or not new['claim'] or new['claim'] not in memories.get(old['id'], '')):
                    return False
                position += 1
            elif old['id'] not in removable and index not in claims:
                return False
        return position == len(after)

    def supports(self, goal, piece):
        source = self.inputs['ingest']['goals'][goal]
        return source['description'] in piece['brief'] and all(unit.split('::')[0] in piece['files']
            for unit in source.get('proof_obligation', {}).get('subjects', []))

    def pieces(self, candidate):
        before, after = self.draft['pieces'], candidate['pieces']
        if len(before) != len(after):
            return ['/pieces']
        bad_paths = []
        source_goals = set(range(len(self.inputs['ingest']['goals'])))
        missing = source_goals - {g for piece in before for g in piece['goal_ids']}
        for i, (old, new) in enumerate(zip(before, after)):
            if not same({k: v for k, v in old.items() if k != 'goal_ids'}, {k: v for k, v in new.items() if k != 'goal_ids'}):
                bad_paths.append(f'/pieces/{i}')
            a, b = old['goal_ids'], new['goal_ids']
            dropping = [g for g in a if g not in source_goals] if any(item.get('piece') == i for item in self.permissions.get('PIECE_DROP', [])) else []
            retained = [g for g in a if g not in dropping]
            additions = set()
            if self.permissions.get('PIECE_COVER'):
                additions.update(g for g in missing if self.supports(g, old))
            if not a and any(item.get('piece') == i for item in self.permissions.get('PIECE_GOALS', [])):
                additions.update(g for g in source_goals if self.supports(g, old))
            if not insertion(retained, b, additions):
                bad_paths.append(f'/pieces/{i}/goal_ids')
        return bad_paths

    def checks(self, candidate):
        before, after = self.draft['checks'], candidate['checks']
        bad = []
        if len(after) < len(before):
            bad.append('/checks')
        uncovered = self.legacy.uncovered
        goal_union = {goal for check in before for goal in check['goal_ids']}
        for i, new in enumerate(after):
            path = f'/checks/{i}'
            if i >= len(before):
                if (not uncovered or new['kind'] != 'demonstrating'
                        or not new['node_ids'] or not set(new['node_ids']) <= uncovered
                        or not set(new['goal_ids']) <= goal_union
                        or 'forcing' in new and not any(same(new['forcing'], old.get('forcing')) for old in before)
                        or 'declared_symptom' in new):
                    bad.append(path)
                continue
            old = before[i]
            for field in old.keys() | new.keys():
                if same(old.get(field), new.get(field)) and (field in old) == (field in new):
                    continue
                a, b = old.get(field), new.get(field)
                allowed = False
                if field == 'goal_ids':
                    allowed = self.goals(i, old, new)
                elif field == 'node_ids':
                    allowed = self.nodes(i, old, new)
                elif field == 'proof':
                    allowed = self.proof(i, old.get('proof', {}), new.get('proof', {}))
                elif field == 'forcing':
                    allowed = self.forcing(i, a, b)
                elif field == 'declared_symptom':
                    allowed = self.symptom(i, old, new)
                if not allowed:
                    bad.append(path + '/' + field)
        return bad

    def goals(self, index, old, new):
        a, b = old['goal_ids'], new['goal_ids']
        original = a
        count = len(self.inputs['ingest']['goals'])
        if any(item.get('check') == index for item in self.permissions.get('GOAL_SANITIZE', [])):
            a = list(dict.fromkeys(g for g in a if type(g) is int and 0 <= g < count))
        if any(item.get('check') == index for item in self.permissions.get('GOAL_DROP', [])):
            unknown = [item['unknown'] for item in self.permissions['GOAL_DROP'] if item.get('check') == index]
            a = [g for g in a if not any(g in ids for ids in unknown)]
        additions = list(self.legacy.missing_goals)
        if not original and any(item.get('check') == index for item in self.permissions.get('GOAL_ADD', [])):
            proof = old.get('proof', {})
            for g, goal in enumerate(self.inputs['ingest']['goals']):
                obligation = goal.get('proof_obligation')
                if (obligation and proof.get('red_message') == obligation['quote']
                        and all(any(subject_covers(unit, subject) for unit in proof.get('subjects', []))
                                for subject in obligation['subjects'])):
                    additions.append(g)
        return insertion(a, b, additions)

    def symptom(self, index, old, new):
        if not any(item.get('check') == index for item in self.permissions.get('SYMPTOM_FIELDS', [])):
            return False
        a, b = old['declared_symptom'], new['declared_symptom']
        if not isinstance(a, dict) or not isinstance(b, dict):
            return False
        fixed = {k: v for k, v in a.items() if k in {'node', 'exception', 'phase', 'repeat', 'parallel'}}
        if 'node' not in fixed and len(old['node_ids']) == 1:
            fixed['node'] = old['node_ids'][0]
        return same(fixed, b)

    def forcing(self, index, before, after):
        if not isinstance(before, dict) or not isinstance(after, dict):
            return False
        expected = deepcopy(before)
        if any(item.get('check') == index for item in self.permissions.get('FORCING_KEYS', [])):
            expected = {k: v for k, v in expected.items() if k in {'symptom', 'environment', 'patches'}}
        if any(item.get('check') == index for item in self.permissions.get('PATCH_MODE', [])):
            for patch in expected.get('patches', []):
                modes = set(patch) & {'value', 'returns', 'raises', 'delay_seconds'}
                if len(modes) == 1:
                    for key in list(patch):
                        if key not in {'target', *modes}:
                            del patch[key]
        # Reserved variables and symptom prose need trusted runner/source facts.
        # Their presence alone supplies no permission to erase a causal mutation.
        return same(expected, after)

    def nodes(self, index, old, new):
        a, b = old['node_ids'], new['node_ids']
        selectors = self.legacy.selectors.get(index, [])
        forced = [node for item in self.permissions.get('FORCED_SELECT', [])
                  if item.get('check') == index for node in item['nodes']]
        absent = [item['node'] for item in self.permissions.get('BASE_SELECT', []) if item.get('check') == index]
        replace = [*selectors, *forced, *absent]
        # Replacement permissions bind slots. Adding one replacement never
        # authorizes changing another selected node or its order.
        if replace:
            if len(a) != len(b):
                return False
            for before, after in zip(a, b):
                if same(before, after):
                    continue
                if before not in replace or before.split('::')[0] != after.split('::')[0]:
                    return False
                source = self.base_sources.get(after.split('::')[0])
                if source is None or test_function_source(source, after) is None:
                    return False
            return True
        if any(item.get('check') == index for item in self.permissions.get('NODE_DEDUP', [])) and same(list(dict.fromkeys(a)), b):
            return True
        frozen = [item['node'] for item in self.permissions.get('FROZEN', [])]
        if old['kind'] in {'regression', 'behavior-preserving'} and b and deletion(a, b, frozen):
            return True
        if old['kind'] == 'demonstrating' and self.legacy.uncovered:
            return insertion(a, b, list(self.legacy.uncovered))
        return False

    def proof(self, index, before, after):
        if not isinstance(after, dict):
            return False
        missing = self.legacy.missing_proof.get(index, set())
        subjects = self.legacy.missing_subjects.get(index, set())
        red = [item for item in self.permissions.get('RED_FILL', []) if item.get('check') == index]
        for key in before.keys() | after.keys():
            if key in before and key in after and same(before[key], after[key]):
                continue
            if key == 'subjects' and subjects:
                old = before.get(key, [])
                additions = [unit for unit in after.get(key, []) if unit not in old]
                if (not same(after.get(key, [])[:len(old)], old)
                        or any(unit.split('::')[0] not in self.draft['files'] for unit in additions)
                        or any(not any(subject_covers(unit, required) for required in subjects) for unit in additions)):
                    return False
            elif key in {'subjects', 'observer_support'} and any(item.get('check') == index for item in self.permissions.get('PROOF_DEDUP', [])) and same(list(dict.fromkeys(before.get(key, []))), after.get(key)):
                continue
            elif key == 'red_message' and red and not before.get(key, '').strip():
                # An exact source assertion is a fact. An arbitrary plausible
                # failure invented by the correction is not one.
                supported = {item['obligation']['quote'] for item in red if 'quote' in item.get('obligation', {})}
                if after.get(key) not in supported:
                    return False
            elif key in missing and key not in before:
                if key == 'red_message':
                    supported = {goal['proof_obligation']['quote'] for goal in self.inputs['ingest']['goals']
                                 if 'proof_obligation' in goal}
                    if after.get(key) not in supported:
                        return False
                elif after.get(key) != []:
                    return False
            else:
                return False
        return True

    def failure(self, paths):
        return [named('guard.fields_changed', 'Correction changes declarations outside the named objections: '
                      + ', '.join(paths), {'paths': paths})] if paths else []


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


COVERAGE_REPAIR = (
    'Only add the named uncovered goal ids to goal_ids of existing checks. '
    'Preserve every existing citation and every other declaration. '
    'Use already-read facts; return cant_meet if no existing check honestly supports the goals.')


class PlanConsistencyError(ValueError):
    """A draft's checks disagree with its declarations or exact base selections."""

    def __init__(self, uncovered, missing_forced=(), *, draft=None, selectors=None, missing_proof=None,
                 missing_subjects=None, missing_goals=None, base_sha=None):
        self.draft = deepcopy(draft)
        self.base_sha = base_sha
        self.uncovered = set(uncovered)
        self.missing_forced = set(missing_forced)
        self.selectors = selectors or {}
        self.missing_proof = missing_proof or {}
        self.missing_subjects = missing_subjects or {}
        self.missing_goals = dict(sorted((missing_goals or {}).items()))
        self.local = bool(self.selectors or self.missing_proof or self.missing_subjects
                          or self.missing_goals and not (self.uncovered or self.missing_forced))
        self.node_ids = (*uncovered, *missing_forced,
                         *(subject for subjects in self.missing_subjects.values() for subject in sorted(subjects)),
                         *(node for nodes in self.selectors.values() for node in nodes),
                         *(f"check-{index + 1} proof: {', '.join(sorted(fields))}"
                           for index, fields in self.missing_proof.items()))
        self.violations = [
            f"`{node_id}` is in edits_tests but no demonstrating check names it. "
            "Name it in a demonstrating check that fails on the base by an assertion."
            for node_id in uncovered]
        self.violations.extend(f"Existing check must name a test defined at the base: {node_id}"
                               for node_id in missing_forced)
        self.violations.extend(
            f"Checks must name exact pytest node IDs. Replace `{node}` with the exact test definitions this check needs."
            for nodes in self.selectors.values() for node in nodes)
        self.violations.extend(
            f"check-{index + 1} is missing required proof declarations: {', '.join(sorted(fields))}. "
            "State the expected assertion message supported by this check. Missing subjects or observer_support "
            "may only be filled with [] for an inferred partition. Preserve all existing proof values."
            for index, fields in self.missing_proof.items())
        self.violations.extend(
            f"check-{index + 1} source assertion proof must identify its subjects in proof.subjects: "
            + ', '.join(sorted(subjects))
            + '. Only add the named subjects or their covering enclosing modules or definitions.'
            for index, subjects in self.missing_subjects.items())
        if self.missing_goals:
            self.violations.append(self.coverage_violation(self.missing_goals))
        coverage_only = self.missing_goals and not any((self.uncovered, self.missing_forced,
            self.selectors, self.missing_proof, self.missing_subjects))
        if coverage_only:
            super().__init__('\n'.join(self.violations) + '\n' + COVERAGE_REPAIR)
            self.repair_text = self.bounded_coverage_message(COVERAGE_REPAIR)
            return
        if self.local:
            super().__init__("\n".join(self.violations) +
                             "\nUse already-read facts. Only replace the named check's selectors or fill its "
                             "missing proof fields or named subjects. Preserve every other declaration, existing proof value, "
                             "check, goal, forcing, evidence obligation, file and estimate. "
                             "Do not invent base failures; return cant_meet if needed.")
            if self.missing_goals:
                self.add_coverage_instruction()
            return
        super().__init__("\n".join(self.violations) +
                         "\nReconcile the declaration and checks using already-read facts. "
                         "Keep the draft's files, goals, check kinds and forcings. Only add missing "
                         "demonstrating coverage, replace forced non-base selections with base tests, "
                         "replace non-exact selectors with the exact base tests the check needs, "
                         "or drop an uncovered unnecessary edit. Do not expand the draft. "
                         "Drop an edit only if it is unnecessary for the goals. "
                         "A forced check must name existing base tests and preserve every required goal, "
                         "including deterministic and mutant proof; do not treat a passing smoke as red. "
                         "Do not weaken goals or invent base failures; return cant_meet if needed.")
        if self.missing_goals:
            self.add_coverage_instruction()

    @staticmethod
    def coverage_violation(goals):
        return 'Required goals have no check coverage: ' + '; '.join(
            f'goal {index}: {description}' for index, description in goals.items())

    def add_coverage_instruction(self):
        instructions = str(self).split('\n')[-1] + ' ' + COVERAGE_REPAIR
        self.args = ('\n'.join(self.violations) + '\n' + instructions,)
        self.repair_text = self.bounded_coverage_message(instructions)

    def bounded_coverage_message(self, instructions):
        """Keep ids/instructions; share the remaining UTF-8 bytes equally among descriptions."""
        prefix = '\n'.join(self.violations[:-1])
        if prefix:
            prefix += '\n'

        def message(goals):
            return prefix + self.coverage_violation(goals) + '\n' + instructions

        full = message(self.missing_goals)
        if len(full.encode()) <= REPAIR_MESSAGE_BYTES:
            return full
        marker = ' [...]'
        fixed = message(dict.fromkeys(self.missing_goals, ''))
        available = REPAIR_MESSAGE_BYTES - len(fixed.encode())
        if available < 0:
            return None
        cap = available // len(self.missing_goals)
        return message({index: (description if len(description.encode()) <= cap else
                                description.encode()[:cap - len(marker)].decode(errors='ignore') + marker
                                if cap >= len(marker) else '')
                        for index, description in self.missing_goals.items()})

    @classmethod
    def semantic(cls, raw, completed, diagnostics, inputs, legacy):
        error = legacy
        error.raw_draft = deepcopy(raw)
        error.completed_reply = deepcopy(completed)
        error.objections = diagnostics
        error.contract = Correction(raw, diagnostics, inputs, legacy)
        error.repair_text = (error.bounded_coverage_message('goal.coverage: ' + COVERAGE_REPAIR)
                             if diagnostics and all(item.rule == 'goal.coverage' for item in diagnostics)
                             else message(diagnostics))
        error.unavailable_reason = ('disabled' if not inputs.get('repair_plan_consistency', False)
                                    else 'message_limit' if error.repair_text is None else None)
        error.args = (error.repair_text or '\n'.join(str(item) for item in diagnostics),)
        return error

    def constrain(self, reply, raw=None):
        if hasattr(self, 'contract'):
            candidate = raw if raw is not None else {key: value for key, value in reply.items()
                if key not in {'forecast', 'execution', 'violations', 'decision_context', 'evidence_errors', 'forcing_rehearsals'}}
            errors = self.contract.check(candidate)
            reply['violations'].extend(str(item) for item in errors)
            return errors
        self.constrain_draft(reply)
        if self.missing_goals and reply['violations']:
            reply['violations'] = list(dict.fromkeys([
                *self.violations, *reply['violations'], 'Planner coverage correction attempts: 1.']))

    def constrain_draft(self, reply):
        """Keep the correction within the declarations of the rejected draft."""
        draft = self.draft
        errors = []
        if reply['kind'] not in {'leaf', 'cant_meet'}:
            reply['violations'].append('Correction must remain a leaf or honestly report cant_meet.')
            return
        added = set(reply['files']) - set(draft['files'])
        if added:
            errors.append('Correction expands the draft footprint: ' + ', '.join(sorted(added)))
        if reply['kind'] == 'cant_meet':
            reply['violations'].extend(errors)
            return
        if set(reply['files']) != set(draft['files']) and not added:
            errors.append('Correction changes the draft footprint.')
        mutable = {'checks', 'edits_tests', 'forecast', 'execution', 'violations', 'decision_context', 'evidence_errors', 'forcing_rehearsals'}
        if ({key: value for key, value in reply.items() if key not in mutable}
                != {key: value for key, value in draft.items() if key not in mutable}):
            errors.append('Correction changes declarations outside the test consistency error.')
        if self.missing_goals:
            for before, after in zip(draft['checks'], reply['checks']):
                if [goal for goal in after['goal_ids'] if goal not in self.missing_goals] != before['goal_ids']:
                    errors.append('Correction may only add the named uncovered goal ids and must preserve every existing citation.')
        if self.local:
            errors.extend(self.constrain_local(reply))
            reply['violations'].extend(dict.fromkeys(errors))
            return
        before, after = set(draft.get('edits_tests', [])), set(reply.get('edits_tests', []))
        if after - before or (before - after) - self.uncovered:
            errors.append('Correction may only drop uncovered edit declarations.')
        checks = draft['checks']
        if len(reply['checks']) < len(checks):
            errors.append('Correction removes a draft check.')
        uncovered = {function(node) for node in self.uncovered}
        goals = {goal for check in checks for goal in check['goal_ids']}
        for index, check in enumerate(reply['checks']):
            allowed_goals = goals | self.missing_goals.keys() if index < len(checks) else goals
            if not set(check['goal_ids']) <= allowed_goals:
                errors.append('Correction introduces a goal outside the draft checks.')
            if index >= len(checks):
                if (check['kind'] != 'demonstrating'
                        or any(function(node) not in uncovered for node in check['node_ids'])):
                    errors.append('Correction may only add demonstrating checks for uncovered edits.')
                if ('forcing' in check
                        and not any(check['forcing'] == original.get('forcing') for original in checks)):
                    errors.append('Correction adds a forcing absent from the draft.')
                if ('declared_symptom' in check and not any(
                        check['declared_symptom'] == original.get('declared_symptom') for original in checks)):
                    errors.append('Correction adds a declared symptom absent from the draft.')
                continue
            original = checks[index]
            mutable_check = {'node_ids', 'goal_ids'} if self.missing_goals else {'node_ids'}
            if ({key: value for key, value in check.items() if key not in mutable_check}
                    != {key: value for key, value in original.items() if key not in mutable_check}):
                errors.append('Correction changes a draft check kind, goals or forcing.')
            old_nodes, new_nodes = set(original['node_ids']), set(check['node_ids'])
            replacing = original['kind'] == 'demonstrating' and 'forcing' in original
            removable = self.missing_forced if replacing else set()
            if (old_nodes - new_nodes) - removable:
                errors.append('Correction removes a required draft test selection.')
            if not (replacing and old_nodes & self.missing_forced):
                if any(check['kind'] != 'demonstrating' or function(node) not in uncovered
                       for node in new_nodes - old_nodes):
                    errors.append('Correction adds test selections outside the draft consistency error.')
        reply['violations'].extend(dict.fromkeys(errors))

    def constrain_local(self, reply):
        """Only named selectors, proof omissions and uncovered goal citations can change."""
        errors = []
        draft = self.draft
        if reply.get('edits_tests') != draft.get('edits_tests'):
            errors.append('Correction changes edit declarations.')
        if len(reply['checks']) != len(draft['checks']):
            errors.append('Correction changes the number of draft checks.')
        for index, (before, after) in enumerate(zip(draft['checks'], reply['checks'])):
            mutable = ({'node_ids'} if index in self.selectors else set())
            if self.missing_goals:
                mutable.add('goal_ids')
            if index in self.missing_proof or index in self.missing_subjects:
                mutable.add('proof')
                old, new = before.get('proof', {}), after.get('proof', {})
                subject_omission = index in self.missing_subjects
                if (any(new.get(key) != value for key, value in old.items()
                        if key != 'subjects' or not subject_omission)
                        or any(new.get(key) != [] for key in self.missing_proof.get(index, ())
                               if key != 'red_message' and (key != 'subjects' or not subject_omission))):
                    errors.append('Correction changes the declared proof partition or an existing proof value.')
                if subject_omission:
                    previous = old.get('subjects', [])
                    subjects = new.get('subjects', [])
                    if ([subject for subject in subjects if subject in previous] != previous
                            or any(not any(subject_covers(subject, missing)
                                           for missing in self.missing_subjects[index])
                                   for subject in subjects if subject not in previous)):
                        errors.append('Correction may only add the named missing proof subjects or their enclosing definitions.')
            if ({key: value for key, value in before.items() if key not in mutable}
                    != {key: value for key, value in after.items() if key not in mutable}):
                errors.append('Correction changes declarations outside the missing fields or selectors.')
            if index in self.selectors:
                for node in after['node_ids']:
                    try:
                        Check('planned', Kind(after['kind']), (node,))
                    except HarnessInputError:
                        errors.append(f'Corrected check must name an exact pytest node ID: {node}')
                        continue
                    oversized, exists = base_definition(node, self.base_sha)
                    if oversized or not exists:
                        errors.append(f'Corrected check must name a test defined at the base: {node}')
        return errors


def coverage_stop(error):
    """Retain complete coverage evidence when the existing single correction ends."""
    errors = error.errors if isinstance(error, InvalidReply) else (error,)
    if any(isinstance(item, Unrepairable) for item in errors):
        return None
    coverage = [item for item in errors if isinstance(item, PlanConsistencyError) and item.missing_goals]
    if not coverage:
        return None
    reply = deepcopy(coverage[-1].completed_reply)
    diagnostics = [violation for item in errors if isinstance(item, PlanConsistencyError)
                   for violation in item.violations]
    if not isinstance(error, InvalidReply):
        diagnostics.append('Planner coverage correction unavailable: the existing input allowance has no room for a correction turn.')
    if isinstance(error, InvalidReply) and not isinstance(errors[-1], PlanConsistencyError):
        diagnostics.append('Planner coverage correction failed reply validation.')
    reply['violations'] = list(dict.fromkeys([
        *diagnostics, *reply['violations'], f'Planner coverage correction attempts: {len(errors) - 1}.']))
    return reply


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
    missing_forced = []
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
                    forcings[index] = Forcing.parse(check["forcing"])
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
                            missing_forced.append(node_id)
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
        edit_errors, uncovered = edit_violations(inputs["ingest"]["goals"], reply, inputs.get('base_sha'))
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
    mismatch = PlanConsistencyError(uncovered, missing_forced, draft=raw_draft,
                                    selectors=selectors, missing_proof=missing_proof,
                                    missing_subjects=missing_subjects, missing_goals=missing_goals,
                                    base_sha=inputs.get('base_sha'))
    legacy_codes = ([('edit.uncovered', {'nodes': [node]}) for node in uncovered]
                    + [('forced.base_missing', {'check': i, 'nodes': nodes}) for i, nodes in missing_forced_checks.items() for _ in nodes]
                    + [('check.selector_inexact', {'check': i, 'nodes': nodes}) for i, nodes in selectors.items() for _ in nodes]
                    + [('proof.omitted', {'check': i, 'fields': sorted(fields)}) for i, fields in missing_proof.items()]
                    + [('proof.subject_missing', {'check': i, 'subjects': sorted(subjects)}) for i, subjects in missing_subjects.items()]
                    + ([('goal.coverage', {'goals': sorted(missing_goals), 'descriptions': missing_goals})] if missing_goals else []))
    violations.extend(named(code, text, items) for text, (code, items) in zip(mismatch.violations, legacy_codes))
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
        mismatch.base_nodes = base_nodes
        mismatch.base_sources = base_sources
        repair_error = PlanConsistencyError.semantic(raw_draft, reply, violations, inputs, mismatch)
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


def edit_violations(goals, reply, base_sha=None):
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
        definition = base_definition(node_id, base_sha)
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


ROLE = Role(
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
