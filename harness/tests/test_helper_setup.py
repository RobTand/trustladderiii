"""Exercise callable helper preconditions with real repository code and pytest."""

import json
from pathlib import Path
import sys

import pytest

from tl3_harness import execution, HelperEvidence
from tl3_harness.execution import Container, Progress, run_checks
from tl3_harness.model import Check, HarnessInputError, Kind, Outcome, Verdict
from tl3_harness.trees import git, helper_test_edits


FIXTURE = Path(__file__).parent / 'fixtures/helper_setup/test_product.py'
RUNNER = Path(__file__).parents[1] / 'src/tl3_harness/runner.py'
TARGET = 'tests/test_product.py::_execute_mover'
OBSERVER = 'tests/test_product.py::test_diagnostics'
IMAGE = 'sha256:' + '1' * 64
DIAGNOSTIC = ("f'{mover} code={outcome.returncode} stdout: {outcome.stdout[:128]}' "
              "+ (' [truncated]' if len(outcome.stdout) > 128 else '') "
              "+ f' stderr: {outcome.stderr[:128]}'")
OBSERVER_SOURCE = '''

def test_diagnostics():
    import contextlib
    import io
    import subprocess
    import sys
    import tempfile
    from pathlib import Path
    with tempfile.TemporaryDirectory() as directory:
        inner = Path(directory) / 'test_inner.py'
        inner.write_text('import sys\\nsys.path.insert(0, ' + repr(str(Path(__file__).parent)) + ')\\n'
                         'from test_product import _failed_queue, _execute_mover\\n'
                         'def test_inner():\\n'
                         '    q = _failed_queue()\\n'
                         '    try:\\n        _execute_mover(q, "mover-key")\\n'
                         '    finally:\\n        assert q.cleaned\\n')
        child = subprocess.run([sys.executable, '-m', 'pytest', '-q', '--tb=short', str(inner)],
                               capture_output=True, text=True)
    assert child.returncode == 1, child.stdout
    for marker in ('mover-key code=7', 'stdout: stdout-marker', 'stderr: stderr-cause', '[truncated]'):
        assert marker in child.stdout, child.stdout
    assert 'x' * 129 not in child.stdout
    q = Queue(0)
    output = io.StringIO()
    with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
        assert _execute_mover(q, 'mover-key') is q.result
    assert q.cleaned
    assert output.getvalue() == ''
'''


def evidence(setup='_failed_queue'):
    return HelperEvidence(TARGET, (json.dumps({'args': [{'$setup': setup, '$call': 'execute'}, 'mover-key'], 'kwargs': {}}),))


@pytest.fixture
def helper_repo(tmp_path, monkeypatch):
    repo = tmp_path / 'repo'
    (repo / 'tests').mkdir(parents=True)
    source = FIXTURE.read_text()
    path = repo / 'tests/test_product.py'
    path.write_text(source)
    (repo / 'pyproject.toml').write_text(
        '[tool.tl3.harness]\ncommand=["python", "-m", "pytest"]\ntest-files=["tests/*.py"]\n')
    git(repo, 'init', '-q')
    def save():
        git(repo, 'add', '.')
        git(repo, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'fixture')
        return git(repo, 'rev-parse', 'HEAD').decode().strip()
    base = save()
    path.write_text(source.replace("'mover failed'", DIAGNOSTIC) + OBSERVER_SOURCE)
    monkeypatch.setattr(execution, 'require_admission', lambda: None)
    monkeypatch.setenv('PRISMABUILD_CONTAINER_OWNER', str(repo))
    monkeypatch.setattr(Progress, 'commit', lambda *_: None)
    monkeypatch.setattr(execution, 'container_command',
        lambda tree, container, tests: [sys.executable, '-I', '-B', str(RUNNER), '-', str(tree)])
    return repo, base, save


def check_repo(helper_repo, declaration):
    repo, base, save = helper_repo
    head = save()
    check = Check('queue diagnostics', Kind.DEMONSTRATING, (OBSERVER,), helper_evidence=declaration)
    result, = run_checks(repo, base, head, (check,), container=Container(IMAGE))
    return result


@pytest.mark.parametrize('keyword', [False, True])
def test_existing_setup_reaches_failed_outcome_and_binds_receipt(helper_repo, keyword):
    declaration = evidence()
    if keyword:
        declaration = HelperEvidence(TARGET, (json.dumps({
            'args': [], 'kwargs': {'q': {'$setup': '_failed_queue', '$call': 'execute'}, 'mover': 'mover-key'}}),))
    result = check_repo(helper_repo, declaration)
    assert result.verdict == Verdict.PASS, result
    assert (result.tests[0].base, result.tests[0].head) == (Outcome.ASSERTION, Outcome.PASSED)
    receipt = result.helper_evidence
    assert receipt['failure_cases'] == declaration.as_json()['failure_cases']
    assert receipt['failure_preservation'] == [{'base': 'passed', 'head': 'passed'}]
    for name in ('target', 'tree'):
        assert receipt[f'base_{name}_sha256'] != receipt[f'head_{name}_sha256']
    assert len(receipt['failure_driver_sha256']) == 64
    assert set(receipt['observers']) == {OBSERVER}


@pytest.mark.parametrize('setup', ['_missing_row', '_noncallable', '_setup_asserts', '_wrong_exception'])
def test_setup_errors_never_prove_failure_preservation(helper_repo, setup):
    result = check_repo(helper_repo, evidence(setup))
    assert result.verdict == Verdict.FAIL
    assert result.helper_evidence['failure_preservation'] == [{'base': 'error', 'head': 'error'}]
    assert 'setup' in result.tests[0].message.lower()


def test_helper_returning_success_cannot_satisfy_preservation(helper_repo):
    repo, _, _ = helper_repo
    path = repo / 'tests/test_product.py'
    path.write_text(path.read_text().replace('assert outcome.returncode == 0, ' + DIAGNOSTIC, 'pass'))
    result = check_repo(helper_repo, evidence())
    assert result.verdict == Verdict.FAIL
    assert result.helper_evidence['failure_preservation'] == [{'base': 'passed', 'head': 'assertion'}]


@pytest.mark.parametrize('setup', ['_absent', '_execute_mover', 'Queue', '_needs_argument', '_decorated', '_rebound'])
def test_setup_preflight_rejects_unsupported_factory_before_execution(tmp_path, setup):
    source = FIXTURE.read_text() + '''

def _needs_argument(value): return value

@staticmethod
def _decorated(): return Queue(7)

def _rebound(): return Queue(7)
_rebound = _failed_queue
'''
    base, head, staged = [tmp_path / name for name in ('base', 'head', 'staged')]
    for tree, content in ((base, source), (head, source.replace("'mover failed'", DIAGNOSTIC) + OBSERVER_SOURCE),
                          (staged, source)):
        (tree / 'tests').mkdir(parents=True)
        (tree / 'tests/test_product.py').write_text(content)
    with pytest.raises(ValueError, match='setup'):
        helper_test_edits(base, head, staged, ['tests/*.py'], evidence(setup), (OBSERVER,))


@pytest.mark.parametrize('value', [
    {'$setup': 'os.system', '$call': 'execute'}, {'$setup': '_failed_queue', 'code': 'arbitrary()'},
    {'$setup': '_failed_queue', 'args': [1]}, {'$setup': 42},
    {'$setup': '_failed_queue', '$attributes': {}},
    {'$setup': '_failed_queue'}, {'$setup': '_failed_queue', '$call': 'execute.run'},
    {'$setup': '_failed_queue', '$call': '__class__'},
])
def test_setup_declarations_accept_only_a_bounded_local_function_name(value):
    with pytest.raises(HarnessInputError, match='setup'):
        HelperEvidence.parse({'target': TARGET, 'failure_cases': [{'args': [value], 'kwargs': {}}]})


def test_data_objects_cannot_supply_callable_queue_preconditions(helper_repo):
    declaration = HelperEvidence(TARGET, (json.dumps({
        'args': [{'$attributes': {'records': {'item': None}, 'execute': None}}, 'mover-key'],
        'kwargs': {}}),))
    result = check_repo(helper_repo, declaration)
    assert result.verdict == Verdict.FAIL
    assert result.helper_evidence['failure_preservation'] == [{'base': 'error', 'head': 'error'}]


def test_setup_function_cannot_change_with_the_target(tmp_path):
    source = FIXTURE.read_text()
    base, head, staged = [tmp_path / name for name in ('base', 'head', 'staged')]
    for tree, content in ((base, source), (staged, source),
                          (head, source.replace("'mover failed'", DIAGNOSTIC).replace('return Queue(7)', 'return Queue(0)')
                           + OBSERVER_SOURCE)):
        (tree / 'tests').mkdir(parents=True)
        (tree / 'tests/test_product.py').write_text(content)
    with pytest.raises(ValueError, match='undeclared'):
        helper_test_edits(base, head, staged, ['tests/*.py'], evidence(), (OBSERVER,))
