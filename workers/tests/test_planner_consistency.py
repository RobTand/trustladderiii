"""Planner consistency contracts."""
# ruff: noqa: F811

import copy
import json
from pathlib import Path

import pytest
from langchain_core.messages import HumanMessage

from planner_fixtures import (
    CANT_MEET,
    CONTROL,
    CREATED,
    FINISHED,
    INVENTED,
    LATE,
    LEAF,
    PBWAIT_TESTS,
    PROPOSED_EXPORTS,
    cache_tokenizer,  # noqa: F401
    planner_model,
    planner_request,  # noqa: F401
    request156,  # noqa: F401
    request162,  # noqa: F401
    request165,  # noqa: F401
    tool_capable_fake,  # noqa: F401
)


@pytest.mark.parametrize('correction', ['check', 'new_check', 'unchanged', 'cant_meet'])
def test_request156_gets_one_tool_free_consistency_correction(planner_request, request156, monkeypatch, correction):
    from tl3_workers import runtime
    from tl3_workers.forecast import Forecaster

    fixed = copy.deepcopy(request156)
    if correction == 'check':
        fixed['checks'][0]['node_ids'].append(CONTROL)
    elif correction == 'new_check':
        fixed['checks'].append({'proof': {'subjects': [], 'observer_support': [], 'red_message': 'expected regression'}, 'kind': 'demonstrating', 'node_ids': [CONTROL], 'goal_ids': [1]})
    elif correction == 'unchanged':
        fixed['edits_tests'].remove(CONTROL)
    else:
        fixed = {**copy.deepcopy(CANT_MEET), 'evidence_contract': []}
    # Recompute trusted measurements even though a correction cannot expand the footprint.
    measurements = []
    original = Forecaster.measure

    def measure(self, brief, paths):
        measurements.append(set(paths))
        return original(self, brief, paths)

    monkeypatch.setattr(Forecaster, 'measure', measure)
    model = planner_model(monkeypatch, request156, fixed)
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 2, 'the inconsistent draft must reach the existing correction boundary'
    history, offered = model.calls[1]
    assert model.calls[0][1] and not offered
    assert isinstance(history[-1], HumanMessage)
    assert CONTROL in history[-1].text and 'no demonstrating check' in history[-1].text
    assert 'Tools are unavailable' in history[-1].text
    assert json.loads(next(m.text for m in history if isinstance(m, HumanMessage)))['ingest'] == planner_request['inputs']['ingest']
    result = events[-1]['reply']
    assert result['kind'] == fixed['kind'] and result['violations'] == []
    assert result['files'] == fixed['files']
    assert measurements == [set(request156['files']), set(fixed['files'])]
    assert result['evidence_contract'] == planner_request['inputs']['ingest']['evidence_contract']
    assert result['evidence_errors'] == []
    assert sum(event['event'] == 'reply' for event in events) == 1
    if correction != 'cant_meet':
        assert result['checks'] == fixed['checks'] and result['edits_tests'] == fixed['edits_tests']


def test_request156_second_mismatch_stops_with_both_named_diagnostics(planner_request, request156, monkeypatch):
    from tl3_workers import runtime
    from tl3_workers.tracing import failure_tags
    from tl3_workers.turns import InvalidReply

    second = copy.deepcopy(request156)
    second['checks'][0]['node_ids'] = [CONTROL]
    model = planner_model(monkeypatch, request156, second, LEAF)
    events = []
    with pytest.raises(InvalidReply) as raised:
        runtime.run(planner_request, events.append)
    assert len(model.calls) == 2
    assert CONTROL in str(raised.value.errors[0])
    assert LATE in str(raised.value.errors[1])
    diagnostics = failure_tags(raised.value)['failure_message']
    assert CONTROL in diagnostics and LATE in diagnostics
    assert not any(event['event'] == 'reply' for event in events)


@pytest.mark.parametrize('first', [{'kind': 'not-a-plan'}, {**LEAF, 'forecast': {'fits': True}}])
def test_request156_shares_the_single_format_correction(planner_request, request156, monkeypatch, first):
    from tl3_workers import runtime
    from tl3_workers.turns import InvalidReply

    model = planner_model(monkeypatch, first, request156, LEAF)
    with pytest.raises(InvalidReply) as raised:
        runtime.run(planner_request, lambda _: None)
    assert CONTROL in str(raised.value.errors[1])
    assert len(model.calls) == 2


@pytest.mark.parametrize('obstacle', ['goal', 'protected', 'unsupported', 'proof_scope'])
def test_request156_corrected_reply_is_fully_rechecked(planner_request, request156, monkeypatch, obstacle):
    from tl3_workers import runtime

    fixed = copy.deepcopy(request156)
    fixed['checks'][0]['node_ids'].append(CONTROL)
    if obstacle == 'goal':
        fixed['checks'][0]['goal_ids'] = [0]
    elif obstacle == 'protected':
        fixed['files'].append('conftest.py')
    elif obstacle == 'unsupported':
        requirement = copy.deepcopy(planner_request['inputs']['ingest']['evidence_contract'][0])
        requirement['executor'] = 'buildbuddy_remote_bazel'
        fixed['evidence_contract'] = [requirement]
    else:
        planner_request['inputs']['tool_capabilities'][0]['methods'].append('subject_observer')
        fixed['checks'][0]['proof'] = {
            'subjects': ['undeclared.py'], 'observer_support': [], 'red_message': 'old subject'}
    model = planner_model(monkeypatch, request156, fixed, LEAF)
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 2
    result = events[-1]['reply']
    assert result['kind'] == 'cant_meet' if obstacle == 'unsupported' else result['violations']


@pytest.mark.parametrize('obstacle', ['cant_meet', 'unsupported', 'protected', 'out_of_room', 'forcing', 'proof_scope'])
def test_request156_substantive_stops_do_not_get_consistency_correction(planner_request, request156, monkeypatch, obstacle):
    from tl3_workers import runtime

    if obstacle == 'cant_meet':
        request156 = {**CANT_MEET, 'evidence_contract': []}
    elif obstacle == 'unsupported':
        planner_request['inputs']['tool_capabilities'] = []
    elif obstacle == 'protected':
        request156['files'].append('conftest.py')
    elif obstacle == 'out_of_room':
        planner_request['inputs']['must_split'] = True
    elif obstacle == 'forcing':
        request156['checks'][0]['forcing'] = {
            'symptom': 'Delay submission.', 'environment': {'OMP_NUM_THREADS': '1'}, 'patches': []}
    else:
        planner_request['inputs']['tool_capabilities'][0]['methods'].append('subject_observer')
        request156['checks'][0]['proof'] = {
            'subjects': ['undeclared.py'], 'observer_support': [], 'red_message': 'old subject'}
    model = planner_model(monkeypatch, request156, LEAF)
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 1
    result = events[-1]['reply']
    if obstacle in {'cant_meet', 'unsupported'}:
        assert result['kind'] == 'cant_meet'
        if obstacle == 'unsupported':
            assert result['evidence_errors']
    else:
        assert result['violations']
        assert any(CONTROL in violation and 'no demonstrating check' in violation
                   for violation in result['violations'])


def test_request156_trace_diagnostics_redact_model_text():
    from tl3_workers.roles.planner import PlanConsistencyError
    from tl3_workers.tracing import failure_tags
    from tl3_workers.turns import InvalidReply

    secret = 'sk-proj-' + 'aB7cD9eF2gH4jK6mN8pQ0rS3tU5vW1xY'
    error = PlanConsistencyError([CONTROL + '[api_key=' + secret + ']'])
    tags = failure_tags(InvalidReply(error, error))
    assert secret not in json.dumps(tags)
    assert CONTROL in tags['failure_message']
    assert 'repaired reply: planner check inconsistency:' in tags['failure_message']


@pytest.mark.parametrize('resolution', ['base_nodes', 'cant_meet', 'still_invented'])
def test_request162_forced_check_gets_the_same_single_correction(planner_request, request162, monkeypatch, resolution):
    from tl3_workers import runtime
    from tl3_workers.turns import InvalidReply

    fixed = copy.deepcopy(request162)
    if resolution == 'base_nodes':
        fixed['checks'][0]['node_ids'] = [CREATED]
    elif resolution == 'cant_meet':
        fixed = copy.deepcopy(CANT_MEET)
        fixed['report'] = 'The existing base tests cannot honestly demonstrate the required mutant and deterministic goals.'
    model = planner_model(monkeypatch, request162, fixed, LEAF)
    events = []
    if resolution == 'still_invented':
        with pytest.raises(InvalidReply) as raised:
            runtime.run(planner_request, events.append)
        assert all(node in str(raised.value.errors[1]) for node in INVENTED)
        assert not any(event['event'] == 'reply' for event in events)
    else:
        runtime.run(planner_request, events.append)
        result = events[-1]['reply']
        assert result['kind'] == fixed['kind']
        if resolution == 'base_nodes':
            assert result['violations'] and all('matching forcing rehearsal receipt' in v for v in result['violations'])
            assert result['checks'][0]['node_ids'] == [CREATED]
            assert result['checks'][0]['goal_ids'] == [0, 1]
            assert result['checks'][1] == request162['checks'][1]
            assert result['checks'][0]['forcing'] == request162['checks'][0]['forcing']
        else:
            assert result['violations'] == []
            assert result['report'] == fixed['report']
    assert len(model.calls) == 2
    history, offered = model.calls[1]
    assert not offered
    assert all(node in history[-1].text for node in INVENTED)
    assert 'Existing check must name a test defined at the base' in history[-1].text
    inputs = json.loads(next(m.text for m in history if isinstance(m, HumanMessage)))
    assert inputs['ingest']['goals'] == planner_request['inputs']['ingest']['goals']


def test_request162_ordinary_missing_regression_still_stops(planner_request, request162, monkeypatch):
    from tl3_workers import runtime

    # Only the forced demonstrating mismatch is locally repairable.
    request162['files'].append('greeting.py')
    request162['checks'][0].pop('forcing')
    request162['checks'][0]['kind'] = 'regression'
    request162.pop('edits_tests')
    model = planner_model(monkeypatch, request162, LEAF)
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 1
    assert all(any(node in v for v in events[-1]['reply']['violations']) for node in INVENTED)


@pytest.mark.parametrize('obstacle', ['protected', 'forcing', 'out_of_room'])
def test_request162_substantive_stops_bypass_correction(planner_request, request162, monkeypatch, obstacle):
    from tl3_workers import runtime

    if obstacle == 'protected':
        request162['files'].append('conftest.py')
    elif obstacle == 'forcing':
        request162['checks'][0]['forcing']['environment'] = {'OMP_NUM_THREADS': '1'}
    else:
        planner_request['inputs']['must_split'] = True
    model = planner_model(monkeypatch, request162, LEAF)
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 1
    assert all(any(node in v for v in events[-1]['reply']['violations']) for node in INVENTED)


def test_request162_correction_cannot_drop_mutant_coverage(planner_request, request162, monkeypatch):
    from tl3_workers import runtime

    fixed = copy.deepcopy(request162)
    fixed['checks'][0]['node_ids'] = [CREATED]
    fixed['checks'][0]['goal_ids'] = [0, 2]
    model = planner_model(monkeypatch, request162, fixed)
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 2
    assert any('Required goals have no check coverage: goal 1:' in v for v in events[-1]['reply']['violations'])


@pytest.mark.parametrize('resolution', ['base_node', 'cant_meet', 'still_invented', 'drop_tier_goal'])
def test_request165_preserves_deterministic_tier_and_rate_goals(planner_request, request165, monkeypatch, resolution):
    from tl3_workers import runtime
    from tl3_workers.turns import InvalidReply

    fixed = copy.deepcopy(request165)
    fixed['checks'][0]['node_ids'] = [FINISHED]
    if resolution == 'cant_meet':
        fixed = copy.deepcopy(CANT_MEET)
        fixed['evidence_contract'] = []
        fixed['report'] = 'No existing base test can honestly demonstrate all the deterministic tier/rate goals.'
    elif resolution == 'still_invented':
        fixed = copy.deepcopy(request165)
    elif resolution == 'drop_tier_goal':
        fixed['checks'][0]['goal_ids'].remove(5)
    model = planner_model(monkeypatch, request165, fixed, LEAF)
    events = []
    if resolution == 'still_invented':
        with pytest.raises(InvalidReply) as raised:
            runtime.run(planner_request, events.append)
        assert all(node in str(raised.value.errors[1]) for node in PROPOSED_EXPORTS)
        assert not any(event['event'] == 'reply' for event in events)
    else:
        runtime.run(planner_request, events.append)
        result = events[-1]['reply']
        if resolution == 'drop_tier_goal':
            assert any('Required goals have no check coverage: goal 5:' in v for v in result['violations'])
        else:
            assert result['kind'] == fixed['kind']
            if resolution == 'base_node':
                assert result['violations'] and all('matching forcing rehearsal receipt' in v for v in result['violations'])
                assert result['checks'] == [{**request165['checks'][0], 'node_ids': [FINISHED]}]
                assert result['edits_tests'] == [FINISHED]
                assert result['checks'][0]['forcing']['environment'] == {}
            else:
                assert result['violations'] == []
                assert result['report'] == fixed['report']
            assert result['evidence_errors'] == []
    assert len(model.calls) == 2
    history, offered = model.calls[1]
    assert not offered
    assert all(node in history[-1].text for node in PROPOSED_EXPORTS)
    inputs = json.loads(next(m.text for m in history if isinstance(m, HumanMessage)))
    assert inputs['ingest']['goals'] == planner_request['inputs']['ingest']['goals']
    assert inputs['ingest']['evidence_errors'] == []


@pytest.mark.parametrize('shape', ['request156', 'request162', 'request165'])
@pytest.mark.parametrize('enabled', [None, False])
def test_consistency_correction_defaults_off(planner_request, request, monkeypatch, shape, enabled):
    from tl3_workers import runtime

    draft = request.getfixturevalue(shape)
    if enabled is None:
        planner_request['inputs'].pop('repair_plan_consistency', None)
    else:
        planner_request['inputs']['repair_plan_consistency'] = enabled
    fixed = copy.deepcopy(draft)
    if shape == 'request156':
        fixed['checks'][0]['node_ids'].append(CONTROL)
    else:
        fixed['checks'][0]['node_ids'] = draft['checks'][0]['node_ids'][:1]
    model = planner_model(monkeypatch, draft, fixed)
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 1, 'default-off must keep the original PlanCantMeet classification'
    assert events[-1]['reply']['violations']


@pytest.mark.parametrize('expansion', ['file', 'check', 'forcing', 'goal', 'edit'])
def test_correction_cannot_expand_draft(planner_request, request156, monkeypatch, expansion):
    from tl3_workers import runtime

    fixed = copy.deepcopy(request156)
    fixed['checks'][0]['node_ids'].append(CONTROL)
    if expansion == 'file':
        fixed['files'].append('tests/test_existing.py')
    elif expansion == 'check':
        fixed['checks'].append({'proof': {'subjects': [], 'observer_support': [], 'red_message': 'expected regression'}, 'kind': 'demonstrating', 'node_ids': [f'{PBWAIT_TESTS}::test_unrelated'],
                                'goal_ids': [0]})
    elif expansion == 'forcing':
        fixed['checks'][0]['forcing'] = {'symptom': 'A new condition.', 'environment': {},
                                         'patches': [{'target': 'shim.DELAY', 'value': 1}]}
    elif expansion == 'goal':
        fixed['checks'][0]['goal_ids'] = [0, 1, 2]
    else:
        unrelated = f'{PBWAIT_TESTS}::test_unrelated'
        with Path(planner_request['worktree'], PBWAIT_TESTS).open('a') as stream:
            stream.write('\ndef test_unrelated():\n    assert True\n')
        fixed['edits_tests'].append(unrelated)
        fixed['checks'][0]['node_ids'].append(unrelated)
    model = planner_model(monkeypatch, request156, fixed, LEAF)
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 2
    violations = events[-1]['reply']['violations']
    assert violations, 'a coherent correction must not authorize a wider draft'
    if expansion == 'file':
        assert any('Correction expands the draft footprint: tests/test_existing.py' in v for v in violations)


@pytest.mark.parametrize('forcing_change', ['new', 'same', 'target', 'value', 'environment'])
def test_appended_coverage_check_cannot_introduce_forcing(planner_request, request156, monkeypatch, forcing_change):
    from tl3_workers import runtime

    forcing = {'symptom': 'Delay daemon replies.', 'environment': {},
               'patches': [{'target': 'shim.DELAY', 'value': 1}]}
    if forcing_change != 'new':
        request156['checks'][0]['forcing'] = copy.deepcopy(forcing)
    fixed = copy.deepcopy(request156)
    if forcing_change == 'target':
        forcing['patches'][0]['target'] = 'shim.OTHER_DELAY'
    elif forcing_change == 'value':
        forcing['patches'][0]['value'] = 2
    elif forcing_change == 'environment':
        forcing['environment'] = {'SHIM_DELAY': '1'}
    fixed['checks'].append({'proof': {'subjects': [], 'observer_support': [], 'red_message': 'expected regression'}, 'kind': 'demonstrating', 'node_ids': [CONTROL], 'goal_ids': [1],
                            'forcing': forcing})
    model = planner_model(monkeypatch, request156, fixed, LEAF)
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 2
    assert not model.calls[1][1]
    result = events[-1]['reply']
    if forcing_change == 'same':
        assert result['violations'] and all('matching forcing rehearsal receipt' in v for v in result['violations'])
        assert result['checks'] == fixed['checks']
    else:
        assert 'Correction adds a forcing absent from the draft.' in result['violations']


@pytest.mark.parametrize('fix_consistency', [True, False])
def test_missing_exploration_and_consistency_share_one_format_repair(
        planner_request, request156, monkeypatch, fix_consistency):
    from tl3_workers import runtime
    from tl3_workers.turns import InvalidReply

    missing = copy.deepcopy(request156)
    del missing['estimated_exploration_turns']
    corrected = copy.deepcopy(request156)
    if fix_consistency:
        corrected['checks'][0]['node_ids'].append(CONTROL)
    model = planner_model(monkeypatch, missing, corrected, LEAF)
    events = []
    if fix_consistency:
        runtime.run(planner_request, events.append)
        assert events[-1]['reply']['kind'] == 'leaf'
        assert events[-1]['reply']['violations'] == []
    else:
        with pytest.raises(InvalidReply) as error:
            runtime.run(planner_request, events.append)
        assert CONTROL in str(error.value.errors[1])
        assert not any(event['event'] == 'reply' for event in events)
    assert [offered for _, offered in model.calls] == [True, False]
    assert 'required' in model.calls[1][0][-1].text
    assert 'estimated_exploration_turns' in model.calls[1][0][-1].text


@pytest.fixture
def coverage_case(planner_request, monkeypatch):
    """PB #2710's saved goals/checks; only repository contents are synthetic."""
    fixture = json.loads((Path(__file__).parent / 'fixtures/planner-pb2710.json').read_text())
    root = Path(planner_request['worktree'])
    path = 'tests/test_checkout_recovery_1465.py'
    (root / path).write_text(
        'def _git(): pass\n'
        'def test_missing_or_conflicting_action_snapshot_and_ending_proof_keeps_original():\n'
        '    assert True\n')
    for file in fixture['files']:
        if file != path:
            (root / file).parent.mkdir(parents=True, exist_ok=True)
            (root / file).write_text('')
    monkeypatch.chdir(root)
    planner_request['inputs']['repair_plan_consistency'] = True
    planner_request['inputs']['ingest']['goals'] = fixture['goals']
    draft = {**copy.deepcopy(LEAF), 'files': fixture['files'], 'checks': fixture['checks']}
    fixed = copy.deepcopy(draft)
    fixed['checks'][1]['goal_ids'].append(3)
    return fixture, draft, fixed


def test_pb2710_names_only_uncovered_required_goal(planner_request, coverage_case):
    from tl3_workers.roles.planner import check_reply

    fixture, draft, _ = coverage_case
    planner_request['inputs']['repair_plan_consistency'] = False
    check_reply(planner_request['inputs'], draft)
    assert draft['violations'] == [fixture['violation'], 'Planner coverage correction attempts: 0.']
    assert 'goal 6:' not in draft['violations'][0]


@pytest.mark.parametrize('resolution', ['citation', 'cant_meet'])
def test_pb2710_one_tool_free_coverage_correction(planner_request, coverage_case, monkeypatch, resolution):
    from tl3_workers import runtime

    fixture, draft, fixed = coverage_case
    if resolution == 'cant_meet':
        fixed = copy.deepcopy(CANT_MEET)
    model = planner_model(monkeypatch, draft, fixed)
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 2, 'coverage must reach the existing single repair boundary'
    history, tools = model.calls[1]
    assert not tools and 'Tools are unavailable' in history[-1].text
    assert fixture['violation'] in history[-1].text
    assert fixture['repair_instruction'] in history[-1].text
    assert sum(event['event'] == 'reply' for event in events) == 1
    assert events[-1]['reply']['violations'] == []
    assert events[-1]['reply']['kind'] == fixed['kind']
    if resolution == 'citation':
        assert events[-1]['reply']['checks'] == fixed['checks']


def test_pb2710_format_and_coverage_share_one_repair(planner_request, coverage_case, monkeypatch):
    from tl3_workers import runtime

    _, draft, fixed = coverage_case
    model = planner_model(monkeypatch, {'kind': 'invalid'}, draft, fixed)
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 2
    assert 'goal 3:' in '\n'.join(events[-1]['reply']['violations'])
    assert 'Planner coverage correction attempts: 1.' in events[-1]['reply']['violations']


@pytest.mark.parametrize('second_goal', [3, 2])
def test_pb2710_exhaustion_keeps_both_named_goals(planner_request, coverage_case, monkeypatch, second_goal):
    from tl3_workers import runtime

    fixture, draft, fixed = coverage_case
    if second_goal != 3:
        fixed['checks'][1]['goal_ids'].remove(second_goal)
    else:
        fixed = copy.deepcopy(draft)
    model = planner_model(monkeypatch, draft, fixed, LEAF)
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 2
    violations = events[-1]['reply']['violations']
    assert 'Planner coverage correction attempts: 1.' in violations
    assert fixture['violation'] in violations
    assert f'goal {second_goal}:' in '\n'.join(violations)
    assert planner_request['inputs']['ingest']['goals'][second_goal]['description'] in '\n'.join(violations)
    assert sum(event['event'] == 'reply' for event in events) == 1


@pytest.mark.parametrize('change', ['scope', 'selector', 'kind', 'subjects', 'observers', 'message',
                                    'forcing', 'symptom', 'citations', 'other_goal', 'new_check',
                                    'edits', 'estimate', 'evidence', 'goal_text'])
def test_pb2710_correction_freezes_every_other_declaration(planner_request, coverage_case, monkeypatch, change):
    from tl3_workers import runtime

    _, draft, fixed = coverage_case
    check = fixed['checks'][0]
    if change == 'scope':
        fixed['files'].append('other.py')
    elif change == 'selector':
        check['node_ids'].append('tests/test_checkout_recovery_1465.py::test_other')
    elif change == 'kind':
        fixed['checks'][1]['kind'] = 'behavior-preserving'
    elif change in {'subjects', 'observers', 'message'}:
        field = {'subjects': 'subjects', 'observers': 'observer_support', 'message': 'red_message'}[change]
        check['proof'][field] = 'other assertion' if change == 'message' else ['tests/test_checkout_recovery_1465.py']
    elif change == 'forcing':
        check['forcing'] = {'symptom': 'Another condition.', 'environment': {},
                            'patches': [{'target': 'shim.DELAY', 'value': 1}]}
    elif change == 'symptom':
        check.pop('proof')
        check['declared_symptom'] = {'node': check['node_ids'][0], 'exception': 'builtins.RuntimeError',
                                     'phase': 'call', 'repeat': 2, 'parallel': 0}
    elif change == 'citations':
        # Goal 1 remains covered elsewhere, so the full recheck alone cannot catch this.
        check['goal_ids'].remove(1)
    elif change == 'other_goal':
        check['goal_ids'].append(2)
    elif change == 'new_check':
        fixed['checks'].append(copy.deepcopy(fixed['checks'][1]))
    elif change == 'edits':
        fixed['edits_tests'] = []
    elif change == 'estimate':
        fixed['estimated_tool_turns'] = 2
    elif change == 'evidence':
        fixed['evidence_contract'] = []
    else:
        from tl3_workers.roles.planner import PlanConsistencyError, check_reply
        with pytest.raises(PlanConsistencyError) as caught:
            check_reply(planner_request['inputs'], draft)
        fixed['goals'] = [{'description': 'Weakened goal text.'}]
        fixed['violations'] = []
        caught.value.constrain(fixed)
        assert fixed['violations']
        return
    model = planner_model(monkeypatch, draft, fixed)
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 2
    assert events[-1]['reply']['violations'], 'coverage repair must only add the missing citations'


@pytest.mark.parametrize('obstacle', ['protected', 'budget', 'proof', 'disabled'])
def test_pb2710_substantive_and_proof_refusals_stay_terminal(planner_request, coverage_case, monkeypatch, obstacle):
    from tl3_workers import runtime

    fixture, draft, fixed = coverage_case
    if obstacle == 'protected':
        draft['files'].append('conftest.py')
    elif obstacle == 'budget':
        draft['estimated_tool_turns'] = 1000
    elif obstacle == 'proof':
        from tl3_workers.roles.planner import check_reply
        draft['checks'][0]['proof']['red_message'] = ''
        check_reply(planner_request['inputs'], draft)
        assert fixture['violation'] in draft['violations']
        assert any('proof' in violation.lower() for violation in draft['violations'])
        return
    else:
        planner_request['inputs']['repair_plan_consistency'] = False
    model = planner_model(monkeypatch, draft, fixed)
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 1
    assert fixture['violation'] in events[-1]['reply']['violations']


@pytest.mark.parametrize('omission', ['proof', 'subject', 'selector', 'edit'])
def test_pb2710_coverage_composes_with_existing_defects(planner_request, coverage_case, monkeypatch, omission):
    from tl3_workers import runtime

    _, draft, fixed = coverage_case
    if omission == 'proof':
        draft['checks'][0]['proof'].pop('red_message')
    elif omission == 'subject':
        planner_request['inputs']['ingest']['goals'][0]['proof_obligation'] = {
            'kind': 'assertion', 'observer': 'new',
            'subjects': ['tests/test_checkout_recovery_1465.py::_git'],
            'source': 'issue.body', 'quote': 'Keep assertion proof on the real _git helper.'}
        draft['checks'][0]['proof']['subjects'] = []
    elif omission == 'selector':
        draft['checks'][1]['node_ids'] = ['tests/test_existing.py']
    else:
        node = draft['checks'][1]['node_ids'][0]
        # It is an uncovered edit rather than a frozen regression selection.
        draft['checks'].pop(1)
        fixed = copy.deepcopy(draft)
        draft['edits_tests'] = [node]
        fixed['edits_tests'] = [node]
        fixed['checks'][0]['node_ids'].append(node)
        fixed['checks'][0]['goal_ids'] = list(range(6))
    model = planner_model(monkeypatch, draft, fixed)
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 2, events[-1]['reply']['violations']
    assert events[-1]['reply']['violations'] == []


@pytest.mark.parametrize('too_many_ids', [False, True])
def test_pb2710_message_clips_descriptions_never_ids(planner_request, coverage_case, monkeypatch, too_many_ids):
    from tl3_workers import runtime
    from tl3_workers.turns import REPAIR_MESSAGE_BYTES, repair_message
    from tl3_workers.roles.planner import PlanConsistencyError, check_reply

    _, draft, fixed = coverage_case
    goals = planner_request['inputs']['ingest']['goals']
    if too_many_ids:
        goals.extend({'description': 'Uncovered review goal.', 'verification': 'review'} for _ in range(400))
        model = planner_model(monkeypatch, draft, fixed)
        events = []
        runtime.run(planner_request, events.append)
        assert len(model.calls) == 1
        violations = events[-1]['reply']['violations']
        assert 'Planner declarations exceed the existing correction message limit.' in violations
        assert 'goal 406:' in '\n'.join(violations)
    else:
        goals[3]['description'] = 'é' * 3000
        goals.append({'description': '界' * 3000, 'verification': 'review'})
        with pytest.raises(PlanConsistencyError) as caught:
            check_reply(planner_request['inputs'], draft)
        error = caught.value
        message = repair_message(error)
        assert len(message.encode()) <= REPAIR_MESSAGE_BYTES
        assert 'goal 3:' in message and 'goal 7:' in message
        assert message == repair_message(error)
        assert 'é' * 3000 in error.violations[0] and '界' * 3000 in error.violations[0]
        assert ' [...]' in message
        # Equal byte caps per description, rounded down to complete UTF-8 characters.
        assert abs(message.count('é') * 2 - message.count('界') * 3) <= 2


def test_pb2710_clipped_prompt_keeps_full_stop_evidence(planner_request, coverage_case, monkeypatch):
    from tl3_workers import runtime
    from tl3_workers.turns import REPAIR_MESSAGE_BYTES, REPAIR

    _, draft, _ = coverage_case
    description = 'é' * 3000
    planner_request['inputs']['ingest']['goals'][3]['description'] = description
    model = planner_model(monkeypatch, draft, draft)
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 2
    prompt = model.calls[1][0][-1].text
    assert len(prompt.encode()) <= REPAIR_MESSAGE_BYTES + len(REPAIR.format('').encode())
    assert 'goal 3:' in prompt and description not in prompt
    violations = events[-1]['reply']['violations']
    assert description in '\n'.join(violations)
    assert 'Planner coverage correction attempts: 1.' in violations


def test_pb2710_no_allowance_records_zero_coverage_attempts(planner_request, coverage_case, monkeypatch):
    from tl3_workers import runtime
    from tl3_workers.turns import RepairUnavailable, TurnControl

    fixture, draft, fixed = coverage_case
    model = planner_model(monkeypatch, draft, fixed)

    def no_room(self, history, error):
        raise RepairUnavailable()

    monkeypatch.setattr(TurnControl, 'repair', no_room)
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 1
    assert fixture['violation'] in events[-1]['reply']['violations']
    assert 'Planner coverage correction attempts: 0.' in events[-1]['reply']['violations']
    assert any('input allowance has no room' in v for v in events[-1]['reply']['violations'])
