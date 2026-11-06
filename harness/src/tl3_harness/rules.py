"""Refusal rows: checks, subjects and bounded repairs share one definition."""

from collections.abc import Callable, Iterable
from copy import deepcopy
from dataclasses import dataclass, field, replace
import inspect
import json
import re
from typing import Literal

Facts = dict


@dataclass(frozen=True)
class Name:
    kind: Literal['field', 'path', 'symbol']
    value: str

    def __post_init__(self):
        if self.kind not in ('field', 'path', 'symbol') or not isinstance(self.value, str):
            raise ValueError('invalid rule subject')
        if self.kind == 'field':
            if (self.value and not self.value.startswith('/')) or re.search(r'~(?![01])', self.value):
                raise ValueError('field subject must be a JSON pointer')
        elif (not self.value or self.value.startswith('/') or '..' in self.value.split('/')
              or '\\' in self.value or any(character in self.value for character in '\x00\r\n')):
            raise ValueError('source subject must be repository-relative')
        if self.kind == 'symbol':
            path, separator, qualname = self.value.partition('::')
            if (not separator or not path.endswith(('.py', '.rs'))
                    or not all(part.isidentifier() for part in qualname.split('.'))):
                raise ValueError('symbol subject must name path::qualname')

    def render(self):
        label = {'field': 'reply field', 'path': 'source path', 'symbol': 'symbol'}[self.kind]
        return f'{label} {json.dumps(self.value, ensure_ascii=False)}'


@dataclass(frozen=True, eq=False)
class Allowed:
    keyword: str
    value: object

    def __eq__(self, other):
        if not isinstance(other, Allowed):
            return NotImplemented
        return (self.keyword == other.keyword
                and json.dumps(self.value, sort_keys=True) == json.dumps(other.value, sort_keys=True))

    __hash__ = None

    def accepts(self, value):
        if self.keyword not in ('enum', 'const'):
            raise ValueError('core Allowed requires explicit enum or const membership')
        alternatives = (self.value,) if self.keyword == 'const' else self.value
        candidate = json.dumps(value, sort_keys=True)
        return candidate in {json.dumps(item, sort_keys=True) for item in alternatives}

    def render(self):
        return f'{self.keyword} {json.dumps(self.value, ensure_ascii=False, sort_keys=True)}'


@dataclass(frozen=True)
class Edit:
    pointer: Name
    op: Literal['add', 'replace', 'remove']
    allowed: Allowed

    def __post_init__(self):
        if self.pointer.kind != 'field' or self.op not in ('add', 'replace', 'remove'):
            raise ValueError('repair edit needs a field pointer and an operation')


@dataclass(frozen=True)
class CantMeet:
    reason: str


@dataclass(frozen=True)
class SourceEdit:
    path: str
    instruction: str

    def __post_init__(self):
        Name('path', self.path)
        if not self.instruction.strip():
            raise ValueError('source edit needs an instruction')


@dataclass(frozen=True)
class TerminalForm:
    role: str
    predicate: Callable[[object], bool]
    phrase: str

    def __post_init__(self):
        if not self.role or not callable(self.predicate) or not self.phrase.strip():
            raise ValueError('terminal reply form needs a predicate and phrase')


_TERMINALS = {}
_SOURCE_EDITORS = set()


def register_terminal(form):
    previous = _TERMINALS.get(form.role)
    if previous is not None and previous != form:
        raise ValueError(f'terminal reply form already registered for {form.role}')
    _TERMINALS[form.role] = form
    return form


def terminal_for(role):
    if role not in _TERMINALS:
        raise ValueError(f'no terminal reply form registered for {role}')
    return _TERMINALS[role]


def register_source_editor(role):
    _SOURCE_EDITORS.add(role)
    return role


def is_source_editor(role):
    return role in _SOURCE_EDITORS


def _uses_role(value):
    if not callable(value):
        return False
    try:
        inspect.signature(value).bind({}, 'role')
    except TypeError:
        return False
    return True


def _resolve(value, facts, role):
    if not callable(value):
        return value
    return value(facts, role) if _uses_role(value) else value(facts)


def _require_repair(repair, subjects, role):
    if isinstance(repair, CantMeet) and repair.reason.strip():
        terminal_for(role)
    elif isinstance(repair, SourceEdit):
        if not is_source_editor(role):
            raise ValueError(f'role {role} is not a source editor')
        if not subjects or any(name.kind not in ('path', 'symbol') for name in subjects):
            raise ValueError('source edit needs path or symbol subjects')


@dataclass(frozen=True)
class Rule:
    id: str
    check: Callable[[Facts], Iterable[Facts]]
    names: Callable[[Facts], tuple[Name, ...]]
    allowed: Allowed | Callable[[Facts], Allowed] | Callable[[Facts, str], Allowed]
    repair: Edit | CantMeet | SourceEdit | Callable[[Facts], Edit | CantMeet | SourceEdit] | Callable[[Facts, str], Edit | CantMeet | SourceEdit]
    gating: bool = False
    example: Facts | None = None
    witness: object = None
    roles: frozenset[str] = frozenset()


@dataclass(frozen=True)
class Violation:
    rule: Rule
    facts: Facts
    role: str | None = None
    allowed: Allowed = field(init=False)
    repair: Edit | CantMeet | SourceEdit = field(init=False)

    def __post_init__(self):
        object.__setattr__(self, 'allowed', _resolve(self.rule.allowed, self.facts, self.role))
        object.__setattr__(self, 'repair', _resolve(self.rule.repair, self.facts, self.role))
        if self.role is not None:
            _require_role(self.rule, self.role)
            _require_repair(self.repair, self.rule.names(self.facts), self.role)

    def _require_resolution_role(self, role):
        if self.role != role and (_uses_role(self.rule.allowed) or _uses_role(self.rule.repair)):
            raise ValueError('role-dependent violation must use its evaluation role')

    def render(self, *, role, context="reply"):
        _require_role(self.rule, role)
        self._require_resolution_role(role)
        subjects = self.rule.names(self.facts)
        allowed, repair = self.allowed, self.repair
        _require_repair(repair, subjects, role)
        if not subjects or any(not isinstance(name, Name) for name in subjects):
            raise ValueError('refusal needs typed subjects')
        text = f'{self.rule.id}: ' + ', '.join(name.render() for name in subjects) + '.'
        if 'bad_value' in self.facts:
            value = self.facts['bad_value']
            # Large containers need their subject identity, not a copy of the reply.
            shown = json.dumps(value, ensure_ascii=False)
            if len(shown) <= 256:
                text += f' Bad value: {shown}.'
        text += f' Allowed: {allowed.render()}.'
        if isinstance(repair, Edit):
            text += f' Permitted edit: {repair.op} {repair.pointer.render()}'
            if repair.allowed != allowed:
                text += f'; allowed: {repair.allowed.render()}.'
            elif repair.op == 'remove':
                text += '.'
            else:
                text += ' with one of the allowed values above.'
        elif isinstance(repair, SourceEdit):
            text += f' Change {json.dumps(repair.path, ensure_ascii=False)}: {repair.instruction}'
        elif isinstance(repair, CantMeet) and repair.reason.strip():
            text += f' {terminal_for(role).phrase} Reason: {repair.reason}'
        else:
            raise ValueError('rule has no renderable repair')
        if self.rule.gating:
            text += ' Further checks were not run.'
        return text


def _require_role(row, role):
    if role not in row.roles:
        raise ValueError(f'role {role} was not declared for rule {row.id}')


def evaluate(rules, facts, *, role):
    violations = []
    for row in rules:
        _require_role(row, role)
        found = [Violation(row, item, role=role) for item in row.check(facts)]
        violations.extend(found)
        if found and row.gating:
            break
    return tuple(violations)


def _parts(pointer):
    return [part.replace('~1', '/').replace('~0', '~') for part in pointer.split('/')[1:]]


def at(document, pointer):
    for part in _parts(pointer):
        document = document[int(part)] if isinstance(document, list) else document[part]
    return document


def apply_edit(document, edit, value=None):
    """Apply the exact operation rendered for a row, without mutating the draft."""
    result = deepcopy(document)
    parts = _parts(edit.pointer.value)
    if not parts:
        if edit.op != 'replace':
            raise ValueError('only replace is supported at the reply root')
        return deepcopy(value)
    parent = at(result, edit.pointer.value.rsplit('/', 1)[0])
    key = int(parts[-1]) if isinstance(parent, list) else parts[-1]
    exists = 0 <= key < len(parent) if isinstance(parent, list) else key in parent
    if edit.op == 'add':
        if exists and not isinstance(parent, list):
            raise ValueError('add requires a missing field')
        if isinstance(parent, list):
            if not 0 <= key <= len(parent):
                raise ValueError('array add index is out of bounds')
            parent.insert(key, deepcopy(value))
        else:
            parent[key] = deepcopy(value)
    elif edit.op == 'remove':
        del parent[key]
    else:
        if not exists:
            raise ValueError('replace requires an existing field')
        parent[key] = deepcopy(value)
    return result


def permits(violation_or_rule, before, after, *, role):
    """The candidate differs only by the union of the rows' permitted edits."""
    if isinstance(violation_or_rule, Rule):
        violations = evaluate((violation_or_rule,), {**(violation_or_rule.example or {}), 'reply': before}, role=role)
    else:
        violations = (violation_or_rule,) if isinstance(violation_or_rule, Violation) else tuple(violation_or_rule)
    for item in violations:
        _require_role(item.rule, role)
        item._require_resolution_role(role)
    expected = deepcopy(before)
    try:
        repairs = [(item, item.repair) for item in violations]
        for item, repair in repairs:
            _require_repair(repair, item.rule.names(item.facts), role)
        if any(isinstance(repair, CantMeet) for item, repair in repairs):
            return bool(terminal_for(role).predicate(after))
        for violation, edit in repairs:
            if isinstance(edit, SourceEdit):
                continue
            if not isinstance(edit, Edit):
                return False
            value = None if edit.op == 'remove' else at(after, edit.pointer.value)
            if edit.op != 'remove' and not edit.allowed.accepts(value):
                return False
            expected = apply_edit(expected, edit, value)
    except (KeyError, IndexError, TypeError, ValueError):
        return False
    return json.dumps(expected, sort_keys=True) == json.dumps(after, sort_keys=True)


def register(rule, *, roles: Iterable[str]):
    """Reject rows whose own rendered repair cannot pass their check and guard."""
    row = replace(rule, roles=frozenset(roles))
    if not row.roles:
        raise ValueError('registration needs consuming roles')
    example, witness = row.example, row.witness
    if example is None or 'reply' not in example:
        raise ValueError('registration needs an example with reply')
    if re.fullmatch(r'[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)+', row.id) is None:
        raise ValueError('rule id must be stable and dotted')
    examples = {role: evaluate((row,), example, role=role) for role in row.roles}
    role_dependent = _uses_role(row.repair) or _uses_role(row.allowed)
    source_repairs = any(isinstance(item.repair, SourceEdit)
                         for violations in examples.values() for item in violations)
    by_role = isinstance(witness, dict) and (
        role_dependent or source_repairs or bool(row.roles.intersection(witness)))
    if role_dependent and not isinstance(witness, dict):
        raise ValueError('role-dependent repair needs witnesses by role')
    if by_role and row.roles.difference(witness):
        raise ValueError('witness missing declared roles: ' + ', '.join(sorted(row.roles.difference(witness))))
    for role, violations in examples.items():
        if not violations:
            raise ValueError('registration needs a violating example')
        for violation in violations:
            violation.render(role=role)
        selected = witness[role] if by_role else witness
        if any(isinstance(item.repair, SourceEdit) for item in violations):
            if selected is not None:
                raise ValueError('SourceEdit requires no reply witness; use None for its role')
            # A source change is validated by the editor's ordinary test/proof run.
            if all(isinstance(item.repair, SourceEdit) for item in violations):
                continue
        if any(isinstance(item.repair, CantMeet) for item in violations):
            if not permits(violations, example['reply'], selected, role=role):
                raise ValueError('rendered repair rejects its own witness')
            continue
        # Role maps carry complete edited replies. A0's single value witnesses
        # remain accepted for rows that do not resolve repairs by role.
        if by_role or permits(violations, example['reply'], selected, role=role):
            repaired = deepcopy(selected)
        else:
            repaired = deepcopy(example['reply'])
            for violation in violations:
                if isinstance(violation.repair, Edit):
                    repaired = apply_edit(repaired, violation.repair, selected)
        if not permits(violations, example['reply'], repaired, role=role):
            raise ValueError('rendered repair rejects its own witness')
        candidates = [repaired]
        for violation in violations:
            edit = violation.repair
            if isinstance(edit, Edit) and edit.op != 'remove':
                choices = (edit.allowed.value,) if edit.allowed.keyword == 'const' else (
                    edit.allowed.value if edit.allowed.keyword == 'enum' else ())
                candidates.extend(apply_edit(repaired, Edit(edit.pointer, 'replace', edit.allowed), value)
                                  for value in choices)
        for candidate in candidates:
            if (not permits(violations, example['reply'], candidate, role=role)
                    or any(not isinstance(item.repair, SourceEdit) for item in
                           evaluate((row,), {**example, 'reply': candidate}, role=role))):
                raise ValueError('rendered repair rejects its own witness')

    return row


def render_all(violations, *, role, context="reply"):
    return '\n'.join(violation.render(role=role) for violation in violations)
