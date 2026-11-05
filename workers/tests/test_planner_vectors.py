"""The Python oracle stays byte-identical while P2b consumes the same cases."""
import pytest

from planner_vectors import SOURCES, load, replay, strings, rule_coverage, diagnostic_rules, packed, unpacked, mismatch_fields

DOCUMENT = load()


def test_corpus_covers_every_source():
    assert DOCUMENT['cases'], 'planner parity corpus is empty; generate on BuildBuddy'
    assert DOCUMENT['source_exit'] == 0
    assert set(DOCUMENT['scenarios'].values()) == {'passed'}
    sources = {case['source'] for case in DOCUMENT['cases']}
    assert set(SOURCES) <= sources, f'missing corpus sources: {set(SOURCES) - sources}'
    assert any(case['operation'] == 'constrain' for case in DOCUMENT['cases'])
    assert {True, False} <= {case['outcome']['error']['local'] for case in DOCUMENT['cases']
                            if case['outcome'].get('error', {}).get('class', '').endswith('.PlanConsistencyError')}
    assert strings(DOCUMENT)


@pytest.mark.parametrize('case', DOCUMENT['cases'], ids=lambda case: case['id'])
def test_python_replays_planner_case(case):
    replay(case)


def test_every_diagnostic_template_has_a_case():
    missing = [rule for rule, cases in rule_coverage(DOCUMENT).items() if not cases]
    assert not missing, missing


def test_inventory_includes_subscript_conflict_class_and_constructor_diagnostics():
    rules = diagnostic_rules()
    for module, message in (
        ('planner', 'Correction must remain a leaf or honestly report cant_meet.'),
        ('generated_scope', 'Invented generated-output prohibition in ingest:'),
        ('generated_scope', 'Remove the generated-output prohibition:'),
        ('evidence', 'Return evidence_contract with source requirements and separately marked method choices.'),
    ):
        assert any(rule.startswith(module + ':') and message in rule for rule in rules), (module, message)


def test_repair_limit_cases_cover_both_sides_in_bytes():
    cases = [case for case in DOCUMENT['cases'] if 'repair_boundary' in case]
    assert {(case['repair_boundary']['bytes'], case['repair_boundary']['characters'] < case['repair_boundary']['bytes'])
            for case in cases} == {(size, unicode) for size in (2047, 2048, 2049) for unicode in (False, True)}
    for case in cases:
        boundary = case['repair_boundary']
        from tl3_workers.roles.planner import PlanConsistencyError
        mismatch = PlanConsistencyError([], draft=case['reply'], selectors=unpacked(boundary['selectors']))
        assert len(str(mismatch).encode()) == boundary['bytes']
        assert len(str(mismatch)) == boundary['characters']
        if boundary['bytes'] <= 2048:
            assert case['outcome']['error']['class'].endswith('.PlanConsistencyError')
        else:
            assert 'Planner declarations exceed the existing correction message limit.' in case['outcome']['reply']['violations']


def test_lower_observations_include_lifecycle_failures():
    rows = [row for case in DOCUMENT['cases'] for row in case['facts']]
    calls = {row['call'] for row in rows}
    assert {'git', 'test_function_source', 'Worktree.open', 'Worktree.enter', 'Worktree.file',
            'Worktree.file.open', 'Worktree.file.read', 'Worktree.file.close', 'Worktree.close'} <= calls
    assert {'git', 'Worktree.open', 'Worktree.file.open', 'Worktree.file.read',
            'Worktree.file.close', 'Worktree.close'} <= {row['call'] for row in rows if 'error' in row}
    assert 'base_definition' not in calls


def test_local_correction_retains_current_base_sha_asymmetry():
    cases = [case for case in DOCUMENT['cases'] if case['id'].startswith('extra/local-base-sha-asymmetry#')]
    base, check, correction = cases
    assert unpacked(base['outcome']['return']) == (False, False)
    assert check['inputs']['base_sha'] == 'immutable-base'
    assert check['outcome']['error']['class'].endswith('.PlanConsistencyError')
    assert unpacked(correction['outcome']['return']) == []
    assert any(row['call'] == 'git' for row in base['facts'])
    assert any(row['call'] == 'Worktree.file.read' for row in correction['facts'])
    assert not any(row['call'] == 'git' for row in correction['facts'])


def test_mismatch_capture_includes_every_constructor_defect():
    for case in DOCUMENT['cases']:
        value = case.get('mismatch', case['outcome'].get('error', {}))
        if value.get('class', '').endswith('.PlanConsistencyError'):
            assert set(unpacked(value['mismatch'])) == set(mismatch_fields())


def test_goal_id_oracle_contains_int_float_and_bool_inputs():
    kinds = {type(goal).__name__ for case in DOCUMENT['cases'] if case['operation'] == 'check_reply'
             for check in case['reply'].get('checks', []) for goal in check['goal_ids']}
    assert {'int', 'float', 'bool'} <= kinds


@pytest.mark.parametrize('case', DOCUMENT.get('english', []), ids=lambda case: case['id'])
def test_english_heuristics(case):
    from tl3_workers.roles.planner import mentions, asks_to_edit
    found = list(mentions(case['description'], case['test']))
    assert packed(found) == case['mentions']
    assert any(asks_to_edit(*mention) for mention in found) == case['verdict']


@pytest.mark.parametrize('case', DOCUMENT.get('globs', []), ids=lambda case: case['id'])
def test_protected_glob_oracle(case):
    from pathlib import PurePosixPath
    assert PurePosixPath(case['path']).full_match(case['pattern']) == case['verdict']
