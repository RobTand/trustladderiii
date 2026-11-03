# ruff: noqa: F811

from copy import deepcopy

from worker_process import FORCING
from worker_process import NODE
from worker_process import plan
from worker_process import reachable
from worker_process import rehearsal  # noqa: F401


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
