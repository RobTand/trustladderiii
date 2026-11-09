import json
from pathlib import Path

from pb_fixtures import submission, call  # noqa: F401
from tl3_harness.test_tool import Summary


def test_pytest_submission_seals_registered_disk_scratch(submission):  # noqa: F811
    from pb_fixtures import FAKES
    from tl3_harness.scratch import environment
    result = call(submission, Summary().result)
    assert result['status'] == 'completed'
    [prepared] = FAKES[str(submission[1])].prepared
    contract = json.loads((prepared['root'] / 'worker-input.json').read_text())
    variables = contract['scratch']
    assert variables == environment(), 'account for disk scratch in the admitted run contract'
    pairs = variables['PRISMABUILD_LOCAL_SCRATCH_PAIRS'].split(',')
    selection = json.loads(variables['PRISMABUILD_EPHEMERAL_SCRATCH_DECLARATIONS'])
    assert selection['schema'] == 'prismabuild.scratch_lifetime_selection.v1'
    entry, = selection['entries']
    assert entry['lifetime'] == 'ephemeral'
    root, bound = pairs[0].split(':')
    assert entry['root_env'] == root
    assert Path(variables[root]).is_absolute()
    assert int(variables[bound]) == 512 * 1024 * 1024
