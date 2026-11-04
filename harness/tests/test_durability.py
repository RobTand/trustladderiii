"""#768: content tests skip barriers; persistence tests retain real syncs."""
import json
import os
from pathlib import Path
import sys

import pytest

from tl3_harness import execution
from tl3_harness.model import Outcome


@pytest.mark.parametrize('name', ['fsync', 'fdatasync'])
def test_content_checks_skip_disk_barriers(name):
    assert getattr(os, name)(-1) is None


@pytest.mark.durability
@pytest.mark.parametrize('name', ['fsync', 'fdatasync'])
def test_durability_checks_keep_real_disk_barriers(name):
    with pytest.raises(OSError):
        getattr(os, name)(2**31 - 1)


@pytest.mark.durability
def test_report_journal_is_synced_before_progress(tmp_path, monkeypatch):
    """#768: execution.run_tree must sync each journal record before progress."""
    with pytest.raises(OSError):
        os.fsync(2**31 - 1)
    (tmp_path / 'test_probe.py').write_text('def test_ok(): pass\n')
    runner = Path(execution.__file__).with_name('runner.py')
    monkeypatch.setattr(execution, 'container_command', lambda *args: [
        sys.executable, '-I', '-B', str(runner), '-', str(tmp_path)])
    report = tmp_path / 'report.jsonl'
    synced, progressed = [], []
    original = os.fsync

    def sync(fd):
        original(fd)
        synced.append(report.read_bytes())

    def progress(self, test):
        assert synced[-1] == report.read_bytes()
        assert json.loads(synced[-1].splitlines()[-1])['when'] == 'teardown'
        progressed.append(test)

    monkeypatch.setattr(os, 'fsync', sync)
    monkeypatch.setattr(execution.Progress, 'commit', progress)
    node = 'test_probe.py::test_ok'
    outcomes = execution.run_tree(tmp_path, execution.Container('sha256:' + '1' * 64),
                                  (node,), report, execution.Progress())
    assert outcomes == {node: Outcome.PASSED}
    records = report.read_bytes().splitlines(keepends=True)
    assert synced == [b''.join(records[:end]) for end in range(1, len(records) + 1)]
    assert progressed == [node]
