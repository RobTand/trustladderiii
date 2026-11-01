"""Prove per-test cleanup within the harness's bounded 512 MiB tmpfs."""
import json
from pathlib import Path
import shutil
import sys
import tempfile

from tl3_harness import Check, Container, Kind, Verdict, run_checks
from tl3_harness.execution import require_admission, result_json
from tl3_harness.trees import git
from verify_reproducing import save


def verify(container):
    with tempfile.TemporaryDirectory(prefix='tl3-scratch-fixture-') as directory:
        repo = Path(directory)
        repo.chmod(0o755)
        shutil.copyfile(Path(__file__).resolve().parents[1] / 'tests/fixtures/scratch/test_scratch.py', repo / 'test_scratch.py')
        (repo / 'pyproject.toml').write_text("[tool.tl3.harness]\ncommand=['python','-m','pytest']\ntest-files=['test_*.py']\n")
        git(repo, 'init', '-q')
        sha = save(repo, 'scratch fixture')
        tests = tuple(f'test_scratch.py::test_scratch[{i}]' for i in range(4))
        results = run_checks(repo, sha, sha, (Check('scratch cleanup', Kind.BEHAVIOR_PRESERVING, tests),), container=container)
        print(result_json(results), flush=True)
        assert results[0].verdict == Verdict.PASS, '1536 MiB cumulative writes must fit by releasing each 384 MiB test'
        print(json.dumps({'scratch_verification': 'passed', 'tests': 4, 'mib_per_test': 384, 'tmpfs_mib': 512}), flush=True)


if __name__ == '__main__':
    require_admission()
    verify(Container(sys.argv[1]))
