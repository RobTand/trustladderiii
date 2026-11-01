"""Prove the opt-in in real contained checks and maker summaries, through PB."""
import json
import os
from pathlib import Path
import sys
import tempfile

from tl3_harness import Check, Container, Kind, run_checks
from tl3_harness.execution import require_admission, result_json
from tl3_harness.model import Outcome
from tl3_harness.test_tool import run_current
from tl3_harness.trees import export, git
from verify_reproducing import save
from verify_scratch import verify as verify_cleanup


def verify(image):
    require_admission()
    with tempfile.TemporaryDirectory(prefix='tl3-executable-fixture-') as directory:
        repo = Path(directory)
        repo.chmod(0o755)
        (repo / 'pyproject.toml').write_text('[tool.tl3.harness]\ncommand=["python","-m","pytest"]\ntest-files=["test_*.py"]\n')
        (repo / 'test_exec.py').write_text('''import ctypes
import os
from pathlib import Path
import shutil
import socket
import subprocess


def test_copied_executable(tmp_path):
    binary = tmp_path / 'true'
    shutil.copy2('/usr/bin/true', binary)
    subprocess.run([str(binary)], check=True)


def test_compiled_library(tmp_path):
    source, library = tmp_path / 'value.c', tmp_path / 'value.so'
    source.write_text('int value(void) { return 42; }')
    subprocess.run(['gcc', '-shared', '-fPIC', str(source), '-o', str(library)], check=True)
    assert ctypes.CDLL(str(library)).value() == 42


def test_containment(tmp_path):
    assert Path(os.environ['TMPDIR']) == tmp_path
    assert os.getuid() == 65534
    assert not Path('/checkout/.git').exists()
    assert not Path('/mnt/shared').exists()
    assert not Path('/var/run/docker.sock').exists()
    for name in ['/checkout/test_exec.py', '/harness/runner.py']:
        try:
            Path(name).write_text('changed')
        except OSError:
            pass
        else:
            raise AssertionError(name + ' must be read-only')
    try:
        socket.create_connection(('1.1.1.1', 443), timeout=1)
    except OSError:
        pass
    else:
        raise AssertionError('network must stay disabled')
'''+f'    assert sorted(os.sched_getaffinity(0)) == {sorted(os.sched_getaffinity(0))!r}\n')
        git(repo, 'init', '-q')
        sha = save(repo, 'scratch execution fixture')
        summary_tree = repo / 'exported'
        export(repo, sha, summary_tree)
        tests = tuple('test_exec.py::' + name for name in ('test_copied_executable', 'test_compiled_library', 'test_containment'))
        plan = (Check('executable scratch boundary', Kind.BEHAVIOR_PRESERVING, tests),)
        for enabled in (False, True):
            container = Container(image, exec_scratch=enabled)
            results = run_checks(repo, sha, sha, plan, container=container)
            print(result_json(results), flush=True)
            expected = [Outcome.PASSED if enabled else Outcome.ERROR] * 2 + [Outcome.PASSED]
            assert [test.base for test in results[0].tests] == expected
            assert [test.head for test in results[0].tests] == expected
            summary = run_current(summary_tree, container, tests)
            print(json.dumps(summary), flush=True)
            assert summary['status'] == 'completed'
            assert summary['totals'] == {'passed': 3 if enabled else 1, 'failed': 0, 'error': 0 if enabled else 2, 'skipped': 0}
        verify_cleanup(Container(image, exec_scratch=True))
    print(json.dumps({'exec_scratch_verification': 'passed', 'image': image, 'tmpfs_mib': 512}), flush=True)


if __name__ == '__main__':
    verify(sys.argv[1])
