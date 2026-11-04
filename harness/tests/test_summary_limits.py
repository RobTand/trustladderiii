"""Summary bounding must make progress or refuse, with a short hang backstop."""
import json
from pathlib import Path
import subprocess
import sys

import pytest

from tl3_harness import test_tool


CHILD = '''
import faulthandler
import json
import sys
faulthandler.dump_traceback_later(2, exit=True)
sys.path.insert(0, sys.argv[1])
from tl3_harness.test_tool import bounded
request = json.load(sys.stdin)
result = bounded(request['result'], limit=request['limit'])
faulthandler.cancel_dump_traceback_later()
print(json.dumps(result))
'''


def bounded(result, limit):
    child = subprocess.run(
        [sys.executable, '-I', '-c', CHILD, str(Path(test_tool.__file__).parents[1])],
        input=json.dumps({'result': result, 'limit': limit}), text=True,
        capture_output=True, timeout=5)
    assert child.returncode == 0, child.stderr
    return json.loads(child.stdout)


def test_truncation_note_does_not_prevent_shrinking_the_envelope():
    result = test_tool.Summary().result
    result['tests'] = [{'test': str(i), 'outcome': 'passed', 'message': ''} for i in range(3)]
    result['totals']['passed'] = 3
    limit = len(json.dumps(result).encode()) - 1
    observed = bounded(result, limit)
    assert observed['status'] == 'completed', observed
    assert observed['totals']['passed'] == 3
    assert len(observed['tests']) < 3
    assert len(json.dumps(observed).encode()) <= limit
    assert 'Truncated' in observed['note']


def test_digest_that_still_cannot_fit_exits_without_looping():
    result = test_tool.Summary().result
    result.update(selectors=['test_many.py::test_' + 'x' * 2000], immutable='x' * 2000)
    observed = bounded(result, 1000)
    assert observed['status'] == 'infrastructure_error', observed
    assert observed['execution']['termination_reason'] == 'identity_exceeds_summary_limit'


def test_digest_then_entry_removal_terminates_with_exact_identity():
    from tl3_harness.selector_identity import selector_identity
    selected = ['test_many.py::test_' + 'x' * 2000]
    result = test_tool.Summary().result
    result.update(selectors=selected, tests=[{'test': selected[0], 'outcome': 'passed', 'message': ''}])
    result['totals']['passed'] = 1
    observed = bounded(result, 1000)
    assert observed['status'] == 'completed', observed
    assert observed['selectors'] == selector_identity(selected)
    assert observed['totals']['passed'] == 1
    assert len(json.dumps(observed).encode()) <= 1000


@pytest.mark.parametrize('note', ['', 'Each group runs with its own declared source partition and execution settings.'])
def test_error_dominated_group_removes_failures_but_keeps_totals_and_proof(note):
    module = 'tests/test_checkout_recovery_1465.py'
    proof = {'rule': 'subject-observer-v1', 'mode': 'pytest-outcome',
             'declaration': None, 'source_base_tree_sha256': 'a' * 64,
             'base_tree_sha256': 'b' * 64, 'head_tree_sha256': 'c' * 64}
    control = test_tool.Summary().result
    control.update(check='planned observer', proof={**proof, 'mode': 'head-observer'},
                   tests=[{'test': module + '::test_git_config', 'outcome': 'failed',
                           'message': 'maintenance and gc must be disabled'}])
    control['totals']['failed'] = 1
    full_file = test_tool.Summary().result
    outcomes = ['passed'] * 25 + ['error'] * 267 + ['failed']
    full_file.update(check='unplanned base tests', proof=proof,
                     tests=[{'test': module + f'::test_checkout[{index}]', 'outcome': outcome,
                             'message': 'checkout recovery failure ' * 8 if outcome != 'passed' else ''}
                            for index, outcome in enumerate(outcomes)])
    full_file['totals'].update(passed=25, failed=1, error=267)
    result = test_tool.Summary().result
    result.update(groups=[control, full_file], totals={'passed': 25, 'failed': 2, 'error': 267, 'skipped': 0},
                  note=note)
    limit = test_tool.SUMMARY_BYTES
    assert len(json.dumps(result).encode()) > limit

    observed = bounded(result, limit)

    assert observed['status'] == 'completed', observed
    assert observed['totals'] == result['totals']
    assert len(observed['groups']) == 2
    for before, after in zip(result['groups'], observed['groups']):
        assert after['check'] == before['check']
        assert after['status'] == before['status']
        assert after['totals'] == before['totals']
        assert after['proof'] == before['proof']
    kept = observed['groups'][1]['tests']
    assert not any(test['outcome'] in ('passed', 'failed') for test in kept)
    assert sum(test['outcome'] == 'error' for test in kept) < 267
    assert len(json.dumps(observed).encode()) <= limit
    assert 'Truncated' in observed['note']
