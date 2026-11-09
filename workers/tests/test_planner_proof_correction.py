"""Proof declaration omissions share the existing bounded planner correction (#694)."""
# ruff: noqa: F811
import copy

import pytest

from planner_fixtures import (
    LEAF, cache_tokenizer, planner_model, planner_request, tool_capable_fake,  # noqa: F401
)


@pytest.fixture
def proof_plan(planner_request, monkeypatch):
    monkeypatch.chdir(planner_request['worktree'])
    planner_request['inputs']['repair_plan_consistency'] = True
    return copy.deepcopy(LEAF)


def omit(draft, missing):
    if missing == 'proof':
        draft['checks'][0].pop('proof')
    else:
        for key in missing.split(','):
            draft['checks'][0]['proof'].pop(key)


@pytest.mark.parametrize('missing', ['proof', 'red_message', 'subjects', 'observer_support',
                                     'subjects,observer_support'])
def test_proof_omissions_get_one_tool_free_correction(planner_request, proof_plan, monkeypatch, missing):
    from tl3_workers import runtime

    fixed = copy.deepcopy(proof_plan)
    omit(proof_plan, missing)
    model = planner_model(monkeypatch, proof_plan, fixed)
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 2
    history, tools = model.calls[-1]
    assert not tools and 'Tools are unavailable' in history[-1].text
    assert '/checks/0' in history[-1].text
    assert bool(events[-1]['reply']['violations']) == (missing in {'proof', 'red_message'})
    if missing in {'proof', 'red_message'}:
        assert any(item['rule'] == 'guard.fields_changed' for item in events[-1]['plan_check']['objections'])
    assert events[-1]['reply']['checks'] == fixed['checks']


@pytest.mark.parametrize('change', ['subjects', 'observer_support', 'kind', 'goal', 'scope', 'selectors'])
def test_proof_correction_only_fills_missing_fields(planner_request, proof_plan, monkeypatch, change):
    from tl3_workers import runtime

    fixed = copy.deepcopy(proof_plan)
    omit(proof_plan, 'red_message')
    if change in {'subjects', 'observer_support'}:
        fixed['checks'][0]['proof'][change] = ['greeting.py']
    elif change == 'kind':
        fixed['checks'][2]['kind'] = 'behavior-preserving'
    elif change == 'goal':
        fixed['checks'][0]['goal_ids'] = [0, 1]
    elif change == 'scope':
        fixed['files'].append('another.py')
    else:
        fixed['checks'][0]['tests'] = ['tests/test_greeting.py::test_other']
    model = planner_model(monkeypatch, proof_plan, fixed)
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 2
    assert events[-1]['reply']['violations'], 'omission repair cannot redesign the proof or the plan'


@pytest.mark.parametrize('missing', ['proof', 'red_message', 'subjects'])
def test_proof_omission_with_substantive_failure_stops(planner_request, proof_plan, monkeypatch, missing):
    from tl3_workers import runtime

    omit(proof_plan, missing)
    proof_plan['estimated_tool_turns'] = 1000
    model = planner_model(monkeypatch, proof_plan, LEAF)
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 2
    assert events[-1]['plan_check']
    assert events[-1]['reply']['violations']


def test_invalid_proof_value_stays_a_substantive_refusal(planner_request, proof_plan, monkeypatch):
    proof_plan['checks'][0]['proof']['red_message'] = ''
    # Exercise semantic validation, not the transport's pre-existing format repair.
    from tl3_workers.roles.planner import check_reply
    from tl3_workers.roles.planner import PlanConsistencyError
    with pytest.raises(PlanConsistencyError) as error:
        check_reply(planner_request['inputs'], proof_plan)
    assert any(item.rule == 'proof.red_missing' for item in error.value.objections)
    assert proof_plan['violations']
    assert 'proof.red_message' in '\n'.join(proof_plan['violations'])
