"""Run the unmodified disk-scratch specimen through both admitted pytest paths."""
import json
from pathlib import Path
import shutil
import sys
import tempfile

from tl3_harness import Check, Container, Kind, run_checks
from tl3_harness.execution import require_admission, result_json
from tl3_harness.model import Outcome
from tl3_harness.test_tool import run_current
from tl3_harness.trees import git


def verify(image):
    require_admission()
    with tempfile.TemporaryDirectory(prefix='tl3-disk-specimen-') as directory:
        repo = Path(directory)
        repo.chmod(0o755)
        shutil.copyfile(Path(__file__).resolve().parents[1] / 'tests/fixtures/scratch/test_disk.py',
                        repo / 'test_disk.py')
        (repo / 'pyproject.toml').write_text('[tool.tl3.harness]\ncommand=["python","-m","pytest"]\ntest-files=["test_*.py"]\n')
        git(repo, 'init', '-q')
        git(repo, 'add', '.')
        git(repo, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'untouched base')
        sha = git(repo, 'rev-parse', 'HEAD').decode().strip()
        tests = ('test_disk.py::test_disk_backed_scratch', 'test_disk.py::test_restrictive_session_directory')
        plan = (Check('disk-backed base', Kind.BEHAVIOR_PRESERVING, tests),)
        result, = run_checks(repo, sha, sha, plan, container=Container(image))
        print(result_json((result,)), flush=True)
        summary = run_current(repo, Container(image), tests)
        print(json.dumps(summary), flush=True)
        assert all(test.base == test.head == Outcome.PASSED for test in result.tests), 'untouched base must pass on disk'
        assert summary['totals'] == {'passed': 2, 'failed': 0, 'error': 0, 'skipped': 0}


if __name__ == '__main__':
    verify(sys.argv[1])
