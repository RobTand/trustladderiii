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


def test_leaf_without_rehearsal_is_refused_before_maker(rehearsal):
    _, _, inputs, _ = rehearsal
    reply = plan()
    planner.check_reply(inputs, reply)
    assert any('matching forcing rehearsal receipt' in v for v in reply['violations'])


def test_unreachable_slow_decision_reports_zero_hits_and_green_base(rehearsal):
    ledger, _, inputs, calls = rehearsal
    result = ledger.run([NODE], deepcopy(FORCING))
    assert result['status'] == 'refused'
    assert 'zero-hit' in result['message'] and 'publisher.Publisher.decide' in result['message']
    assert result['forced']['tests'][0]['target_hits'] == {'publisher.Publisher.decide': 0}
    assert result['forced']['tests'][0]['outcome'] == 'passed'
    assert ledger.violations(inputs['base_sha'], plan()['checks'])
    assert 1 <= len(calls) <= 2


def test_reached_assertion_has_bound_verified_receipts(rehearsal):
    ledger, root, inputs, calls = rehearsal
    forcing = reachable()
    # An uncommitted change must never affect the pinned rehearsal.
    (root / 'publisher.py').write_text('raise RuntimeError("dirty checkout")\n')
    result = ledger.run([NODE], forcing)
    assert result['status'] == 'validated'
    assert result['base_sha'] == inputs['base_sha'] and result['forcing'] == forcing
    assert result['selectors'] == [NODE]
    assert result['forced']['tests'][0]['target_hits'] == {'publisher.Publisher.try_adopt': 1}
    assert result['forced']['tests'][0]['outcome'] == 'failed'
    assert len(result['forced']['receipts'][0]['receipt_sha256']) == 64
    assert ledger.violations(inputs['base_sha'], plan(forcing)['checks']) == []
    assert len(calls) == 2
    assert all('--anywhere' in cmd and '--demand' in cmd and '--cpus' in cmd for cmd in calls)


def test_killed_fork_target_validates_exact_rehearsal_receipts(rehearsal):
    ledger, root, inputs, calls = rehearsal
    (root / 'tests/test_adoption.py').write_text('''
import os
import signal
import sys
import time
from publisher import Publisher

def test_adopt():
    reader, writer = os.pipe()
    child = os.fork()
    if child == 0:
        os.close(reader)
        def entering_sleep(frame, event, function):
            if event == 'c_call' and function is time.sleep:
                os.write(writer, b'E')
                sys.setprofile(None)
        sys.setprofile(entering_sleep)
        Publisher().try_adopt()
        os.write(writer, b'D')
        os._exit(0)
    os.close(writer)
    try:
        observed = os.read(reader, 1)
        if observed == b'E':
            os.kill(child, signal.SIGKILL)
    finally:
        os.close(reader)
        os.waitpid(child, 0)
    assert observed == b'D', 'child did not finish submission'
''')
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'forking base')
    inputs['base_sha'] = git(root, 'rev-parse', 'HEAD')
    ledger = ForcingRehearsals(ledger.worktree, inputs)
    forcing = reachable()
    result = ledger.run([NODE], forcing)
    assert result['forced']['tests'][0]['outcome'] == 'failed'
    assert result['forced']['tests'][0]['target_hits'] == {'publisher.Publisher.try_adopt': 1}
    assert result['status'] == 'validated', result
    assert result['base_sha'] == inputs['base_sha'] and result['forcing'] == forcing
    assert result['selectors'] == [NODE]
    assert len(result['forced']['receipts'][0]['receipt_sha256']) == 64
    assert ledger.violations(inputs['base_sha'], plan(forcing)['checks']) == []
    assert len(calls) == 2


@pytest.mark.parametrize('control', ['still_green', 'unrelated', 'setup', 'import', 'missing', 'no_report'])
def test_unusable_experiments_cannot_supply_red_or_repeat_identically(rehearsal, monkeypatch, control):
    ledger, root, inputs, calls = rehearsal
    forcing = reachable()
    if control == 'still_green':
        forcing['patches'][0]['delay_seconds'] = 0.001
    elif control == 'unrelated':
        (root / 'tests/test_adoption.py').write_text(TEST + '\n    assert False, "unrelated"\n')
    elif control == 'setup':
        (root / 'tests/conftest.py').write_text('import pytest\n@pytest.fixture(autouse=True)\ndef broken():\n    raise RuntimeError("setup")\n')
    elif control == 'import':
        (root / 'tests/test_adoption.py').write_text('import absent_fixture_module\n' + TEST)
    elif control == 'missing':
        (root / 'tests/test_adoption.py').write_text(TEST.replace('def test_adopt', 'def test_other'))
    elif control == 'no_report':
        (root / 'tests/test_adoption.py').write_text('import os\nos._exit(0)\n')
    if control in {'unrelated', 'setup', 'import', 'missing', 'no_report'}:
        git(root, 'add', '.')
        git(root, 'commit', '-qm', control)
        inputs['base_sha'] = git(root, 'rev-parse', 'HEAD')
        ledger = ForcingRehearsals(ledger.worktree, inputs)
    result = ledger.run([NODE], forcing)
    expected = 'infrastructure_error' if control == 'no_report' else 'refused'
    assert result['status'] == expected
    count = len(calls)
    again = ledger.run([NODE], deepcopy(forcing))
    assert again['status'] == expected and len(calls) == count
    assert ledger.violations(inputs['base_sha'], plan(forcing)['checks'])


@pytest.mark.parametrize('change', ['base', 'selectors', 'target', 'value', 'environment', 'receipt'])
def test_changed_or_stale_evidence_is_unusable(rehearsal, change):
    ledger, _, inputs, _ = rehearsal
    forcing = reachable()
    assert ledger.run([NODE], forcing)['status'] == 'validated'
    checks = plan(forcing)['checks']
    base = inputs['base_sha']
    if change == 'base':
        base = 'f' * 40
    elif change == 'selectors':
        checks[0]['node_ids'].append('tests/test_adoption.py::test_other')
    elif change == 'target':
        checks[0]['forcing']['patches'][0]['target'] = 'publisher.Publisher.decide'
    elif change == 'value':
        checks[0]['forcing']['patches'][0]['delay_seconds'] = 2
    elif change == 'environment':
        checks[0]['forcing']['environment'] = {'ADOPTION': '0'}
    else:
        ledger.receipts.clear()
    assert ledger.violations(base, checks)


def test_planner_tool_allows_only_declarative_forcing_and_bounded_actions(rehearsal):
    ledger, _, _, calls = rehearsal
    refused = ledger.run([NODE], {**FORCING, 'command': 'touch /tmp/arbitrary'})
    assert refused['status'] == 'refused' and 'command' in refused['message']
    assert calls == []
    assert ledger.run([NODE], reachable())['status'] == 'validated'
    count = len(calls)
    assert ledger.run([NODE], reachable())['status'] == 'validated'
    assert len(calls) == count
    for index in range(10):
        forcing = reachable()
        forcing['symptom'] += str(index)
        ledger.run([NODE], forcing)
    assert len(calls) <= 16


def test_independent_checking_is_still_required(rehearsal):
    ledger, _, _, _ = rehearsal
    result = ledger.run([NODE], reachable())
    assert result['status'] == 'validated'
    assert result['pending_checking'] is True
    assert 'head' not in result and 'head_forced' not in result


def test_every_named_old_path_must_hit_the_delayed_target(rehearsal):
    ledger, root, inputs, _ = rehearsal
    (root / 'tests/test_adoption.py').write_text(TEST + '\ndef test_bypass():\n    assert Publisher().decide()\n')
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'second path bypasses adoption')
    inputs['base_sha'] = git(root, 'rev-parse', 'HEAD')
    ledger = ForcingRehearsals(ledger.worktree, inputs)
    result = ledger.run([NODE, 'tests/test_adoption.py::test_bypass'], reachable())
    assert result['status'] == 'refused'
    assert 'zero-hit' in result['message'] and 'test_bypass' in result['message']


@pytest.mark.parametrize('changed', ['forcing', 'selectors'])
def test_pb_verified_log_for_another_forcing_or_selection_is_not_evidence(rehearsal, monkeypatch, changed):
    from tl3_workers import pb_tests
    ledger, _, _, _ = rehearsal
    original = pb_tests._summary

    def stale(ending, key):
        result = original(ending, key)
        if 'forcing' in result:
            result[changed] = {} if changed == 'forcing' else ['tests/test_other.py::test_other']
        return result

    monkeypatch.setattr(pb_tests, '_summary', stale)
    result = ledger.run([NODE], reachable())
    assert result['status'] == 'infrastructure_error'
    assert result['forced']['status'] == 'infrastructure_error'
    assert ledger.failure is None


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
