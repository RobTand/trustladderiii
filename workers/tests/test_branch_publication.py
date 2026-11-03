"""Branch obligations are checked before planning without weakening the issue."""
from copy import deepcopy

import pytest

from tl3_workers.evidence import preflight
from tl3_workers.contracts import validate
from tl3_workers.roles import ROLES


CONTEXT = {
    'repository': 'RobTand/prismabuild', 'issue': 2646, 'node': 200,
    'template': 'prismabuild-{issue}-tl3-{node}', 'branch': 'prismabuild-2646-tl3-200',
}


def goal(publication):
    return {'description': f'Publish with {next(iter(publication))} {next(iter(publication.values()))}.',
            'verification': 'review', 'stage': 'publishing',
            'publication': {**publication, 'non_draft': True, 'body_lines': [], 'proof': False}}


@pytest.mark.parametrize(('requirement', 'satisfied'), [
    ({'branch': 'prismabuild-2646'}, False),
    ({'branch': 'prismabuild-2646-tl3-200'}, True),
    ({'branch_prefix': 'prismabuild-2646'}, True),
    ({'branch_prefix': 'other-'}, False),
])
def test_t604_audit_checks_actual_branch_semantics_and_preserves_provenance(requirement, satisfied):
    goals = [goal(requirement)]
    original = deepcopy(goals)
    quote = goals[0]['description']
    inputs = {'publication_branch': CONTEXT,
              'issue': {'number': 2646, 'title': 'Publish', 'body': quote,
                        'author': 'requester', 'comments': []},
              'repo': {'readme': 'Repo', 'file_list': []},
              'ingest': {'kind': 'brief', 'brief': f'Issue says: {quote}',
                         'memory_ids': [], 'goals': goals}, 'tool_capabilities': []}
    original_inputs = deepcopy(inputs)
    validate('audit-input', inputs)
    reply = {'verdict': 'agree', 'objections': [], 'evidence_contract': []}
    preflight(inputs, reply)
    assert (reply['verdict'] == 'agree') is satisfied
    assert goals == original
    assert inputs == original_inputs
    validate('audit-reply', reply)
    if not satisfied:
        objection = '\n'.join(reply['objections'])
        assert next(iter(requirement.values())) in objection
        assert CONTEXT['branch'] in objection
        assert CONTEXT['template'] in objection
        assert 'scope' in objection.lower()


def test_t604_ingest_reports_a_repairable_conflict_without_value_error():
    reply = {'kind': 'brief', 'goals': [goal({'branch': 'prismabuild-2646'})], 'evidence_contract': []}
    preflight({'publication_branch': CONTEXT, 'tool_capabilities': []}, reply)
    assert 'prismabuild-2646-tl3-200' in '\n'.join(reply['evidence_errors'])
    assert reply['goals'][0]['publication']['branch'] == 'prismabuild-2646'
    assert 'branch_prefix' not in reply['goals'][0]['publication']


@pytest.mark.parametrize(('sentence', 'pattern'), [
    ("Branch name follows PB's issue-link rule (TL3 uses `prismabuild-<issue>-tl3-<node>`)",
     'prismabuild-<issue>-tl3-<node>'),
    ('The base branch is main.', None),
    ('Never rename it after opening the PR.', None),
    ('Use the issue-link notation `prismabuild-<issue>[-<word>]`.',
     'prismabuild-<issue>[-<word>]'),
])
def test_t604_live_pb_and_incidental_branch_wording_pass_ingest_and_audit(sentence, pattern):
    publication = {'non_draft': True, 'body_lines': [], 'proof': False}
    if pattern:
        publication['branch'] = pattern
    goals = [{'description': sentence, 'verification': 'review', 'stage': 'publishing',
              'publication': publication}]
    inputs = {'publication_branch': CONTEXT, 'tool_capabilities': [],
              'issue': {'number': 2646, 'title': 'Publish', 'body': sentence,
                        'author': 'requester', 'comments': []},
              'repo': {'readme': 'Repo', 'file_list': []}}
    reply = {'kind': 'brief', 'brief': sentence, 'memory_ids': [], 'goals': goals,
             'evidence_contract': []}
    validate('ingest-input', inputs)
    validate('ingest-reply', reply)
    ROLES['ingest'].check_reply(inputs, reply)
    assert reply['evidence_errors'] == []
    assert reply['goals'] == goals
    validate('ingest-reply', reply)
    inputs['ingest'] = deepcopy(reply)
    validate('audit-input', inputs)
    audit = {'verdict': 'agree', 'objections': [], 'evidence_contract': []}
    ROLES['audit'].check_reply(inputs, audit)
    assert audit['verdict'] == 'agree'
    assert audit['objections'] == []
    validate('audit-reply', audit)
