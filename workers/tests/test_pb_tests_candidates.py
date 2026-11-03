# ruff: noqa: F811

from copy import deepcopy
import pytest
from tl3_workers.forcing_rehearsal import ForcingRehearsals
from tl3_workers.roles import planner

from worker_process import FORCING
from worker_process import NODE
from worker_process import TEST
from worker_process import git
from worker_process import plan
from worker_process import reachable
from worker_process import rehearsal  # noqa: F401


@pytest.mark.parametrize('negative', ['zero_hit', 'no_red', 'setup_error'])
def test_distinct_candidate_after_negative_preserves_valid_receipts(rehearsal, monkeypatch, negative):
    ledger, _, inputs, calls = rehearsal
    first = reachable()
    assert ledger.run([NODE], first)['status'] == 'validated'
    bad = deepcopy(FORCING) if negative == 'zero_hit' else reachable()
    if negative == 'no_red':
        bad['patches'][0]['delay_seconds'] = 0.001
    elif negative == 'setup_error':
        bad['patches'] = [{'target': 'publisher.Publisher.try_adopt', 'raises': 'RuntimeError'}]
    rejected = ledger.run([NODE], bad)
    assert rejected['status'] == 'refused'
    assert ledger.failure is None, 'a completed negative candidate must not latch planning'
    assert ledger.violations(inputs['base_sha'], plan(bad)['checks'])
    third = reachable()
    third['environment'] = {'CANDIDATE': 'distinct'}
    assert ledger.run([NODE], third)['status'] == 'validated'
    assert ledger.violations(inputs['base_sha'], plan(first)['checks'] + plan(third)['checks']) == []
    count = len(calls)
    assert ledger.run([NODE], deepcopy(bad)) == rejected
    assert ledger.run([NODE], deepcopy(first))['status'] == 'validated'
    assert len(calls) == count == 6


def test_negative_cache_and_exhaustion_preserve_eight_candidate_cap(rehearsal):
    ledger, _, inputs, calls = rehearsal
    for i in range(8):
        bad = deepcopy(FORCING)
        bad['environment'] = {'CANDIDATE': str(i)}
        rejected = ledger.run([NODE], bad)
        assert rejected['status'] == 'refused'
        assert ledger.run([NODE], deepcopy(bad)) == rejected
        assert len(calls) == 2 * (i + 1), 'distinct negatives must consume their own bounded actions'
    exhausted = ledger.run([NODE], reachable())
    assert exhausted['status'] == 'exhausted'
    assert '8 rehearsals' in exhausted['message'] and '16' in exhausted['message']
    assert ledger.run([NODE], reachable()) == exhausted
    assert len(calls) == 16
    assert ledger.violations(inputs['base_sha'], plan(reachable())['checks']) == [exhausted['message']]
    assert all(cmd[cmd.index('--timeout-s') + 1] == '120' for cmd in calls)


def test_final_plan_accepts_only_matching_validated_candidates(rehearsal):
    ledger, _, inputs, _ = rehearsal
    assert ledger.run([NODE], deepcopy(FORCING))['status'] == 'refused'
    reply = plan()
    planner.check_reply(inputs, reply, rehearsals=ledger)
    assert any('matching forcing rehearsal receipt' in v for v in reply['violations'])
    good = reachable()
    assert ledger.run([NODE], good)['status'] == 'validated'
    reply = plan(good)
    planner.check_reply(inputs, reply, rehearsals=ledger)
    assert reply['violations'] == []
    assert {r['status'] for r in reply['forcing_rehearsals']} == {'refused', 'validated'}
    changed = plan(good)
    changed['checks'][0]['forcing']['environment'] = {'DIFFERENT': '1'}
    planner.check_reply(inputs, changed, rehearsals=ledger)
    assert any('matching forcing rehearsal receipt' in v for v in changed['violations'])


@pytest.mark.parametrize('replacement', [True, 1.0])
def test_receipt_binding_and_execution_cache_preserve_json_value_types(rehearsal, replacement):
    ledger, root, inputs, calls = rehearsal
    (root / 'tests/test_adoption.py').write_text(
        'from publisher import Publisher\ndef test_adopt():\n    assert Publisher().try_adopt() is True\n')
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'type-sensitive assertion')
    inputs['base_sha'] = git(root, 'rev-parse', 'HEAD')
    ledger = ForcingRehearsals(ledger.worktree, inputs)
    forcing = {'symptom': 'The return type matters.',
               'patches': [{'target': 'publisher.Publisher.try_adopt', 'returns': 1}]}
    assert ledger.run([NODE], forcing)['status'] == 'validated'
    changed = deepcopy(forcing)
    changed['patches'][0]['returns'] = replacement
    assert ledger.violations(inputs['base_sha'], plan(changed)['checks']), '1 must not bind true or 1.0'
    assert ledger.violations(inputs['base_sha'], plan(forcing)['checks']) == []
    ledger.run([NODE], changed)
    assert len(calls) == 4, 'JSON-distinct values must run as distinct experiments'


@pytest.mark.parametrize('validated', [False, True])
@pytest.mark.parametrize('cosmetic', ['symptom', 'selectors', 'patches', 'all'])
def test_execution_cache_ignores_prose_and_irrelevant_order(rehearsal, tmp_path, validated, cosmetic):
    ledger, root, inputs, calls = rehearsal
    (root / 'tests/test_adoption.py').write_text(TEST + TEST[TEST.index('def test_adopt'):].replace(
        'def test_adopt', 'def test_other'))
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'two selectors')
    inputs['base_sha'] = git(root, 'rev-parse', 'HEAD')
    checkpoint = tmp_path / 'rehearsals.json'
    ledger = ForcingRehearsals(ledger.worktree, inputs, checkpoint=checkpoint)
    forcing = reachable()
    if not validated:
        forcing['patches'][0]['delay_seconds'] = 0.001
    forcing['patches'].append({'target': 'publisher.Publisher.decide', 'returns': 1})
    selectors = [NODE, NODE.replace('::test_adopt', '::test_other')]
    first = ledger.run(selectors, forcing)
    assert first['status'] == ('validated' if validated else 'refused')
    changed = deepcopy(forcing)
    reordered = list(selectors)
    if cosmetic in {'symptom', 'all'}:
        changed['symptom'] = 'The same experiment, described differently.'
    if cosmetic in {'selectors', 'all'}:
        reordered.reverse()
    if cosmetic in {'patches', 'all'}:
        changed['patches'].reverse()
    for current in (ledger, ForcingRehearsals(ledger.worktree, inputs, checkpoint=checkpoint)):
        assert current.run(reordered, changed) == first, 'cosmetic changes must return the original evidence'
        assert current.attempts == 1 and current.actions == 2
        assert len(calls) == 2
        final = plan(changed)['checks']
        final[0]['node_ids'] = reordered
        assert current.violations(inputs['base_sha'], final), 'cache reuse must not mint a changed final receipt'
        exact = plan(forcing)['checks']
        exact[0]['node_ids'] = selectors
        assert bool(current.violations(inputs['base_sha'], exact)) is not validated
