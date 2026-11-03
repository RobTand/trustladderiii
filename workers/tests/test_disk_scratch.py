import json
from pathlib import Path

from test_pb_tests import submission, call, Summary  # noqa: F401


def test_pytest_submission_seals_registered_disk_scratch(submission):
    result = call(submission, Summary().result)
    assert result['status'] == 'completed'
    args = json.loads((submission[1] / 'submitted.json').read_text())
    variables = dict(args[i + 1].split('=', 1) for i, arg in enumerate(args) if arg == '--env')
    assert 'PRISMABUILD_LOCAL_SCRATCH_PAIRS' in variables, 'account for disk scratch in the admitted action'
    pairs = variables['PRISMABUILD_LOCAL_SCRATCH_PAIRS'].split(',')
    selection = json.loads(variables['PRISMABUILD_EPHEMERAL_SCRATCH_DECLARATIONS'])
    assert selection['schema'] == 'prismabuild.scratch_lifetime_selection.v1'
    entry, = selection['entries']
    assert entry['lifetime'] == 'ephemeral'
    root, bound = pairs[0].split(':')
    assert entry['root_env'] == root
    assert Path(variables[root]).is_absolute()
    assert int(variables[bound]) == 512 * 1024 * 1024
