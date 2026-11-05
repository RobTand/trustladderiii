"""The Python oracle stays byte-identical while P2b consumes the same cases."""
import pytest

from planner_vectors import SOURCES, load, replay, strings

DOCUMENT = load()


def test_corpus_covers_every_source():
    assert DOCUMENT['cases'], 'planner parity corpus is empty; generate on BuildBuddy'
    sources = {case['source'] for case in DOCUMENT['cases']}
    assert set(SOURCES) <= sources, f'missing corpus sources: {set(SOURCES) - sources}'
    assert any(case['operation'] == 'constrain' for case in DOCUMENT['cases'])
    assert {True, False} <= {case['outcome']['error']['local'] for case in DOCUMENT['cases']
                            if case['outcome'].get('error', {}).get('class', '').endswith('.PlanConsistencyError')}
    assert strings(DOCUMENT)


@pytest.mark.parametrize('case', DOCUMENT['cases'], ids=lambda case: case['id'])
def test_python_replays_planner_case(case):
    replay(case)
