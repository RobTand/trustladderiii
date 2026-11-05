"""Whole-file draft checks use the existing bounded consistency correction (#683)."""
# ruff: noqa: F811
import copy
import json
from pathlib import Path

import pytest
from langchain_core.messages import AIMessage, HumanMessage

from planner_fixtures import (
    LEAF, PlannerModel, cache_tokenizer, planner_request, tool_capable_fake,  # noqa: F401
)

FILE = 'tests/test_pbwait_summary_conflict.py'
SOURCE = '''import pytest

def test_conflict():
    assert True

@pytest.mark.parametrize('status', [0, 1])
def test_status(status):
    assert status in (0, 1)

class TestSummary:
    def test_receipt(self):
        assert True
'''
NODES = [FILE + suffix for suffix in ('::test_conflict', '::test_status', '::TestSummary::test_receipt')]


@pytest.fixture
def file_plan(planner_request, monkeypatch):
    root = Path(planner_request['worktree'])
    monkeypatch.chdir(root)
    (root / FILE).write_text(SOURCE)
    planner_request['inputs']['repair_plan_consistency'] = True
    tool = {'tool': 'run_tests', 'executor': 'prismabuild_pytest',
            'runner_features': ['base_head', 'native_threads_1'],
            'artifacts': ['logs', 'terminal_verdict', 'cas_receipt'],
            'methods': ['red_green'], 'protected_files': []}
    planner_request['inputs']['tool_capabilities'] = [tool]
    planner_request['inputs']['ingest']['evidence_contract'] = [
        {**copy.deepcopy(tool), 'requirement': 'Retain base/head proof, receipts and thread bounds.'}]
    draft = copy.deepcopy(LEAF)
    draft['evidence_contract'] = []
    draft['estimated_exploration_turns'] = 7
    draft['estimated_tool_turns'] = 46
    draft['checks'][2]['node_ids'] = [
        'tests/test_existing.py::test_api', 'tests/test_existing.py::test_existing', FILE]
    return draft


def model_for(monkeypatch, draft, fixed):
    from tl3_workers import runtime
    responses = []
    responses.extend(AIMessage(content=json.dumps(reply)) for reply in (draft, fixed, LEAF))
    model = PlannerModel(responses=responses, calls=[])
    monkeypatch.setattr(runtime, 'create_model', lambda _: model)
    return model


def corrected(draft):
    fixed = copy.deepcopy(draft)
    fixed['checks'][2]['node_ids'] = draft['checks'][2]['node_ids'][:-1] + NODES
    return fixed


def test_whole_file_gets_exact_tool_free_correction(planner_request, file_plan, monkeypatch):
    from tl3_workers import runtime
    model = model_for(monkeypatch, file_plan, corrected(file_plan))
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 2, 'whole-file syntax must reach the existing correction boundary'
    history, tools = model.calls[-1]
    assert not tools and isinstance(history[-1], HumanMessage)
    assert f'Replace `{FILE}` with the exact test definitions this check needs.' in history[-1].text
    assert 'Tools are unavailable' in history[-1].text
    assert not any(node in history[-1].text for node in NODES)
    result = events[-1]['reply']
    assert result['violations'] == []
    assert result['checks'] == corrected(file_plan)['checks']
    assert result['files'] == file_plan['files']
    assert result['estimated_tool_turns'] == 46
    assert result['estimated_exploration_turns'] == 7
    assert result['forecast']['fits'] and result['execution']['fits']
    assert result['evidence_contract'] == planner_request['inputs']['ingest'].get('evidence_contract', [])


@pytest.mark.parametrize('change', ['unknown', 'kind', 'goal', 'scope', 'estimate', 'other_check', 'proof', 'edits'])
def test_file_correction_cannot_change_obligations(planner_request, file_plan, monkeypatch, change):
    from tl3_workers import runtime
    fixed = corrected(file_plan)
    if change == 'unknown':
        fixed['checks'][2]['node_ids'] = [FILE + '::test_unknown']
    elif change == 'kind':
        fixed['checks'][2]['kind'] = 'behavior-preserving'
    elif change == 'goal':
        fixed['checks'][2]['goal_ids'] = [0]
    elif change == 'scope':
        fixed['files'].append('unrelated.py')
    elif change == 'estimate':
        fixed['estimated_tool_turns'] = 1
    elif change == 'other_check':
        fixed['checks'][1]['node_ids'] = ['tests/test_existing.py::test_existing']
    elif change == 'proof':
        fixed['checks'][0]['proof']['red_message'] = 'different message'
    else:
        fixed['edits_tests'] = []
    model = model_for(monkeypatch, file_plan, fixed)
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 2, 'validate the attempted correction, not just the original draft'
    assert events[-1]['reply']['violations']


def test_file_correction_exhaustion_records_selector(planner_request, file_plan, monkeypatch):
    from tl3_workers import runtime
    from tl3_workers.tracing import failure_tags
    from tl3_workers.turns import InvalidReply
    model = model_for(monkeypatch, file_plan, file_plan)
    events = []
    with pytest.raises(InvalidReply) as raised:
        runtime.run(planner_request, events.append)
    assert len(model.calls) == 2 and not any(event['event'] == 'reply' for event in events)
    message = failure_tags(raised.value)['failure_message']
    assert FILE in message and 'repaired reply' in message


@pytest.mark.parametrize('obstacle', ['budget', 'protected', 'must_split', 'disabled'])
def test_file_syntax_does_not_repair_substantive_refusals(planner_request, file_plan, monkeypatch, obstacle):
    from tl3_workers import runtime
    if obstacle == 'budget':
        file_plan['estimated_tool_turns'] = 1000
    elif obstacle == 'protected':
        file_plan['files'].append('conftest.py')
    elif obstacle == 'must_split':
        planner_request['inputs']['must_split'] = True
    elif obstacle == 'disabled':
        planner_request['inputs']['repair_plan_consistency'] = False

    model = model_for(monkeypatch, file_plan, corrected(file_plan))
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 1
    assert events[-1]['reply']['violations']


@pytest.mark.parametrize('selector', [FILE, 'tests/', FILE + '::'])
def test_nonexact_selectors_use_planner_chosen_base_nodes(planner_request, file_plan, monkeypatch, selector):
    from tl3_workers import runtime

    file_plan['checks'][2]['node_ids'][-1] = selector
    fixed = corrected(file_plan)
    # The planner chooses a subset; code makes no claim about whole-file collection.
    fixed['checks'][2]['node_ids'] = [NODES[0]]
    model = model_for(monkeypatch, file_plan, fixed)
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 2
    assert events[-1]['reply']['violations'] == []
    assert events[-1]['reply']['checks'] == fixed['checks']


@pytest.mark.parametrize('at_base', [True, False], ids=['missing-from-worktree', 'only-in-worktree'])
def test_selector_correction_uses_original_base(planner_request, file_plan, monkeypatch, at_base, request):
    import checker_stub
    from worker_process import git
    from tl3_workers import runtime

    # Reuse real Rust budget responses for the same three-check correction.
    budget_case = ('tests/test_planner_file_selectors.py::'
                   'test_nonexact_selectors_use_planner_chosen_base_nodes[' + FILE + ']')
    monkeypatch.setitem(checker_stub.cases(), request.node.nodeid,
                        copy.deepcopy(checker_stub.cases()[budget_case]))
    root = Path(planner_request['worktree'])
    if not at_base:
        (root / FILE).unlink()
    git(root, 'init', '-q')
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'original planner base')
    planner_request['inputs']['base_sha'] = git(root, 'rev-parse', 'HEAD')
    if at_base:
        (root / FILE).unlink()
    else:
        (root / FILE).write_text(SOURCE)
    # A demonstrating observer is allowed to be new. Only selector correction
    # requires its replacement to be an exact definition from the original base.
    file_plan['checks'][2].update(kind='demonstrating', proof={
        'subjects': [], 'observer_support': [], 'red_message': 'expected regression'})
    fixed = corrected(file_plan)
    fixed['checks'][2]['node_ids'] = [NODES[0]]
    model = model_for(monkeypatch, file_plan, fixed)
    events = []
    runtime.run(planner_request, events.append)

    assert len(model.calls) == 2
    expected = [] if at_base else [
        f'Corrected check must name a test defined at the base: {NODES[0]}']
    assert events[-1]['reply']['violations'] == expected
    assert events[-1]['reply']['checks'] == fixed['checks']
