"""Sanitized request166 scope exchange, without repository-specific policy."""

from copy import deepcopy

import pytest

from tl3_workers.contracts import validate
from tl3_workers.roles import audit, ingest, planner


FOCUSED = 'tests/test_transition.py'
METADATA = 'tests/wait_counts.txt'
ESTIMATE = f'Expected footprint: `{FOCUSED}` and a test-owned clock adapter.'
BAN = f'The only edited file is `{FOCUSED}`.'
EXCLUSION = 'Do not edit `.github/**`.'


def goal(description, scope_kind=None, paths=None, quote=None, **source):
    result = {'description': description, 'verification': 'review'}
    if scope_kind:
        result['scope'] = {'kind': scope_kind, 'paths': paths, 'source': {
            'kind': 'issue', 'quote': quote, **source}}
    return result


def inputs(body=ESTIMATE, goals=None):
    return {'issue': {'number': 1, 'title': 'Remove real waits', 'body': body,
                      'author': 'owner', 'comments': []},
            'repo': {'readme': '', 'file_list': [FOCUSED, METADATA]},
            'ingest': brief(goals or [goal('Remove waits without changing transition behavior.')])}


def brief(goals):
    return {'kind': 'brief', 'brief': 'Remove waits. Required metadata counts may only shrink.',
            'goals': deepcopy(goals), 'memory_ids': []}


def test_estimate_cannot_be_promoted_to_an_exclusive_goal():
    reply = brief([goal(BAN)])
    with pytest.raises(ValueError, match='goal-1.*source'):
        ingest.check_memory_ids(inputs(), reply)


@pytest.mark.parametrize('kind,description,paths,quote', [
    ('estimate', ESTIMATE, [FOCUSED], ESTIMATE),
    ('only', BAN, [FOCUSED], BAN),
    ('exclude', EXCLUSION, ['.github/**'], EXCLUSION),
])
def test_scope_source_survives_ingest_and_downstream_contracts(kind, description, paths, quote):
    goals = [goal(description, kind, paths, quote)]
    reply = brief(goals)
    ingest.check_memory_ids(inputs(quote), reply)
    validate('ingest-reply', reply)
    identified = [{**item, 'id': f'goal-{i + 1}'} for i, item in enumerate(reply['goals'])]
    # Exercise the actual goal schema at each later role boundary.
    from tl3_workers.contracts import registry, schema
    from jsonschema import Draft202012Validator
    for name in ('maker-input', 'reviewer-input'):
        contract = {'$ref': schema(name)['$id'] + '#/properties/goals'}
        Draft202012Validator(contract, registry=registry()).validate(identified)
    assert reply['goals'] == goals


@pytest.mark.parametrize('source', [ESTIMATE, 'Expected footprint: focused Python file.'])
def test_a_quoted_estimate_still_cannot_authorize_an_edit_ban(source):
    reply = brief([goal(BAN, 'only', [FOCUSED], source)])
    with pytest.raises(ValueError, match='goal-1.*explicit'):
        ingest.check_memory_ids(inputs(source), reply)


def test_audit_rejects_invented_exclusivity_instead_of_agreeing():
    data = inputs(goals=[goal(BAN)])
    reply = {'verdict': 'agree'}
    audit.ROLE.check_reply(data, reply)
    assert reply['verdict'] == 'disagree'
    assert any('goal-1' in reason and 'source' in reason for reason in reply['objections'])


@pytest.fixture(scope='session', autouse=True)
def tokenizer(tmp_path_factory):
    import hashlib
    from tiktoken.load import read_file_cached
    from tl3_workers.forecast import TOKENIZER_URL, TOKENIZER_SHA256
    data = read_file_cached(TOKENIZER_URL, TOKENIZER_SHA256)
    cache = tmp_path_factory.mktemp('scope-tokenizer')
    (cache / hashlib.sha1(TOKENIZER_URL.encode()).hexdigest()).write_bytes(data)
    with pytest.MonkeyPatch.context() as patch:
        patch.setenv('TIKTOKEN_CACHE_DIR', str(cache))
        yield


@pytest.fixture
def plan_inputs(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    (tmp_path / 'tests').mkdir()
    (tmp_path / FOCUSED).write_text('def test_transition(): assert True\n')
    (tmp_path / METADATA).write_text('sleep 2\n')
    return {'ingest': brief([goal(BAN, 'only', [FOCUSED], BAN)]),
            'must_split': False, 'protected_paths': ['.github/**'],
            'forecast': {'fit_tokens': 100000, 'context_tokens': 150000,
                         'overhead_tokens': {'prompt': 10}}}


def leaf(files):
    return {'kind': 'leaf', 'files': files, 'no_design_questions': True,
            'lands_on_its_own': True, 'checks': [
                {'kind': 'behavior-preserving', 'node_ids': [FOCUSED + '::test_transition'],
                 'goal_ids': [0]}]}


def test_required_metadata_cleanup_cannot_silently_override_a_real_ban(plan_inputs):
    reply = leaf([FOCUSED, METADATA])
    planner.check_reply(plan_inputs, reply)
    assert any('goal-1' in reason and METADATA in reason and 'permission' in reason
               for reason in reply['violations'])
    assert plan_inputs['ingest']['goals'][0]['description'] == BAN


def test_expected_footprint_allows_cleanup_but_keeps_protected_exclusions(plan_inputs):
    plan_inputs['ingest'] = brief([goal(ESTIMATE, 'estimate', [FOCUSED], ESTIMATE),
                                  goal(EXCLUSION, 'exclude', ['.github/**'], EXCLUSION)])
    reply = leaf([FOCUSED, METADATA])
    reply['checks'][0]['goal_ids'] = [0, 1]
    planner.check_reply(plan_inputs, reply)
    assert reply['violations'] == []
    denied = leaf([FOCUSED, '.github/action.yml'])
    denied['checks'][0]['goal_ids'] = [0, 1]
    planner.check_reply(plan_inputs, denied)
    assert any('goal-2' in reason and '.github/action.yml' in reason
               for reason in denied['violations'])


def test_new_scope_contract_must_not_turn_an_estimate_into_a_ban_at_planning(plan_inputs):
    plan_inputs['ingest'] = brief([goal(BAN)])
    reply = leaf([FOCUSED, METADATA])
    planner.check_reply(plan_inputs, reply)
    assert any('goal-1' in reason and 'source' in reason for reason in reply['violations'])


@pytest.mark.parametrize('metadata', ['metadata/dependencies.txt', 'docs/index.json'])
def test_scope_conflict_diagnostic_is_repository_independent(plan_inputs, metadata):
    reply = leaf([FOCUSED, metadata])
    planner.check_reply(plan_inputs, reply)
    assert any('goal-1' in reason and metadata in reason for reason in reply['violations'])


def test_only_a_current_decision_can_supply_scope_authorization():
    quoted = f'Only edit `{FOCUSED}`.'
    reply = brief([goal(BAN, 'only', [FOCUSED], quoted, kind='decision', decision_id='d1')])
    data = inputs()
    data['decision_context'] = {'schema_version': 1, 'request_version': 1, 'decisions': [{
        'id': 'd1', 'source_comment': 'comment-1', 'answer': quoted,
        'restrictions': [quoted], 'allowed_scope': [FOCUSED], 'question': {},
        'origin_request_version': 1, 'superseded_by': 'd2'}]}
    with pytest.raises(ValueError, match='goal-1.*source'):
        ingest.check_memory_ids(data, reply)


@pytest.mark.parametrize('text,kind', [
    # request188 goal-8: an evidence clause and a reporting clause, not an edit ban.
    ('Run new or changed tests in `tests/test_reader.py` through run_tests. '
     'Report only allowlisted fields: status, exit code and receipt hashes.', None),
    ('Changed tests must pass; only status and exit code are reported.', None),
    (BAN, 'only'),
    ('Edit only `tests/test_a.py` and `tests/test_b.py`.', 'only'),
    ('Only `tests/test_the_claim_pass_times_its_transition_holds.py` may be edited.', 'only'),
    (EXCLUSION, 'exclude'),
])
def test_only_restrictions_stay_within_one_clause(text, kind):
    from tl3_workers.scope import restriction_kind
    assert restriction_kind(text) == kind
