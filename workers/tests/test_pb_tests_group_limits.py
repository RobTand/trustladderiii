# ruff: noqa: F811

"""PB #2710's full-file base result must fit after combining proof groups."""
from copy import deepcopy
import json

import pytest
from tl3_harness.selector_identity import selection_matches
from tl3_harness.test_tool import SUMMARY_BYTES, bounded
from tl3_workers.pb_tests import TestSettings, make_test_tool
from tl3_workers.tools import Worktree

from worker_process import IMAGE, git
from worker_process import rehearsal as rehearsal  # noqa: F401


MODULE = 'tests/test_checkout_recovery_1465.py'
MESSAGE = '_git must run every Git call with maintenance.auto=false and gc.auto=0'


def test_full_file_base_keeps_all_partition_verdicts_and_receipts(rehearsal):
    _, root, inputs, calls = rehearsal
    ids = [f'{i}-' + 'x' * 160 for i in range(32)]
    source = ('import pytest\n\ndef _git():\n    return False\n\n'
              f'@pytest.mark.parametrize("case", range(32), ids={ids!r})\n'
              'def test_missing_or_conflicting_action_snapshot_and_ending_proof_keeps_original(case):\n'
              '    assert case >= 0\n\n'
              f'@pytest.mark.parametrize("case", range(32), ids={ids!r})\n'
              'def test_archive_is_a_complete_stable_nofollow_copy_not_merely_a_digest(case):\n'
              '    assert case >= 0\n')
    path = root / MODULE
    path.write_text(source)
    (root / 'pyproject.toml').write_text(
        '[tool.tl3.harness]\ncommand=["python3", "-m", "pytest"]\ntest-files=["tests/*.py"]\n')
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'Parametrized checkout recovery base')
    base = git(root, 'rev-parse', 'HEAD')
    path.write_text(source.replace('return False', 'return True')
                    + f'\ndef test_git_helper_disables_automatic_maintenance_and_gc():\n    assert _git(), {MESSAGE!r}\n')
    checks = [
        {'name': 'Git helper', 'kind': 'demonstrating',
         'tests': [MODULE + '::test_git_helper_disables_automatic_maintenance_and_gc'],
         'proof': {'subjects': [MODULE + '::_git'], 'observer_support': [], 'red_message': MESSAGE}},
        {'name': 'Ending proof', 'kind': 'regression',
         'tests': [MODULE + '::test_missing_or_conflicting_action_snapshot_and_ending_proof_keeps_original']},
        {'name': 'Archive control', 'kind': 'regression',
         'tests': [MODULE + '::test_archive_is_a_complete_stable_nofollow_copy_not_merely_a_digest']},
    ]
    with Worktree(root) as tree:
        tree.test_checkout, tree.check_base_sha, tree.test_checks = root, base, checks
        result = make_test_tool(tree, TestSettings(IMAGE, inputs['tests']['tools_path']))([MODULE], base=True)
    assert len(calls) == 4, 'three planned checks plus the untouched full-file base'
    assert result['status'] == 'completed', result
    assert len(json.dumps(result).encode()) <= SUMMARY_BYTES
    assert result['totals'] == {'passed': 128, 'failed': 1, 'error': 0, 'skipped': 0}
    assert len(result['receipts']) == 4
    assert len(result['groups']) == 4
    assert [group['check'] for group in result['groups']] == [
        'Git helper', 'Ending proof', 'Archive control', 'unplanned base tests']
    for group, selected in zip(result['groups'], [check['tests'] for check in checks] + [[MODULE]]):
        assert group['status'] == 'completed'
        assert group['base_sha'] == base
        assert group['demonstrating_proof'] == 'pending_checking'
        assert group['receipts'][0] in result['receipts']
        assert list(group['proof']['observer_sha256']) == selected
    assert result['groups'][0]['proof']['declaration'] == checks[0]['proof']
    assert result['groups'][0]['totals']['failed'] == 1
    assert any(test['outcome'] == 'failed' and MESSAGE in test['message'] for test in result['tests'])
    assert any(test['outcome'] == 'failed' and MESSAGE in test['message']
               for test in result['groups'][0]['tests'])
    assert 'Truncated' in result['note']


@pytest.mark.parametrize('status', ['completed', 'infrastructure_error'])
def test_nested_summary_bounds_presentation_without_changing_identity(status):
    selected = [MODULE + f'::test_case[{i}-' + 'x' * 1000 + ']' for i in range(24)]
    group = {'check': 'one partition', 'status': status, 'selectors': selected,
             'tests': [{'test': test, 'outcome': 'passed', 'message': ''} for test in selected],
             'totals': {'passed': 24, 'failed': 0, 'error': 0, 'skipped': 0}, 'note': '',
             'proof': {'immutable': 'base and head source identity'},
             'execution': {'outcome': 'unknown_incomplete', 'phase': 'test', 'test_started': True}}
    result = {'status': status, 'groups': [deepcopy(group)], 'tests': [],
              'totals': dict(group['totals']), 'note': ''}
    observed = bounded(result)
    assert observed['status'] == status, observed
    assert 'groups' in observed, observed
    [actual] = observed['groups']
    assert selection_matches(actual['selectors'], selected)
    assert not selection_matches(actual['selectors'], selected[:-1])
    assert not selection_matches(actual['selectors'], list(reversed(selected)))
    assert actual['proof'] == group['proof']
    assert actual['execution'] == group['execution']
    assert actual['totals'] == group['totals']
    assert len(json.dumps(observed).encode()) <= SUMMARY_BYTES - 2048
