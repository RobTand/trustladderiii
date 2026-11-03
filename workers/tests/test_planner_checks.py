"""Planner checks contracts."""
# ruff: noqa: F811

import copy
import json
from pathlib import Path

import pytest
from jsonschema import Draft202012Validator, ValidationError

from tl3_workers.contracts import validate
from planner_fixtures import (
    CANT_MEET,
    LEAF,
    SPLIT,
    assert_rule_report,
    cache_tokenizer,  # noqa: F401
    planner_model,
    planner_request,  # noqa: F401
    run_reply,
    tool_capable_fake,  # noqa: F401
)


@pytest.mark.parametrize("vector", json.loads((Path(__file__).resolve().parents[2] / "harness/test-vectors/pytest-node-ids.json").read_text()), ids=lambda v: repr(v["node_id"]))
def test_planner_exact_node_ids_agree_with_harness(vector, planner_request, monkeypatch):
    from tl3_workers.roles import planner
    from tl3_workers.forecast import Forecaster
    monkeypatch.setattr(Forecaster, "measure", lambda *a: {"fits": True})
    reply = copy.deepcopy(LEAF)
    for check in reply["checks"]:
        check["node_ids"] = [vector["node_id"]]
    try:
        planner.check_reply(planner_request["inputs"], reply)
        valid = "Checks must name exact pytest node IDs." not in reply["violations"]
    except ValueError:
        valid = False
    assert valid == vector["valid"]


@pytest.mark.parametrize('proposal', [LEAF, SPLIT, CANT_MEET])
def test_planner_schema_follower_succeeds_without_code_only_fields(
        tmp_path, planner_request, monkeypatch, proposal):
    from planner_fixtures import FORGED_FORECAST
    from worker_process import fill_visible_fields

    fill_visible_fields(monkeypatch, {'forecast': FORGED_FORECAST, 'violations': []})
    result = run_reply(tmp_path, planner_request, proposal)
    assert result.exit_code == 0, result.stderr
    actual = result.events[-1]['reply']
    assert actual['forecast']['fit_tokens'] == planner_request['inputs']['forecast']['fit_tokens']
    assert {key: value for key, value in actual.items()
            if key not in {'forecast', 'execution', 'violations'}} == proposal
    assert actual['execution']['allowed_tool_turns'] == 64
    assert actual['execution']['usable_tool_turns'] == 56
    assert bool(actual['violations']) == (proposal['kind'] == 'split')


@pytest.mark.parametrize('kind', ['regression', 'behavior-preserving'])
@pytest.mark.parametrize('missing', ['tests/absent.py::test_absent',
                                    'tests/test_sequences.py::test_absent',
                                    'tests/test_sequences.py::TestAbsent::test_absent'])
def test_planner_checks_base_test_definitions(planner_request, monkeypatch, kind, missing):
    from tl3_workers.roles.planner import check_reply
    from tl3_workers.forecast import Forecaster

    root = Path(planner_request['worktree'])
    (root / 'tests').mkdir(exist_ok=True)
    source = (Path(__file__).parents[2] / 'harness/tests/fixtures/chunked_test.py').read_text()
    (root / 'tests/test_sequences.py').write_text(source)
    monkeypatch.chdir(root)
    monkeypatch.setattr(Forecaster, 'measure', lambda *a: {'fits': True})
    reply = copy.deepcopy(LEAF)
    reply['checks'] = [{'kind': kind, 'node_ids': [missing], 'goal_ids': [0, 1]}]
    check_reply(planner_request['inputs'], reply)
    assert any(missing in item and 'base' in item for item in reply['violations'])
    reply = copy.deepcopy(LEAF)
    reply['checks'] = [{'kind': kind,
        'node_ids': ['tests/test_sequences.py::test_chunked_rejects_non_positive_sizes'],
        'goal_ids': [0, 1]}]
    check_reply(planner_request['inputs'], reply)
    assert reply['violations'] == []
    reply = copy.deepcopy(LEAF)
    reply['checks'] = [{'kind': 'demonstrating', 'node_ids': [missing], 'goal_ids': [0, 1]}]
    check_reply(planner_request['inputs'], reply)
    assert reply['violations'] == []


def test_new_unique_test_is_a_planner_defect(tmp_path, planner_request):
    root = Path(planner_request['worktree'])
    (root / 'tests').mkdir(exist_ok=True)
    (root / 'tests/test_sequences.py').write_text('def test_unique_preserves_order(): assert True\n')
    test = 'tests/test_sequences.py::test_unique_does_not_mutate_input'
    proposal = copy.deepcopy(LEAF)
    proposal['checks'] = [{'kind': 'regression', 'node_ids': [test], 'goal_ids': [0, 1]}]
    result = run_reply(tmp_path, planner_request, proposal)
    assert_rule_report(result, test)
    assert 'base' in result.events[-1]['reply']['violations'][0]


def test_prompt_states_how_existing_tests_are_named():
    # The planner gets one retry; a rule it was never told about spends it.
    prompt = " ".join((Path(__file__).parents[1] / "src/tl3_workers/prompts/planner.md").read_text().split())
    assert "must name tests that already exist in the base" in prompt
    assert "without its parameter suffix" in prompt
    assert "A test file path on its own is not a node ID" in prompt


def test_prompt_requires_deterministic_reproduction_for_determinism_work():
    # #340: a flaky test can pass on the base, so it cannot demonstrate a determinism fix.
    prompt = " ".join((Path(__file__).parents[1] / "src/tl3_workers/prompts/planner.md").read_text().split())
    assert "must name a test that reproduces the race deterministically" in prompt
    assert "a flaky test can pass on the base" in prompt


def test_prompt_states_forced_reproduction_for_deflakes():
    # #393, Rob 2026-11-02: a forced reproduction is the deflake red; stress runs don't count.
    prompt = " ".join((Path(__file__).parents[1] / "src/tl3_workers/prompts/planner.md").read_text().split())
    assert "forcing" in prompt and "forced reproduction" in prompt
    assert "fails by assertion on the base under the forcing" in prompt
    assert "passes on the change both with and without the forcing" in prompt
    assert "Repeating a test until it fails does not count" in prompt
    assert "A change to tests alone needs a forced reproduction (Q55, #393)" in prompt


def test_missing_exploration_estimate_gets_one_tool_free_format_repair(planner_request, monkeypatch):
    from tl3_workers import runtime

    monkeypatch.chdir(planner_request['worktree'])
    missing = copy.deepcopy(LEAF)
    del missing['estimated_exploration_turns']
    model = planner_model(monkeypatch, missing, LEAF)
    events = []
    runtime.run(planner_request, events.append)
    reply = events[-1]['reply']

    assert [offered for _, offered in model.calls] == [True, False]
    correction = model.calls[1][0][-1].text
    assert 'estimated_exploration_turns' in correction
    assert 'required' in correction
    prompt = model.calls[0][0][0].text.split('It must match this schema:\n', 1)[1]
    sent_schema, _ = json.JSONDecoder().raw_decode(prompt)
    with pytest.raises(ValidationError, match='estimated_exploration_turns'):
        Draft202012Validator(sent_schema).validate(missing)
    assert reply['kind'] == 'leaf' and reply['violations'] == []
    assert reply['execution']['exploration_declared'] is True
    assert len([event for event in events if event['event'] == 'reply']) == 1


@pytest.mark.parametrize('where', ['leaf', 'split', 'piece'])
def test_exploration_estimate_is_required_at_the_worker_format_boundary(where):
    from tl3_workers import runtime
    from tl3_workers.roles import planner

    proposal = copy.deepcopy(LEAF if where == 'leaf' else SPLIT)
    target = proposal['pieces'][0] if where == 'piece' else proposal
    del target['estimated_exploration_turns']
    # A worker from the previous release may still finish after a deployment (#372).
    validate('planner-reply', proposal)
    with pytest.raises(ValidationError, match='estimated_exploration_turns') as error:
        runtime._checked(planner.ROLE, {}, proposal)
    assert error.value.validator == 'required'
