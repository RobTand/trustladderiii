"""PB #2722's omitted subject uses the existing single planner correction (#721)."""
# ruff: noqa: F811
import copy
from pathlib import Path

import pytest

from planner_fixtures import (
    LEAF, cache_tokenizer, planner_model, planner_request, tool_capable_fake,  # noqa: F401
)

FILE = 'tests/test_the_claim_pass_times_its_transition_holds.py'
SUBJECT = FILE + '::test_a_pass_that_raises_under_the_lock_still_times_the_hold'
OBSERVER = FILE + '::test_hold_time_is_recorded_after_raise'
KEPT = 'greeting.py::existing'


@pytest.fixture
def applicability_plan(planner_request, monkeypatch):
    root = Path(planner_request['worktree'])
    monkeypatch.chdir(root)
    (root / FILE).write_text('def test_a_pass_that_raises_under_the_lock_still_times_the_hold(): pass\n')
    (root / 'greeting.py').write_text('def existing(): pass\nclass Counter:\n    def hold(self): pass\n')
    inputs = planner_request['inputs']
    inputs['repair_plan_consistency'] = True
    inputs['ingest']['goals'] = [
        {'description': 'Record hold time after a pass raises.', 'verification': 'test'} for _ in range(9)]
    for index in (0, 8):
        inputs['ingest']['goals'][index]['proof_obligation'] = {
            'kind': 'assertion', 'observer': 'new', 'subjects': [SUBJECT],
            'source': 'issue.body', 'quote': 'A deterministic assertion must show the hold time on base.'}
    draft = copy.deepcopy(LEAF)
    draft['files'] = ['greeting.py', FILE]
    draft['estimated_tool_turns'] = 40
    draft['checks'] = [
        {'kind': 'regression', 'node_ids': ['tests/test_existing.py::test_existing'],
         'goal_ids': list(range(1, 8))},
        {'kind': 'demonstrating', 'node_ids': [OBSERVER], 'goal_ids': [0, 8],
         'proof': {'subjects': [KEPT], 'observer_support': [], 'red_message': 'hold time was not recorded'}},
    ]
    return draft


def corrected(draft, subject=SUBJECT):
    fixed = copy.deepcopy(draft)
    fixed['checks'][1]['proof']['subjects'].append(subject)
    return fixed


@pytest.mark.parametrize('cover', ['exact', 'module', 'definition'])
def test_pb2722_missing_subject_gets_one_tool_free_correction(
        planner_request, applicability_plan, monkeypatch, cover):
    from tl3_workers import runtime

    subject = SUBJECT
    if cover == 'definition':
        subject = 'greeting.py::Counter::hold'
        for index in (0, 8):
            planner_request['inputs']['ingest']['goals'][index]['proof_obligation']['subjects'] = [subject]
    chosen = {'exact': subject, 'module': FILE, 'definition': 'greeting.py::Counter'}[cover]
    fixed = corrected(applicability_plan, chosen)
    model = planner_model(monkeypatch, applicability_plan, fixed)
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 2, 'a pure missing subject must reach the existing correction'
    history, tools = model.calls[-1]
    assert not tools and 'Tools are unavailable' in history[-1].text
    assert 'check-2' in history[-1].text and subject in history[-1].text
    reply = events[-1]['reply']
    assert reply['violations'] == []
    assert reply['checks'] == fixed['checks']
    assert reply['files'] == fixed['files'] and reply['estimated_tool_turns'] == 40


@pytest.mark.parametrize('change', ['extra_subject', 'drop_subject', 'observers', 'message', 'kind',
                                    'goals', 'selectors', 'other_check', 'scope', 'estimate', 'forcing'])
def test_applicability_correction_only_adds_the_missing_subject(
        planner_request, applicability_plan, monkeypatch, change):
    from tl3_workers import runtime

    fixed = corrected(applicability_plan)
    check = fixed['checks'][1]
    if change == 'extra_subject':
        check['proof']['subjects'].append('greeting.py::unrelated')
    elif change == 'drop_subject':
        check['proof']['subjects'].remove(KEPT)
    elif change == 'observers':
        check['proof']['observer_support'] = ['greeting.py::Counter']
    elif change == 'message':
        check['proof']['red_message'] = 'a different assertion'
    elif change == 'kind':
        fixed['checks'][0]['kind'] = 'behavior-preserving'
    elif change == 'goals':
        check['goal_ids'].append(1)
    elif change == 'selectors':
        check['node_ids'] = [FILE + '::test_different_observer']
    elif change == 'other_check':
        fixed['checks'][0]['node_ids'] = ['tests/test_existing.py::test_api']
    elif change == 'scope':
        fixed['files'].append('another.py')
    elif change == 'estimate':
        fixed['estimated_tool_turns'] = 41
    else:
        check['forcing'] = {'symptom': 'A different condition.', 'environment': {'CASE': 'race'}, 'patches': []}
    model = planner_model(monkeypatch, applicability_plan, fixed)
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 2
    assert events[-1]['reply']['violations'], 'omission repair cannot change any other declaration'


@pytest.mark.parametrize('obstacle', ['wrong_method', 'old_observer', 'campaign', 'budget', 'disabled'])
def test_non_omission_applicability_and_substantive_failures_stop_immediately(
        planner_request, applicability_plan, monkeypatch, obstacle):
    from tl3_workers import runtime

    check = applicability_plan['checks'][1]
    if obstacle == 'wrong_method':
        check['kind'] = 'regression'
        check.pop('proof')
        check['node_ids'] = [SUBJECT]
    elif obstacle == 'old_observer':
        # The subject is still absent: that omission must not hide the wrong observer.
        check['node_ids'] = [SUBJECT]
    elif obstacle == 'campaign':
        check.pop('proof')
        check['node_ids'] = [SUBJECT]
        check['declared_symptom'] = {'node': SUBJECT, 'exception': 'builtins.RuntimeError',
                                     'phase': 'call', 'repeat': 2, 'parallel': 0}
    elif obstacle == 'budget':
        applicability_plan['estimated_tool_turns'] = 1000
    else:
        planner_request['inputs']['repair_plan_consistency'] = False
    model = planner_model(monkeypatch, applicability_plan, corrected(applicability_plan)
                          if 'proof' in check else LEAF)
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 1
    assert events[-1]['reply']['violations']


def test_unrelated_subject_does_not_fix_the_omission(planner_request, applicability_plan, monkeypatch):
    from tl3_workers import runtime
    from tl3_workers.turns import InvalidReply
    from tl3_workers.tracing import failure_tags

    model = planner_model(monkeypatch, applicability_plan, corrected(applicability_plan, 'greeting.py::unrelated'))
    with pytest.raises(InvalidReply) as caught:
        runtime.run(planner_request, lambda _: None)
    assert len(model.calls) == 2
    diagnostic = failure_tags(caught.value)['failure_message']
    assert SUBJECT in diagnostic and 'repaired reply' in diagnostic


@pytest.mark.parametrize('supported', [True, False])
def test_empty_subject_omission_preserves_capability_preflight(
        planner_request, applicability_plan, monkeypatch, supported):
    from tl3_workers import runtime

    applicability_plan['checks'][1]['proof']['subjects'] = []
    applicability_plan['evidence_contract'] = []
    capability = {'tool': 'run_tests', 'executor': 'prismabuild_pytest', 'runner_features': ['base_head'],
                  'artifacts': ['cas_receipt', 'terminal_verdict'], 'methods': ['subject_observer'],
                  'protected_files': []}
    planner_request['inputs']['tool_capabilities'] = [capability] if supported else []
    fixed = corrected(applicability_plan)
    model = planner_model(monkeypatch, applicability_plan, fixed)
    events = []
    runtime.run(planner_request, events.append)
    if supported:
        assert len(model.calls) == 2
        assert events[-1]['reply']['violations'] == []
        assert events[-1]['reply']['checks'] == fixed['checks']
        assert events[-1]['reply']['evidence_contract'][0]['methods'] == ['subject_observer']
    else:
        assert len(model.calls) == 1, 'unsupported required proof is substantive before correction'
        assert events[-1]['reply']['kind'] == 'cant_meet'
        assert 'Unsupported evidence requirement' in events[-1]['reply']['report']
