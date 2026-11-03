"""Q27 #592: exact, predeclared exceptions and bounded intermittent evidence."""
import json
from pathlib import Path
import sys

import pytest

from tl3_harness import execution, test_tool
from tl3_harness.model import Check, Forcing, HarnessInputError, Kind, Verdict
from tl3_harness.trees import git


NODE = 'tests/test_original.py::test_original'
CONTAINER = execution.Container('sha256:' + '1' * 64)
RUNNER = Path(execution.__file__).with_name('runner.py')
FORCING = Forcing.parse({'symptom': 'The original fixture or call fails.',
                         'environment': {'REPRODUCE': '1'}})


def declaration(exception='builtins.TimeoutError', phase='call', repeat=1, parallel=0):
    return dict(node=NODE, exception=exception, phase=phase, repeat=repeat, parallel=parallel)


def check(symptom=None, forcing=FORCING):
    return Check('original symptom', Kind.DEMONSTRATING, (NODE,), forcing=forcing,
                 declared_symptom=declaration() if symptom is None else symptom)


def commit(repo):
    git(repo, 'add', '.')
    git(repo, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'fixture')
    return git(repo, 'rev-parse', 'HEAD').decode().strip()


@pytest.fixture
def repo(tmp_path, monkeypatch):
    root = tmp_path / 'repo'
    (root / 'tests').mkdir(parents=True)
    git(root, 'init', '-q')
    (root / 'pyproject.toml').write_text(
        '[tool.tl3.harness]\ncommand=["python", "-m", "pytest"]\ntest-files=["tests/*.py"]\n')
    monkeypatch.setattr(execution, 'require_admission', lambda: None)
    monkeypatch.setattr(test_tool, 'require_admission', lambda: None)
    command = lambda tree, container, tests: [sys.executable, '-I', '-B', str(RUNNER), '-', str(tree)]
    monkeypatch.setattr(execution, 'container_command', command)
    monkeypatch.setattr(test_tool, 'container_command', command)
    monkeypatch.setenv('PRISMABUILD_CONTAINER_OWNER', str(root))
    monkeypatch.setenv('PRISMABUILD_ACTION_PROGRESS_PATH', str(tmp_path / 'progress.json'))
    monkeypatch.setenv('PRISMABUILD_ACTION_PROGRESS_TOKEN', 'fixture')
    return root


def run(repo, old, new, planned):
    source = repo / 'tests/test_original.py'
    source.write_text(old)
    base = commit(repo)
    source.write_text(new)
    head = commit(repo)
    result, = execution.run_checks(repo, base, head, (planned,), container=CONTAINER)
    return result, base, head


TIMEOUT = '''from concurrent.futures import Future
import os

def test_original():
    future = Future()
    if not os.getenv('REPRODUCE'):
        future.set_result(None)
    future.result(timeout=0)
'''
PASS = 'def test_original():\n    assert True\n'
SETUP = '''import os
import pytest
@pytest.fixture
def scratch(tmp_path):
    if os.getenv('REPRODUCE'):
        raise PermissionError('checkout is unwritable')
    yield tmp_path

def test_original(scratch):
    assert scratch.is_dir()
'''
INTERMITTENT = '''import os
import pytest
@pytest.fixture
def archive(tmp_path):
    marker = os.environ.get('COUNTER')
    if marker:
        from pathlib import Path
        path = Path(marker)
        count = int(path.read_text()) if path.exists() else 0
        path.write_text(str(count + 1))
        if count == 1:
            raise FileNotFoundError('.git/objects/maintenance.lock')
    yield tmp_path

def test_original(archive):
    assert archive.is_dir()
'''


def test_pb2643_future_timeout_is_red_and_receipts_bind_both_trees(repo):
    result, base, head = run(repo, TIMEOUT, PASS, check())
    assert result.verdict == Verdict.PASS, result.tests
    evidence = json.loads(execution.result_json((result,)))[0]['symptom_evidence']
    assert (evidence['base'], evidence['head'], evidence['forcing']) == (base, head, FORCING.as_json())
    assert evidence['declaration'] == declaration()
    assert {receipt['side'] for receipt in evidence['runs']} == {'base', 'head', 'ordinary-head'}
    assert all(len(receipt['sha256']) == 64 and receipt['records'] for receipt in evidence['runs'])


def test_pq3284_declared_setup_uses_the_unchanged_base_fixture(repo):
    planned = check(declaration('builtins.PermissionError', 'setup'))
    fixed = SETUP.replace("if os.getenv('REPRODUCE'):", 'if False:')
    result, _, _ = run(repo, SETUP, fixed, planned)
    assert result.verdict == Verdict.PASS, result.tests


def test_pb2710_fixed_campaign_keeps_every_run_without_stopping_at_red(repo, tmp_path):
    marker = tmp_path / 'counter'
    forcing = Forcing.parse({'symptom': 'The archive loses a maintenance lock.',
                             'environment': {'COUNTER': str(marker)}})
    result, _, _ = run(repo, INTERMITTENT, PASS,
                       check(declaration('builtins.FileNotFoundError', 'setup', repeat=3), forcing))
    assert result.verdict == Verdict.PASS, result.tests
    evidence = json.loads(execution.result_json((result,)))[0]['symptom_evidence']
    assert len(evidence['runs']) == 9
    assert [r['outcome'] for r in evidence['runs'] if r['side'] == 'base'] == ['passed', 'symptom', 'passed']


def test_zero_occurrences_at_base_is_reported_without_retrying(repo):
    result, _, _ = run(repo, PASS, PASS + '\n# head\n', check(declaration(repeat=3)))
    assert result.verdict == Verdict.FAIL
    assert '0 of 3' in result.tests[0].message
    evidence = json.loads(execution.result_json((result,)))[0]['symptom_evidence']
    assert len([r for r in evidence['runs'] if r['side'] == 'base']) == 3


@pytest.mark.parametrize('symptom', [
    declaration('builtins.Exception'), declaration(phase='teardown'), declaration(repeat=11),
    declaration(parallel=1), {**declaration(), 'node': 'tests/test_original.py::other'},
])
def test_invalid_declarations_are_specific_harness_errors(symptom):
    with pytest.raises(HarnessInputError, match='declared symptom'):
        check(symptom)


@pytest.mark.parametrize('symptom', [declaration('builtins.FileNotFoundError'), declaration(phase='setup')])
def test_wrong_exception_or_phase_never_counts(repo, symptom):
    result, _, _ = run(repo, TIMEOUT, PASS, check(symptom))
    assert result.verdict == Verdict.FAIL


@pytest.mark.parametrize('old', [
    'raise TimeoutError("import")\n' + PASS,
    'import pytest\n@pytest.fixture\ndef bad():\n    raise TimeoutError()\n'
    'def test_original(bad):\n    pass\n',
    'import os\ndef test_original():\n    os._exit(1)\n',
])
def test_undeclared_setup_import_and_outer_exit_never_count(repo, old):
    result, _, _ = run(repo, old, PASS, check())
    assert result.verdict != Verdict.PASS


def test_head_matching_symptom_fails(repo):
    result, _, _ = run(repo, TIMEOUT, TIMEOUT + '\n# still broken\n', check())
    assert result.verdict == Verdict.FAIL


def test_assertion_red_remains_the_default(repo):
    result, _, _ = run(repo, TIMEOUT, PASS, Check('default', Kind.DEMONSTRATING, (NODE,), forcing=FORCING))
    assert result.verdict == Verdict.FAIL


def test_declared_demonstrating_without_forcing_uses_original_base(repo):
    result, _, _ = run(repo, TIMEOUT.replace("if not os.getenv('REPRODUCE'):", 'if False:'), PASS,
                       check(forcing=None))
    assert result.verdict == Verdict.PASS, result.tests
