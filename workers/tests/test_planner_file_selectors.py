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
    (root / 'pyproject.toml').write_text('[tool.tl3.harness]\ntest-files = ["tests/**/*.py"]\n')
    planner_request['inputs']['repair_plan_consistency'] = True
    draft = copy.deepcopy(LEAF)
    draft['estimated_exploration_turns'] = 7
    draft['estimated_tool_turns'] = 46
    draft['checks'][2]['node_ids'] = [
        'tests/test_existing.py::test_api', 'tests/test_existing.py::test_existing', FILE]
    return draft


def model_for(monkeypatch, draft, fixed, *, read=True):
    from tl3_workers import runtime
    responses = []
    if read:
        responses.append(AIMessage(content='', tool_calls=[
            {'name': 'read', 'args': {'path': FILE, 'limit': 200}, 'id': 'base-definitions'}]))
    responses.extend(AIMessage(content=json.dumps(reply)) for reply in (draft, fixed, LEAF))
    model = PlannerModel(responses=responses, calls=[])
    monkeypatch.setattr(runtime, 'create_model', lambda _: model)
    return model


def corrected(draft):
    fixed = copy.deepcopy(draft)
    fixed['checks'][2]['node_ids'] = draft['checks'][2]['node_ids'][:-1] + NODES
    return fixed


def test_whole_file_gets_complete_exact_tool_free_correction(planner_request, file_plan, monkeypatch):
    from tl3_workers import runtime
    model = model_for(monkeypatch, file_plan, corrected(file_plan))
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 3, 'whole-file syntax must reach the existing correction boundary'
    history, tools = model.calls[-1]
    assert not tools and isinstance(history[-1], HumanMessage)
    assert FILE in history[-1].text and 'Tools are unavailable' in history[-1].text
    result = events[-1]['reply']
    assert result['violations'] == []
    assert result['checks'] == corrected(file_plan)['checks']
    assert result['files'] == file_plan['files']
    assert result['estimated_tool_turns'] == 46
    assert result['estimated_exploration_turns'] == 7
    assert result['forecast']['fits'] and result['execution']['fits']
    assert result['evidence_contract'] == planner_request['inputs']['ingest'].get('evidence_contract', [])


@pytest.mark.parametrize('change', ['drop', 'extra', 'case_only', 'kind', 'goal', 'scope', 'estimate'])
def test_file_correction_cannot_change_obligations(planner_request, file_plan, monkeypatch, change):
    from tl3_workers import runtime
    fixed = corrected(file_plan)
    if change == 'drop':
        fixed['checks'][2]['node_ids'].remove(NODES[0])
    elif change == 'extra':
        fixed['checks'][2]['node_ids'].append('tests/test_existing.py::test_unrelated')
        Path('tests/test_existing.py').write_text(Path('tests/test_existing.py').read_text() +
                                                 'def test_unrelated(): assert True\n')
    elif change == 'case_only':
        fixed['checks'][2]['node_ids'][3] += '[0]'
    elif change == 'kind':
        fixed['checks'][2]['kind'] = 'demonstrating'
    elif change == 'goal':
        fixed['checks'][2]['goal_ids'] = [0]
    elif change == 'scope':
        fixed['files'].append('unrelated.py')
    else:
        fixed['estimated_tool_turns'] = 1
    model = model_for(monkeypatch, file_plan, fixed)
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 3, 'validate the attempted correction, not just the original draft'
    assert events[-1]['reply']['violations']


@pytest.mark.parametrize('source,reason', [
    (None, 'unavailable'),
    ('def test_a(): pass\ndef test_a(): pass\n', 'ambiguous'),
    ('for name in ["test_dynamic"]:\n    globals()[name] = lambda: None\n', 'dynamic'),
    ('from other import test_imported\n', 'dynamic'),
    ('class TestInherited(Unknown):\n    pass\n', 'dynamic'),
    ('#' * (16 * 1048576 + 1), 'limit'),
    ('def test_broken(:\n', 'unavailable'),
], ids=['missing', 'ambiguous', 'generated', 'imported', 'inherited', 'oversized', 'syntax'])
def test_file_discovery_refuses_without_guessing(planner_request, file_plan, monkeypatch, source, reason):
    from tl3_workers import runtime
    if source is None:
        Path(FILE).unlink()
    else:
        Path(FILE).write_text(source)
    model = model_for(monkeypatch, file_plan, corrected(file_plan))
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 2
    violations = '\n'.join(events[-1]['reply']['violations'])
    assert FILE in violations and reason in violations


def test_file_correction_requires_already_read_definitions(planner_request, file_plan, monkeypatch):
    from tl3_workers import runtime
    model = model_for(monkeypatch, file_plan, corrected(file_plan), read=False)
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 1
    assert any(FILE in item and 'already-read' in item for item in events[-1]['reply']['violations'])


def test_file_correction_exhaustion_records_selector(planner_request, file_plan, monkeypatch):
    from tl3_workers import runtime
    from tl3_workers.tracing import failure_tags
    from tl3_workers.turns import InvalidReply
    model = model_for(monkeypatch, file_plan, file_plan)
    events = []
    with pytest.raises(InvalidReply) as raised:
        runtime.run(planner_request, events.append)
    assert len(model.calls) == 3 and not any(event['event'] == 'reply' for event in events)
    message = failure_tags(raised.value)['failure_message']
    assert FILE in message and 'repaired reply' in message


@pytest.mark.parametrize('obstacle', ['budget', 'protected', 'must_split', 'disabled', 'demonstrating'])
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
    else:
        file_plan['checks'][2]['kind'] = 'demonstrating'
    model = model_for(monkeypatch, file_plan, corrected(file_plan))
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 2
    assert events[-1]['reply']['violations']
