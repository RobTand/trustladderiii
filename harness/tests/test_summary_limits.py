"""Summary bounding must make progress or refuse, with a short hang backstop."""
import json
from pathlib import Path
import subprocess
import sys

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


def test_summary_exits_when_truncation_note_would_grow_the_envelope():
    result = test_tool.Summary().result
    result['tests'] = [{'test': str(i), 'outcome': 'passed', 'message': ''} for i in range(3)]
    result['totals']['passed'] = 3
    observed = bounded(result, len(json.dumps(result).encode()) - 1)
    assert observed['status'] == 'infrastructure_error', observed
    assert observed['execution']['termination_reason'] == 'identity_exceeds_summary_limit'
    assert observed['totals']['passed'] == 3


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
