"""Prove per-test cleanup within the harness's bounded tmpfs."""
import json
from pathlib import Path
import shutil
import sys
import tempfile

from tl3_harness import Check, Container, Kind, Verdict, run_checks
from tl3_harness.model import Outcome
from tl3_harness.test_tool import run_current
from tl3_harness.execution import TMPFS_MIB, require_admission, result_json
from tl3_harness.trees import git
from verify_reproducing import save


def verify(container):
    with tempfile.TemporaryDirectory(prefix='tl3-scratch-fixture-') as directory:
        repo = Path(directory)
        repo.chmod(0o755)
        shutil.copyfile(Path(__file__).resolve().parents[1] / 'tests/fixtures/scratch/test_scratch.py', repo / 'test_scratch.py')
        (repo / 'pyproject.toml').write_text("[tool.tl3.harness]\ncommand=['python','-m','pytest']\ntest-files=['test_*.py']\n")
        shutil.copyfile(Path(__file__).resolve().parents[1] / 'tests/fixtures/scratch/test_cleanup.py', repo / 'test_cleanup.py')
        git(repo, 'init', '-q')
        sha = save(repo, 'scratch fixture')
        tests = tuple(f'test_scratch.py::test_scratch[{i}]' for i in range(4))
        cleanup = tuple(f'test_cleanup.py::test_writer_recreates_a_file_during_cleanup[{value}]'
                        for value in (False, True))
        shape = ('test_cleanup.py::test_tmp_path_keeps_pytest_name_and_number',)
        plan = (Check('scratch cleanup', Kind.BEHAVIOR_PRESERVING, tests),
                Check('cleanup preserves outcomes', Kind.BEHAVIOR_PRESERVING, cleanup),
                Check('pytest path shape', Kind.BEHAVIOR_PRESERVING, shape))
        results = run_checks(repo, sha, sha, plan, container=container)
        print(result_json(results), flush=True)
        assert results[0].verdict == Verdict.PASS, '1536 MiB cumulative writes must fit by releasing each 384 MiB test'
        assert [test.head for test in results[1].tests] == [Outcome.PASSED, Outcome.ASSERTION]
        assert results[2].verdict == Verdict.PASS
        summary = run_current(repo, container, cleanup + shape)
        print(json.dumps(summary), flush=True)
        assert summary['status'] == 'completed'
        assert summary['totals'] == {'passed': 2, 'failed': 1, 'error': 0, 'skipped': 0}
        print(json.dumps({'scratch_verification': 'passed', 'tests': 4, 'mib_per_test': 384, 'tmpfs_mib': TMPFS_MIB}), flush=True)


if __name__ == '__main__':
    require_admission()
    verify(Container(sys.argv[1]))
