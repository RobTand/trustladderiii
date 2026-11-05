# ruff: noqa: F811

"""Completed selector-heavy runs retain exact receipt identity (#704)."""
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import sys

import pytest
from pb_fixtures import submission as submission  # noqa: F401
from pb_fixtures import supported_fake_runtime as supported_fake_runtime
from tl3_harness import execution, test_tool
from tl3_harness.model import Forcing
from tl3_workers.contracts import validate
from tl3_workers.pb_tests import TestSettings, make_test_tool
from tl3_workers.tools import Evidence, Worktree, TEST_EVIDENCE_BYTES
from worker_process import IMAGE, KEY, reachable


def identity(selected):
    raw = json.dumps(selected, ensure_ascii=True, separators=(',', ':')).encode()
    return {'schema': 'tl3.selectors.v1', 'count': len(selected),
            'sha256': hashlib.sha256(raw).hexdigest(),
            'prefix': [test[:120] for test in selected[:3]]}


def selectors():
    return sorted(f'test_many.py::test_value[{i}_' + 'x' * 1200 + ']' for i in range(16))


def invoke(submission, response, selected, forcing=None):
    root, scripts = submission
    (scripts / 'response.json').write_text(json.dumps(response))
    with Worktree(root) as tree:
        return make_test_tool(tree, TestSettings(IMAGE, scripts), forcing=forcing)(selected)


def test_completed_parametrized_file_keeps_digest_identity(submission, monkeypatch):
    root, _ = submission
    selected = selectors()
    ids = [test.split('[', 1)[1][:-1] for test in selected]
    (root / 'test_many.py').write_text(
        'import pytest\n' + f'@pytest.mark.parametrize("value", range(16), ids={ids!r})\n'
        + 'def test_value(value):\n    assert value >= 0\n')
    monkeypatch.setattr(test_tool, 'require_admission', lambda: None)
    monkeypatch.setenv('PRISMABUILD_ACTION_PROGRESS_PATH', str(root / 'progress.json'))
    monkeypatch.setenv('PRISMABUILD_ACTION_PROGRESS_TOKEN', 'fixture')
    monkeypatch.setattr(execution.time, 'time', lambda: 1000.0)
    monkeypatch.setattr(test_tool, 'container_command', lambda tree, container, tests: [
        sys.executable, '-I', '-B', str(Path(execution.__file__).with_name('runner.py')), '-', str(tree)])
    response = test_tool.run(root, execution.Container(IMAGE), ('test_many.py',))
    assert response['status'] == 'completed' and response['totals']['passed'] == 16
    response['selectors'] = selected
    response = test_tool.bounded(response)
    result = invoke(submission, response, selected)
    assert result['status'] == 'completed', result
    assert result['selectors'] == identity(selected)
    assert result['totals']['passed'] == 16
    assert len(json.dumps(result).encode()) <= test_tool.SUMMARY_BYTES
    assert result['receipts'][0]['action_key'] == KEY


@pytest.mark.parametrize('damage', ['matching', 'digest', 'count', 'order', 'prefix', 'omitted', 'extra'])
def test_receipt_compares_complete_digest_identity(submission, damage):
    selected = selectors()
    response = test_tool.Summary().result
    response['totals']['passed'] = len(selected)
    response.update(selectors=identity(selected), forcing=reachable())
    if damage == 'digest':
        response['selectors']['sha256'] = 'f' * 64
    elif damage == 'count':
        response['selectors']['count'] += 1
    elif damage == 'order':
        response['selectors'] = identity(list(reversed(selected)))
    elif damage == 'prefix':
        response['selectors']['prefix'][0] = 'other.py::test_other'
    elif damage == 'omitted':
        response['selectors'] = identity(selected[:-1])
    elif damage == 'extra':
        response['selectors']['untrusted'] = True
    result = invoke(submission, response, selected, Forcing.parse(reachable()))
    if damage == 'matching':
        assert result['status'] == 'completed', result
    else:
        assert result['status'] == 'infrastructure_error', result
        assert result['execution']['termination_reason'] == 'forcing_or_selection_mismatch'


def test_small_summary_keeps_full_selector_list(submission):
    selected = ['test_many.py::test_value']
    response = test_tool.Summary().result
    response.update(selectors=selected, forcing=reachable())
    original = deepcopy(response)
    assert test_tool.bounded(response) == original
    result = invoke(submission, response, selected, Forcing.parse(reachable()))
    assert result['status'] == 'completed'
    assert result['selectors'] == selected


def test_reviewer_evidence_keeps_digest_of_omitted_selectors():
    selected = selectors()
    evidence = Evidence()
    evidence.test_result({'status': 'completed', 'totals': {'passed': 16, 'failed': 0, 'error': 0, 'skipped': 0},
                          'tree': 'e' * 40, 'receipts': [{'action_key': KEY, 'status': 'executed',
                          'exit_code': 0, 'receipt_sha256': 'b' * 64, 'log_sha256': 'c' * 64}]},
                         (False, tuple(selected)))
    runs = evidence.test_evidence()
    assert runs[0]['tests_identity'] == identity(selected)
    assert runs[0]['tests_omitted'] > 0
    assert len(json.dumps(runs).encode()) <= TEST_EVIDENCE_BYTES
    validate('maker-reply', {'kind': 'done', 'head': 'f' * 40,
                            'self_check': [{'goal_id': 'g1', 'status': 'met', 'evidence': 'receipt'}],
                            'test_evidence': runs})


@pytest.mark.parametrize('damage', ['matching', 'digest', 'count', 'omitted'])
def test_maker_final_check_uses_complete_identity(tmp_path, monkeypatch, damage):
    from types import SimpleNamespace
    from tl3_workers.roles import maker
    selected = selectors()
    tests_identity = identity(selected)
    if damage == 'digest':
        tests_identity['sha256'] = 'f' * 64
    elif damage == 'count':
        tests_identity['count'] += 1
    elif damage == 'omitted':
        tests_identity = identity(selected[:-1])
    entry = {'tests': [], 'tests_omitted': len(selected), 'tests_identity': tests_identity,
             'base': False, 'status': 'completed', 'tree': 'e' * 40,
             'totals': {'passed': 16, 'failed': 0, 'error': 0, 'skipped': 0}}
    monkeypatch.setattr(maker, '_git', lambda request, *args, **kwargs: SimpleNamespace(
        stdout='' if args[0] == 'diff' else 'e' * 40))
    worktree = SimpleNamespace(generator=None, evidence=SimpleNamespace(test_evidence=lambda: [entry]))
    remaining = {'tool_turns': {'used': 1, 'allowed': 1}, 'input_tokens': {'used': 1, 'allowed': 1},
                 'no_progress_window': 1, 'no_progress_turns': 0, 'last_progress_kind': 'test'}
    request = {'operation': {'node': 1, 'generation': 0, 'kind': 'make'},
               'inputs': {'goals': [], 'plan': {'checks': [{'name': 'all nodes', 'tests': selected}]}}}
    reply = maker._budget_reply(request, SimpleNamespace(remaining=lambda: remaining, exhaustion='tool_turns',
                                                        compaction_count=0, context_fraction=0.0),
                               [], worktree, {'kind': 'done'})
    unfinished = reply['budget_exhaustion']['unfinished_obligations']
    assert ('Unverified final check: all nodes' in unfinished) is (damage != 'matching')
