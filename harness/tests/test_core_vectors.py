"""#749/#818: pin Python outputs, including documented wave B divergences."""
from copy import deepcopy
import json

import pytest

from core_vectors import DIVERGENCES, KINDS, ROOT, check, vectors


@pytest.mark.parametrize('kind', KINDS)
def test_core_vectors_match_python(kind):
    check(kind)


@pytest.mark.parametrize('case_id', DIVERGENCES)
def test_divergence_still_pins_python_output(case_id, tmp_path):
    committed = json.loads((ROOT / 'core/summary-bound.json').read_text())
    changed = deepcopy(committed)
    case = next(case for case in changed['cases'] if case['id'] == case_id)
    assert case['divergence'] == DIVERGENCES[case_id]
    case['expected']['json_bytes'] += 1
    (tmp_path / 'summary-bound.json').write_text(json.dumps(changed))
    with pytest.raises(AssertionError, match=f'summary-bound/{case_id}'):
        check('summary-bound', tmp_path)


def test_divergence_witnesses():
    cases = {case['id']: case for case in vectors('summary-bound')['cases']}
    oversized = cases['grouped-base-48-checks']
    assert len(oversized['input']['result']['groups']) >= 7
    assert oversized['expected']['json_bytes'] > oversized['input']['limit']
    priority = cases['demonstrating-red-before-unplanned-errors']
    before, after = priority['input']['result'], priority['expected']['result']
    assert any(test['test'] == priority['input']['red_test'] for test in before['groups'][0]['tests'])
    assert not after['groups'][0]['tests']
    assert any(test['outcome'] == 'error' for group in after['groups'] for test in group['tests'])
    assert after['totals'] == before['totals']
    for original, bounded in zip(before['groups'], after['groups']):
        assert bounded['proof'] == original['proof']
        assert bounded['totals'] == original['totals']


@pytest.mark.parametrize('mode', ['checking', 'summary'])
def test_finish_exit_three_is_refused_in_every_mode(mode):
    cases = {case['id']: case for case in vectors('finish-teardown')['cases']}
    expected = cases[f'{mode}-parse-3-matching']['expected']
    if mode == 'checking':
        assert expected == {'error': 'incomplete report or inconsistent process exit'}
    else:
        assert expected['status'] == 'infrastructure_error'
    for scenario in ('wrong-file', 'wrong-frame', 'wrong-type', 'subclass', 'wrong-message', 'system-exit'):
        assert cases[f'{mode}-finish-1-{scenario}']['expected'] == 3
    assert cases[f'{mode}-finish-1-xdist-thread']['expected'] == 1
