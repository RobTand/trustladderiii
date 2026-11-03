"""Planner frozen tests contracts."""
# ruff: noqa: F811

import copy
from pathlib import Path

import pytest

from tl3_workers.contracts import validate
from planner_fixtures import (
    CANT_MEET,
    DIGESTS,
    FORCING,
    FROZEN,
    LEAF,
    NAME,
    SIBLING,
    cache_tokenizer,  # noqa: F401
    digests_plan,  # noqa: F401
    planned,
    planner_request,  # noqa: F401
    run_reply,
    tool_capable_fake,  # noqa: F401
)


def test_goal_that_edits_a_frozen_test_is_a_plan_defect(planner_request, digests_plan):
    violations = planned(planner_request, digests_plan)
    assert any('Goal 0 names `test_digests_imports_only_the_standard_library`' in item
               and 'check-3' in item and 'edits_tests' in item for item in violations), violations


def test_goal_naming_a_frozen_test_in_an_undeclared_file_is_not_flagged(planner_request, digests_plan):
    # The maker can't edit an undeclared file, so freezing its tests is right.
    digests_plan['files'] = ['digests.py']
    digests_plan['checks'][0]['node_ids'] = ['tests/test_lazy.py::test_lazy_imports_run']
    digests_plan['files'].append('tests/test_lazy.py')
    assert planned(planner_request, digests_plan) == []


@pytest.mark.parametrize('frozen', [FROZEN, f'{SIBLING}[1]'])
@pytest.mark.parametrize('kind', ['regression', 'behavior-preserving'])
def test_declared_edit_may_not_be_frozen(planner_request, digests_plan, frozen, kind):
    edited = frozen.split('[')[0]
    digests_plan['edits_tests'] = [edited]
    digests_plan['checks'] = [
        {'kind': 'demonstrating', 'node_ids': [edited], 'goal_ids': [0, 1]},
        {'kind': kind, 'node_ids': [frozen], 'goal_ids': [0, 1]},
    ]
    violations = planned(planner_request, digests_plan)
    assert any(f'`{edited}` is in edits_tests' in item and 'only in a demonstrating check' in item
               for item in violations), violations


def test_declared_edit_needs_a_demonstrating_check(planner_request, digests_plan):
    digests_plan['edits_tests'] = [FROZEN]
    digests_plan['checks'] = digests_plan['checks'][:2]
    digests_plan['checks'][0]['goal_ids'] = [0]
    violations = planned(planner_request, digests_plan)
    assert any(f'`{FROZEN}` is in edits_tests but no demonstrating check names it' in item
               for item in violations), violations


@pytest.mark.parametrize('edited,files,reason', [
    ('tests/test_digests.py', ['digests.py', DIGESTS], 'edits_tests must name exact pytest node IDs.'),
    (f'{DIGESTS}::test_new', ['digests.py', DIGESTS],
     f'edits_tests must name a test defined at the base: `{DIGESTS}::test_new`.'),
    (FROZEN, ['digests.py'], f'A test the change edits must be in a declared file: `{DIGESTS}`.'),
])
def test_declared_edits_name_existing_tests_in_declared_files(planner_request, digests_plan, edited, files, reason):
    digests_plan['files'] = files
    digests_plan['edits_tests'] = [edited]
    digests_plan['checks'] = [{'kind': 'demonstrating', 'node_ids': [FROZEN], 'goal_ids': [0, 1]}]
    violations = planned(planner_request, digests_plan)
    assert any(item.startswith(reason) for item in violations), violations


def test_corrected_plan_declares_its_edit(tmp_path, planner_request, digests_plan):
    digests_plan['edits_tests'] = [FROZEN]
    digests_plan['checks'] = [
        {'kind': 'demonstrating', 'node_ids': [FROZEN], 'goal_ids': [0]},
        {'kind': 'regression', 'node_ids': [SIBLING], 'goal_ids': [1]},
    ]
    assert planned(planner_request, copy.deepcopy(digests_plan)) == []
    result = run_reply(tmp_path, planner_request, digests_plan)
    assert result.exit_code == 0, result.stderr
    reply = result.events[-1]['reply']
    assert reply['edits_tests'] == [FROZEN]
    assert reply['violations'] == []


def test_test_only_observer_edits_use_the_source_partition(planner_request, digests_plan):
    # Existing selected assertions are observers. Checking judges their RED.
    digests_plan['files'] = [DIGESTS]
    digests_plan['edits_tests'] = [FROZEN]
    digests_plan['checks'] = [
        {'kind': 'demonstrating', 'node_ids': [FROZEN], 'goal_ids': [0]},
        {'kind': 'regression', 'node_ids': [SIBLING], 'goal_ids': [1]},
    ]
    violations = planned(planner_request, digests_plan)
    assert violations == [], violations
    assert not any('#393' in item for item in violations), violations


def test_test_only_change_with_a_forcing_still_needs_rehearsal(planner_request, digests_plan):
    # Q55 forcing still supports test-only changes, but now needs plan-time execution.
    digests_plan['files'] = [DIGESTS]
    digests_plan['edits_tests'] = [FROZEN]
    digests_plan['checks'] = [
        {'kind': 'demonstrating', 'node_ids': [FROZEN], 'goal_ids': [0], 'forcing': copy.deepcopy(FORCING)},
        {'kind': 'regression', 'node_ids': [SIBLING], 'goal_ids': [1]},
    ]
    validate('planner-reply', digests_plan)
    assert any('matching forcing rehearsal receipt' in v
               for v in planned(planner_request, copy.deepcopy(digests_plan)))
    # A second assertion observer does not waive the forced check rehearsal.
    digests_plan['checks'].append({'kind': 'demonstrating', 'node_ids': [SIBLING], 'goal_ids': [0]})
    assert any('matching forcing rehearsal receipt' in item
               for item in planned(planner_request, copy.deepcopy(digests_plan)))
    digests_plan['checks'].pop()
    digests_plan['checks'][0]['forcing']['patches'][0] = {'target': 'digests.f', 'raises': 'AssertionError'}
    violations = planned(planner_request, digests_plan)
    assert any(item.startswith('Invalid forcing:') for item in violations), violations


def test_plan_schema_allows_edits_tests_only_on_a_leaf():
    validate('planner-reply', {**LEAF, 'edits_tests': [FROZEN]})
    with pytest.raises(Exception):
        validate('planner-reply', {**LEAF, 'edits_tests': [FROZEN, FROZEN]})
    with pytest.raises(Exception):
        validate('planner-reply', {**CANT_MEET, 'edits_tests': [FROZEN]})


def test_prompt_states_edits_tests_rules():
    # #438: a plan that freezes a test its goals edit burns every maker round.
    prompt = " ".join((Path(__file__).parents[1] / "src/tl3_workers/prompts/planner.md").read_text().split())
    assert "edits_tests" in prompt
    assert "never in a regression or behavior-preserving check" in prompt
    assert "naming only tests the change doesn't edit" in prompt
    assert "If the change edits only tests" in prompt and "#393" in prompt
    assert "to extend, patch, update, fix or rewrite it" in prompt


@pytest.mark.parametrize('goal', [
    f'Fix the bug. `{NAME}` must still pass unchanged.',
    f'Do not edit {NAME}.',
    f"Don't change `{NAME}`; add new tests beside it.",
    f'Keep {NAME} green.',
    f'Fix the bug so {NAME} passes.',
    f'Fix the bug, keeping {NAME} green.',
    f'`tests/{NAME}.py` passes.',
    f'Update the docs. Leave `{NAME}` unchanged.',
])
def test_goal_that_keeps_a_frozen_test_is_accepted(planner_request, digests_plan, goal):
    # #444 review: rule 7 fires only when an edit verb governs the name.
    planner_request['inputs']['ingest']['goals'][0]['description'] = goal
    assert planned(planner_request, digests_plan) == []


@pytest.mark.parametrize('goal', [
    f'Extend `{NAME}` so the module force-executes its lazy imports.',
    f'Add a case to {NAME} for the new profile.',
    f'Add a not-interchangeable pair to `{NAME}` showing the new profile differs.',
    f'Update the import assertion in `{NAME}`.',
    f'Patch {NAME} to load the module standalone.',
    f'Rewrite {DIGESTS}::{NAME} without the sleep.',
])
def test_goal_that_edits_a_frozen_test_is_refused(planner_request, digests_plan, goal):
    planner_request['inputs']['ingest']['goals'][0]['description'] = goal
    violations = planned(planner_request, digests_plan)
    assert any(f'Goal 0 names `{NAME}`' in item for item in violations), violations


def test_test_file_location_does_not_invent_a_proof_obligation(planner_request, digests_plan):
    # PQ #3070 and Tessera #1532: a deflake goal names the flaky test without an edit verb.
    planner_request['inputs']['ingest']['goals'][0]['description'] = (
        f'Identify the cause of the intermittent failure of {NAME} and reproduce it deterministically.')
    digests_plan['files'] = [DIGESTS]
    digests_plan['checks'] = [{'kind': 'regression', 'node_ids': [FROZEN, SIBLING], 'goal_ids': [0, 1]}]
    violations = planned(planner_request, copy.deepcopy(digests_plan))
    assert violations == [], violations
    digests_plan['files'] = ['digests.py', DIGESTS]
    assert planned(planner_request, digests_plan) == []


def test_new_test_observer_can_demonstrate_a_test_file_subject(planner_request, digests_plan):
    # Checking grafts only this observer, preserving all other base definitions.
    digests_plan['files'] = [DIGESTS, 'tests/test_lazy.py']
    digests_plan['checks'] = [
        {'kind': 'demonstrating', 'node_ids': ['tests/test_lazy.py::test_lazy_imports_run'], 'goal_ids': [0]},
        {'kind': 'regression', 'node_ids': [SIBLING], 'goal_ids': [1]},
    ]
    violations = planned(planner_request, digests_plan)
    assert violations == [], violations


def test_test_only_regression_plan_is_accepted(planner_request, digests_plan):
    # A test refactor guarded by existing tests can pass; only shapes that can't are refused.
    planner_request['inputs']['ingest']['goals'][0]['description'] = 'Share the digest fixtures.'
    digests_plan['files'] = [DIGESTS]
    digests_plan['checks'] = [{'kind': 'regression', 'node_ids': [FROZEN, SIBLING], 'goal_ids': [0, 1]}]
    assert planned(planner_request, digests_plan) == []


def test_publication_obligations_have_guards_instead_of_pretend_tests(tmp_path, planner_request):
    planner_request['inputs']['ingest']['goals'].append({
        'description': 'Publish a non-draft PR with recorded independent proof.',
        'verification': 'review', 'stage': 'publishing',
        'publication': {'non_draft': True, 'body_lines': ['Change type: test'], 'proof': True},
    })
    result = run_reply(tmp_path, planner_request, LEAF)
    assert result.exit_code == 0, result.stderr
    assert result.events[-1]['reply']['violations'] == []
