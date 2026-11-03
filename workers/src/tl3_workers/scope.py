"""Source-backed edit restrictions, separate from footprint estimates."""

from pathlib import PurePosixPath
import re

from tl3_workers.tools import _parts


# Recognize common edit prohibitions for missing-source diagnostics. The audit
# still judges arbitrary natural language and whether every obligation survived.
_EDIT = r'\b(?:edit(?:ed|ing|s)?|chang(?:e[ds]?|ing)|modif(?:y|ied|ying)|touch(?:ed|ing)?)\b'
# One clause only: a sentence end ('. ', ';' or a line break) separates a reporting or
# evidence 'only' from an edit verb in the next sentence. Dots inside paths still match.
_CLAUSE = r'(?:[^\n.;]|\.(?=\S))'
_ONLY = re.compile(rf'\bonly\b{_CLAUSE}{{0,160}}{_EDIT}|{_EDIT}{_CLAUSE}{{0,160}}\bonly\b', re.I)
_FILE = re.compile(r'\b(?:files?|paths?|footprint)\b|(?:[\w.*-]+/)+[\w.*-]+|\b[\w*-]+\.[\w*]+', re.I)
_EXCLUDE = re.compile(rf'\b(?:do not|must not|never|cannot)\s+{_EDIT}', re.I)


def restriction_kind(text):
    if not _FILE.search(text):
        return None
    if _EXCLUDE.search(text):
        return 'exclude'
    if _ONLY.search(text):
        return 'only'
    return None


def _source_text(inputs, source):
    if source['kind'] == 'issue':
        issue = inputs.get('issue')
        return [issue['title'], issue['body']] if issue is not None else None
    decisions = inputs.get('decision_context', {}).get('decisions', [])
    decision = next((item for item in decisions if item['id'] == source['decision_id']
                     and item.get('superseded_by') is None), None)
    if decision is None:
        return []
    return [decision['answer'], *decision['restrictions']]


def goal_errors(inputs, goals):
    errors = []
    for index, goal in enumerate(goals):
        name = goal.get('id', f'goal-{index + 1}')
        scope = goal.get('scope')
        restriction = restriction_kind(goal['description'])
        if scope is None:
            if restriction:
                errors.append(f'{name}: edit restriction needs an explicit issue or decision source in scope.')
            continue
        kind, source = scope['kind'], scope['source']
        quote = source['quote']
        if restriction and restriction != kind:
            errors.append(f'{name}: scope kind {kind} contradicts the goal\'s {restriction} edit restriction.')
        if kind != 'estimate' and restriction_kind(quote) != kind:
            errors.append(f'{name}: source must state an explicit {kind} edit restriction; a footprint estimate grants no ban.')
        texts = _source_text(inputs, source)
        # Planning receives accepted ingest goals, not the original issue. Audit
        # checks the issue quote before acceptance; decisions remain available.
        if texts is not None and not any(quote in text for text in texts):
            errors.append(f'{name}: scope source is absent or superseded; cite the issue or a current accepted decision.')
        for path in scope['paths']:
            try:
                if not _parts(path):
                    raise ValueError('empty path')
            except ValueError:
                errors.append(f'{name}: invalid scope path {path}.')
            if path not in quote:
                errors.append(f'{name}: scope source does not name {path}; retain the source\'s exact paths.')
    return errors


def plan_errors(inputs, paths):
    goals = inputs['ingest'].get('goals', [])
    errors = goal_errors(inputs, goals)
    for index, goal in enumerate(goals):
        scope = goal.get('scope')
        if scope is None or scope['kind'] == 'estimate':
            continue
        name = goal.get('id', f'goal-{index + 1}')
        for path in sorted(paths):
            matches = any(PurePosixPath(path).full_match(pattern) for pattern in scope['paths'])
            if (scope['kind'] == 'only' and not matches) or (scope['kind'] == 'exclude' and matches):
                errors.append(f'{name}: planned path {path} conflicts with {goal["description"]} '
                              f'Source: {scope["source"]["quote"]} '
                              f'Required permission: an authorized goal revision allowing edits to {path}. '
                              'File ownership and repository conventions do not grant that permission.')
    return errors
