# ruff: noqa: F811

"""PB #2710's full-file base result must fit after combining proof groups."""
from copy import deepcopy
import hashlib
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


# PB #2710's recorded full-file base run at 01:09Z on 2026-11-04 (PB actions 05af9029,
# 351f7744, a76596a6, e14b5b5c): per entry, its outcome, node ID length and message length.
REAL_GROUPS = [
    ('check-1', 'head-observer', [91], [('failed', 91, 854)], {'passed': 0, 'failed': 1, 'error': 0, 'skipped': 0}),
    ('check-2', 'pytest-outcome', [113], [('passed', n, 0) for n in (
        131, 133, 124, 126, 133, 133, 135, 130, 131, 130, 132, 131, 133, 129, 130, 132, 128, 129, 127, 126,
        125, 127, 133)], {'passed': 23, 'failed': 0, 'error': 0, 'skipped': 0}),
    ('check-3', 'pytest-outcome', [105, 112], [('passed', n, 0) for n in (
        113, 118, 121, 130, 117, 123, 123, 122, 123, 124, 127, 125, 121, 123, 115, 121, 114, 119, 120, 119,
        126, 127, 125, 129, 128, 126, 124, 128, 127)], {'passed': 29, 'failed': 0, 'error': 0, 'skipped': 0}),
    ('unplanned base tests', 'pytest-outcome', [36], [('passed', n, 0) for n in (
        127, 125, 129, 128, 126, 124, 128, 127, 98, 94, 95, 100, 101, 101, 102, 105, 104)]
     + [('error', n, 1000) for n in (100, 133, 124, 126, 133, 133, 135, 130, 131)],
     {'passed': 25, 'failed': 1, 'error': 267, 'skipped': 0}),
]


def _node(length, index):
    # The demonstrating test and the unplanned file selector keep their recorded names.
    for name in (MODULE, MODULE + '::test_git_helper_disables_automatic_maintenance_and_gc'):
        if length == len(name):
            return name
    stem = f'{MODULE}::test_case[{index}-'
    return stem + 'p' * (length - len(stem) - 1) + ']'


def _recorded_group(index, name, mode, observers, entries, totals):
    """One group as wait() returns it: the action's summary plus receipt and base metadata."""
    digest = lambda label: hashlib.sha256(f'{name}:{label}'.encode()).hexdigest()  # noqa: E731
    subjects = {MODULE + '::_git': {'base': digest('base'), 'head': digest('head')}} if mode == 'head-observer' else {}
    proof = {'base_tree_sha256': digest('base-tree'), 'head_tree_sha256': digest('head-tree'),
             'source_base_tree_sha256': digest('source'), 'mode': mode, 'rule': 'subject-observer-v1',
             'declaration': {'observer_support': [], 'red_message': MESSAGE, 'subjects': [MODULE + '::_git']}
             if subjects else None,
             'observer_sha256': {_node(length, 900 + i): digest(f'observer-{i}') for i, length in enumerate(observers)},
             'subjects': subjects, 'support_sha256': {}}
    key = digest('action')
    return {'check': name, 'status': 'completed',
            'tests': [{'test': _node(length, i), 'outcome': outcome, 'message': 'E' * size}
                      for i, (outcome, length, size) in enumerate(entries)],
            'totals': totals, 'note': '' if len(entries) == sum(totals.values()) else
            'Truncated: selectors, test entries or run failure IDs were cut. Totals include all results.',
            'proof': proof, 'action_key': key,
            'receipts': [{'action_key': key, 'status': 'executed', 'exit_code': 0, 'receipt_sha256': digest('receipt'),
                          'log_sha256': digest('log'), 'attempt': 1, 'generation': digest('generation')}],
            'base_sha': '32f908fefc87bc83f25e5919809dbebdea4e2b18',
            'base_semantics': 'base_subject_head_observer' if subjects else 'untouched_base',
            'demonstrating_proof': 'pending_checking', 'tree': digest('tree')[:40]}


def test_recorded_pb_2710_base_envelope_completes_with_every_identity():
    # Before #719 this exact shape ended in identity_exceeds_summary_limit: the old bound
    # cut only top-level entries, and the four groups' own entries kept it above 16 KiB.
    groups = [_recorded_group(i, *spec) for i, spec in enumerate(REAL_GROUPS)]
    expected = deepcopy(groups)
    combined = {'status': 'completed', 'groups': groups,
                'tests': [test for group in groups for test in group['tests']],
                'receipts': [receipt for group in groups for receipt in group['receipts']],
                'totals': {outcome: sum(group['totals'][outcome] for group in groups)
                           for outcome in ('passed', 'failed', 'error', 'skipped')},
                'note': 'Each group runs with its own declared source partition and execution settings.'}
    assert len(json.dumps(combined).encode()) > 3 * SUMMARY_BYTES
    result = bounded(combined, limit=SUMMARY_BYTES)
    assert result['status'] == 'completed', result.get('execution')
    assert len(json.dumps(result).encode()) <= SUMMARY_BYTES
    assert result['totals'] == {'passed': 77, 'failed': 2, 'error': 267, 'skipped': 0}
    assert result['receipts'] == [receipt for group in expected for receipt in group['receipts']]
    for group, recorded in zip(result['groups'], expected):
        for field in ('check', 'status', 'totals', 'proof', 'action_key', 'receipts', 'base_sha',
                      'base_semantics', 'demonstrating_proof', 'tree'):
            assert group[field] == recorded[field], (group['check'], field)
