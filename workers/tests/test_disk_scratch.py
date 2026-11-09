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
    # Flux owns the job scratch leaf (launch context); the contract declares only the
    # disk root and its bound, with no PrismaBuild lifetime declaration.
    assert set(variables) == {'TL3_SCRATCH_ROOT', 'TL3_SCRATCH_MAX_BYTES'}
    assert Path(variables['TL3_SCRATCH_ROOT']).is_absolute()
    assert int(variables['TL3_SCRATCH_MAX_BYTES']) == 512 * 1024 * 1024
