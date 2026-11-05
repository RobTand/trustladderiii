"""The Python oracle stays byte-identical while P2b consumes the same cases."""
import pytest

from planner_vectors import SOURCES, load, replay, strings, rule_coverage, packed

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
