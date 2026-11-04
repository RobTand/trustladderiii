# ruff: noqa: F811

from copy import deepcopy
import pytest
from pb_fixtures import supported_fake_runtime as supported_fake_runtime
from tl3_workers.forcing_rehearsal import ForcingRehearsals

from worker_process import NODE
from worker_process import TEST
from worker_process import git
from worker_process import plan
from worker_process import reachable
from worker_process import rehearsal  # noqa: F401


@pytest.mark.parametrize('validated', [False, True])
@pytest.mark.parametrize('cosmetic', ['symptom', 'selectors', 'patches', 'all'])
@pytest.mark.durability
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
