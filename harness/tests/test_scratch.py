from pathlib import Path
import sys

import pytest

from tl3_harness import execution, test_tool
from tl3_harness.model import Outcome

SOURCE = Path(__file__).parent / 'fixtures/scratch/test_scratch.py'
RUNNER = Path(execution.__file__).with_name('runner.py')
IMAGE = execution.Container('sha256:' + '1' * 64)


@pytest.mark.parametrize('summary', [False, True])
def test_temporary_files_are_removed_after_passes_and_failures(tmp_path, monkeypatch, summary):
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    source = SOURCE.read_text().replace('80 * 1024', '4 * 1024').replace('40 * 1024', '1 * 1024')
    source = source.replace('assert value >= 0', 'assert value != 1')
    (tmp_path / 'test_scratch.py').write_text(source)
    monkeypatch.setenv('TMPDIR', str(scratch))
    monkeypatch.setenv('PYTEST_DISABLE_PLUGIN_AUTOLOAD', '1')
    command = lambda tree, container, tests: [sys.executable, '-I', '-B', str(RUNNER), '-', str(tree)]
    tests = tuple(f'test_scratch.py::test_scratch[{i}]' for i in range(4))
    if summary:
        monkeypatch.setattr(test_tool, 'container_command', command)
        monkeypatch.setattr(test_tool, 'require_admission', lambda: None)
        monkeypatch.setenv('PRISMABUILD_ACTION_PROGRESS_PATH', str(tmp_path / 'progress'))
        monkeypatch.setenv('PRISMABUILD_ACTION_PROGRESS_TOKEN', 'fixture')
        result = test_tool.run_current(tmp_path, IMAGE, tests)
        assert result['status'] == 'completed'
        assert result['totals'] == {'passed': 3, 'failed': 1, 'error': 0, 'skipped': 0}
    else:
        monkeypatch.setattr(execution, 'container_command', command)
        class Progress:
            def commit(self, test):
                pass
        result = execution.run_tree(tmp_path, IMAGE, tests, tmp_path / 'report.jsonl', Progress())
        assert result == {test: Outcome.ASSERTION if i == 1 else Outcome.PASSED for i, test in enumerate(tests)}
    assert not list(scratch.rglob('*payload*')), 'earlier tests must release their scratch before the next test'
