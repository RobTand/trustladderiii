"""Branch obligations are checked before planning without weakening the issue."""
from copy import deepcopy

import pytest

from tl3_workers.evidence import preflight


CONTEXT = {
    'repository': 'RobTand/prismabuild', 'issue': 2646, 'node': 200,
    'template': 'prismabuild-{issue}-tl3-{node}', 'branch': 'prismabuild-2646-tl3-200',
}


def goal(publication):
    return {'id': 'branch', 'description': 'Publish on the requested branch.',
            'verification': 'review', 'stage': 'publishing', 'publication': publication,
            'source': {'kind': 'issue', 'quote': 'Use exactly prismabuild-2646.'}}


@pytest.mark.parametrize(('requirement', 'satisfied'), [
    ({'branch': 'prismabuild-2646'}, False),
    ({'branch': 'prismabuild-2646-tl3-200'}, True),
    ({'branch_prefix': 'prismabuild-2646'}, True),
    ({'branch_prefix': 'other-'}, False),
])
def test_t604_audit_checks_actual_branch_semantics_and_preserves_provenance(requirement, satisfied):
    goals = [goal(requirement)]
    original = deepcopy(goals)
    inputs = {'publication_branch': CONTEXT, 'ingest': {'goals': goals}, 'tool_capabilities': []}
    reply = {'verdict': 'agree', 'objections': [], 'evidence_contract': []}
    preflight(inputs, reply)
    assert (reply['verdict'] == 'agree') is satisfied
    assert goals == original
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
    assert reply['goals'][0]['publication'] == {'branch': 'prismabuild-2646'}
