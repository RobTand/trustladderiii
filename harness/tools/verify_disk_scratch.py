"""Run the unmodified disk-scratch specimen through both admitted pytest paths."""
import json
import os
from pathlib import Path
import shutil
import sys
import tempfile
import threading
import time

from tl3_harness import Check, Container, Kind, run_checks
from tl3_harness.execution import require_admission, result_json
from tl3_harness.model import Outcome
from tl3_harness.test_tool import run_current
from tl3_harness.trees import git
from tl3_harness.scratch import registered_leaf


def specimen():
    if os.environ.get('RUNFILES_DIR') or os.environ.get('RUNFILES_MANIFEST_FILE'):
        from python.runfiles import runfiles
        return Path(runfiles.Create().Rlocation('_main/harness/tests/fixtures/scratch/test_disk.py'))
    return Path(__file__).resolve().parents[1] / 'tests/fixtures/scratch/test_disk.py'


def verify(image, mode='success'):
    require_admission()
    with tempfile.TemporaryDirectory(prefix='tl3-disk-specimen-') as directory:
        repo = Path(directory)
        repo.chmod(0o755)
        shutil.copyfile(specimen(), repo / 'test_disk.py')
        (repo / 'pyproject.toml').write_text('[tool.tl3.harness]\ncommand=["python","-m","pytest"]\ntest-files=["test_*.py"]\n')
        git(repo, 'init', '-q')
        git(repo, 'add', '.')
        git(repo, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'untouched base')
        sha = git(repo, 'rev-parse', 'HEAD').decode().strip()
        tests = ('test_disk.py::test_disk_backed_scratch', 'test_disk.py::test_restrictive_session_directory')
        leaf = registered_leaf()
        print(json.dumps({'scratch_leaf': str(leaf), 'mode': mode}), flush=True)
        if mode == 'setup-failure':
            summary = run_current(repo, Container(image), ('test_disk.py::test_setup_failure',))
            print(json.dumps(summary), flush=True)
            assert summary['totals']['error'] == 1
            retained = list(leaf.glob('pytest-*/**/setup-retained*/payload'))
            assert retained and all(path.parent.stat().st_mode & 0o777 == 0o700 for path in retained)
            print('setup failure left mode-0700 payload for PB cleanup', flush=True)
            raise SystemExit(17)
        if mode == 'cancel':
            def report_ready():
                while not list(leaf.glob('pytest-*/cancel-ready')):
                    time.sleep(0.1)
                print('cancellation ready: mode-0700 payload retained in scratch', flush=True)
            threading.Thread(target=report_ready, daemon=True).start()
            run_current(repo, Container(image), ('test_disk.py::test_cancellation',))
            raise AssertionError('cancellation specimen must be stopped by PB')
        assert mode == 'success'
        plan = (Check('disk-backed base', Kind.BEHAVIOR_PRESERVING, tests),)
        result, = run_checks(repo, sha, sha, plan, container=Container(image))
        print(result_json((result,)), flush=True)
        summary = run_current(repo, Container(image), tests)
        print(json.dumps(summary), flush=True)
        assert all(test.base == test.head == Outcome.PASSED for test in result.tests), 'untouched base must pass on disk'
        assert summary['totals'] == {'passed': 2, 'failed': 0, 'error': 0, 'skipped': 0}


if __name__ == '__main__':
    verify(sys.argv[1], sys.argv[2] if len(sys.argv) > 2 else 'success')
