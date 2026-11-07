"""Staged jobs must run without discovering the controller's Rust executable."""
import json
from pathlib import Path
import shutil
import subprocess
import sys

from tl3_workers.pb_tests import _stage
from tl3_workers.tools import Worktree


def staged(tmp_path):
    checkout = tmp_path / 'worktree'
    checkout.mkdir()
    (checkout / 'test_job.py').write_text(
        'def test_pass():\n    assert True\n\n'
        'def test_fail():\n    assert False, "ordinary assertion"\n\n'
        'def test_error():\n    raise RuntimeError("ordinary exception")\n')
    bundle = tmp_path / 'bundle'
    bundle.mkdir()
    with Worktree(checkout) as worktree:
        _stage(worktree, bundle)
    return bundle


def probe(bundle, tmp_path):
    tools = tmp_path / 'job-tools'
    tools.mkdir()
    (tools / 'git').symlink_to(shutil.which('git'))
    source = Path(__file__).resolve().parents[2] / 'harness/tools/staged_job_probe.py'
    return subprocess.run([sys.executable, '-I', '-B', str(source), str(bundle), 'pb'],
        env={'PATH': str(tools), 'HOME': str(tmp_path), 'LANG': 'C.UTF-8',
             'OMP_NUM_THREADS': '1', 'OPENBLAS_NUM_THREADS': '1'},
        cwd=bundle, capture_output=True, text=True, timeout=90)


def test_actual_pb_bundle_imports_and_runs_job_entrypoints_without_rust(tmp_path):
    result = probe(staged(tmp_path), tmp_path)
    assert result.returncode == 0, result.stderr
    report = json.loads(result.stdout)
    assert len(report['modules']) >= 20
    assert 'Redactor.window' in report['entrypoints']


def test_import_time_action_call_is_rejected_by_the_isolated_probe(tmp_path):
    bundle = staged(tmp_path)
    package = bundle / 'trusted/tl3_harness/__init__.py'
    package.write_text('import subprocess\nsubprocess.run(["tl3-action", "policy"], check=True)\n' + package.read_text())
    result = probe(bundle, tmp_path)
    assert result.returncode != 0
    assert 'tl3-action' in result.stderr and 'FileNotFoundError' in result.stderr
