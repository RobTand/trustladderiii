"""Refusal rows: checks, subjects and bounded repairs share one definition."""

from collections.abc import Callable, Iterable
from copy import deepcopy
from dataclasses import dataclass
import json
import re
from typing import Literal

from jsonschema import Draft202012Validator


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
        if self.kind == 'symbol' and ('::' not in self.value or any(not part for part in self.value.split('::'))):
            raise ValueError('symbol subject must name module::name')

    def render(self):
        label = {'field': 'reply field', 'path': 'source path', 'symbol': 'symbol'}[self.kind]
        return f'{label} {json.dumps(self.value, ensure_ascii=False)}'


@dataclass(frozen=True)
class Allowed:
    keyword: str
    value: object

    def accepts(self, value):
        constraint = self.value if self.keyword == 'schema' else {self.keyword: self.value}
        from tl3_workers.contracts import registry
        return Draft202012Validator(constraint, registry=registry()).is_valid(value)

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
class Rule:
    id: str
    check: Callable[[Facts], Iterable[Facts]]
    names: Callable[[Facts], tuple[Name, ...]]
    allowed: Callable[[Facts], Allowed]
    repair: Callable[[Facts], Edit | CantMeet]
    gating: bool = False


@dataclass(frozen=True)
class Violation:
    rule: Rule
    facts: Facts

    def render(self):
        subjects = self.rule.names(self.facts)
        allowed, repair = self.rule.allowed(self.facts), self.rule.repair(self.facts)
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
        if isinstance(repair, Edit) and repair.allowed == allowed:
            text += f' Permitted edit: {repair.op} {repair.pointer.render()}; allowed: {allowed.render()}.'
        elif isinstance(repair, CantMeet) and repair.reason.strip():
            text += f' Return cant_meet: {repair.reason}'
        else:
            raise ValueError('rule has no renderable repair')
        if self.rule.gating:
            text += ' Further checks were not run.'
        return text


def evaluate(rows, facts):
    violations = []
    for row in rows:
        found = [Violation(row, item) for item in row.check(facts)]
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


def permits(before, after, violations):
    """The candidate differs only by the union of the rows' permitted edits."""
    expected = deepcopy(before)
    try:
        for violation in violations:
            edit = violation.rule.repair(violation.facts)
            if not isinstance(edit, Edit):
                return False
            value = None if edit.op == 'remove' else at(after, edit.pointer.value)
            if edit.op != 'remove' and not edit.allowed.accepts(value):
                return False
            expected = apply_edit(expected, edit, value)
    except (KeyError, IndexError, TypeError, ValueError):
        return False
    return json.dumps(expected, sort_keys=True) == json.dumps(after, sort_keys=True)


def register(row, example, witness=None):
    """Reject rows whose own rendered repair cannot pass their check and guard."""
    if re.fullmatch(r'[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)+', row.id) is None:
        raise ValueError('rule id must be stable and dotted')
    violations = evaluate((row,), example)
    if not violations:
        raise ValueError('registration needs a violating example')
    repaired = deepcopy(example['reply'])
    for violation in violations:
        violation.render()
        edit = row.repair(violation.facts)
        if isinstance(edit, Edit):
            repaired = apply_edit(repaired, edit, witness)
    if all(isinstance(row.repair(item.facts), CantMeet) for item in violations):
        return row
    if not permits(example['reply'], repaired, violations) or evaluate((row,), {**example, 'reply': repaired}):
        raise ValueError('rendered repair rejects its own witness')
    return row


def render_all(violations):
    return '\n'.join(violation.render() for violation in violations)
