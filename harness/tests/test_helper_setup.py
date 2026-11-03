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


FIXTURE_OBSERVER = OBSERVER_SOURCE.replace(
    '_failed_queue, _execute_mover', '_staged_world, _execute_mover').replace(
    'def test_inner():', 'def test_inner(tmp_path):').replace(
    'q = _failed_queue()', 'q = _staged_world(tmp_path)').replace(
    'assert q.cleaned\\n', 'assert q.cleaned\\n        assert not q.records.path.exists()\\n'
    '        assert (q.root / \"cleaned\").exists()\\n')

FIXTURE_OBSERVER = FIXTURE_OBSERVER[:FIXTURE_OBSERVER.index('    q = Queue(0)')] + '''
    with tempfile.TemporaryDirectory() as directory:
        q = _staged_world(Path(directory))
        q.result.returncode = 0
        output = io.StringIO()
        with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
            assert _execute_mover(q, 'mover-key') is q.result
        assert q.cleaned
        assert not q.records.path.exists()
        assert (q.root / 'cleaned').exists()
        assert output.getvalue() == ''
'''


def fixture_repo(helper_repo):
    repo, _, _ = helper_repo
    path = repo / 'tests/test_product.py'
    path.write_text(path.read_text().replace(OBSERVER_SOURCE, FIXTURE_OBSERVER))


def capture_fixture_cleanup(monkeypatch):
    original = execution.PYTEST.run
    runs = []
    def observed(tree, container, tests, report, progress, *args, **kwargs):
        if report.name.startswith('helper-failure-'):
            (tree / 'fixture-roots.jsonl').unlink(missing_ok=True)
        result = original(tree, container, tests, report, progress, *args, **kwargs)
        if report.name.startswith('helper-failure-'):
            paths = [Path(json.loads(line)) for line in (tree / 'fixture-roots.jsonl').read_text().splitlines()]
            assert paths, 'the real fixture factory must have run'
            assert all(not path.exists() for path in paths), 'fixture roots must be cleaned even on failure'
            runs.append(report.name)
        return result
    monkeypatch.setattr(execution.PYTEST, 'run', observed)
    return runs


@pytest.mark.parametrize('setup,fixtures', [
    ('_staged_world', ['tmp_path']), ('_two_real_generations', ['tmp_path_factory']),
    ('_both_fixtures', ['tmp_path', 'tmp_path_factory']),
    ('_positional_fixture', ['tmp_path']),
])
def test_fixture_setup_claims_row_preserves_failure_and_cleans_up(helper_repo, monkeypatch, setup, fixtures):
    fixture_repo(helper_repo)
    runs = capture_fixture_cleanup(monkeypatch)
    result = check_repo(helper_repo, evidence(setup))
    assert result.verdict == Verdict.PASS, result
    assert len(runs) == 2
    assert (result.tests[0].base, result.tests[0].head) == (Outcome.ASSERTION, Outcome.PASSED)
    receipt = result.helper_evidence
    assert receipt['failure_preservation'] == [{'base': 'passed', 'head': 'passed'}]
    identity = receipt['setup_functions'][setup]
    assert identity['fixtures'] == fixtures
    assert len(identity['source_sha256']) == 64


@pytest.mark.parametrize('setup', ['_queue', '_fixture_setup_asserts', '_fixture_method_raises'])
def test_fixture_precondition_assertions_are_errors_and_cleanup_runs(helper_repo, monkeypatch, setup):
    fixture_repo(helper_repo)
    runs = capture_fixture_cleanup(monkeypatch)
    result = check_repo(helper_repo, evidence(setup))
    assert result.verdict == Verdict.FAIL
    assert len(runs) == 2
    assert result.helper_evidence['failure_preservation'] == [{'base': 'error', 'head': 'error'}]


def test_fixture_factory_path_allocation_is_bounded(helper_repo, monkeypatch):
    fixture_repo(helper_repo)
    runs = capture_fixture_cleanup(monkeypatch)
    result = check_repo(helper_repo, evidence('_too_many_paths'))
    assert len(runs) == 2
    assert result.verdict == Verdict.FAIL
    assert result.helper_evidence['failure_preservation'] == [{'base': 'error', 'head': 'error'}]


@pytest.mark.parametrize('definition,diagnostic', [
    ('class _World: pass', 'class-based worlds are unsupported'),
    ('def _factory(monkeypatch): pass', 'monkeypatch'),
    ('def _factory(tmp_path, value): pass', 'value'),
    ('def _factory(*args): pass', 'variadic'),
    ('def _factory(**kwargs): pass', 'variadic'),
    ('async def _factory(tmp_path): pass', 'synchronous'),
    ('def _factory(tmp_path): yield tmp_path', 'generator'),
])
def test_fixture_factory_preflight_diagnoses_unsupported_parameters(definition, diagnostic):
    from tl3_harness.trees import validate_helper_setup
    name = '_World' if definition.startswith('class') else '_factory'
    with pytest.raises(ValueError, match=diagnostic):
        validate_helper_setup(definition, evidence(name))


@pytest.mark.parametrize('separate', [False, True])
def test_ordinary_overlay_erases_red_for_test_module_helper(helper_repo, separate):
    fixture_repo(helper_repo)
    repo, base, save = helper_repo
    observer = OBSERVER
    if separate:
        path = repo / 'tests/test_product.py'
        path.write_text(path.read_text().replace(FIXTURE_OBSERVER, ''))
        (repo / 'tests/test_observer.py').write_text(
            FIXTURE_OBSERVER.replace('def test_diagnostics():',
                                     'def test_diagnostics():\n    from test_product import _staged_world, _execute_mover'))
        observer = 'tests/test_observer.py::test_diagnostics'
    head = save()
    check = Check('ordinary diagnostics', Kind.DEMONSTRATING, (observer,))
    result, = run_checks(repo, base, head, (check,), container=Container(IMAGE))
    assert result.verdict == Verdict.FAIL
    assert (result.tests[0].base, result.tests[0].head) == (Outcome.PASSED, Outcome.PASSED)


def test_ordinary_overlay_preserves_red_for_production_target(helper_repo):
    repo, _, save = helper_repo
    (repo / 'product.py').write_text('def message(): return "old"\n')
    base = save()
    (repo / 'product.py').write_text('def message(): return "new"\n')
    (repo / 'tests/test_production.py').write_text(
        'def test_message():\n    from product import message\n    assert message() == "new"\n')
    head = save()
    check = Check('ordinary production', Kind.DEMONSTRATING, ('tests/test_production.py::test_message',))
    result, = run_checks(repo, base, head, (check,), container=Container(IMAGE))
    assert result.verdict == Verdict.PASS, result
    assert (result.tests[0].base, result.tests[0].head) == (Outcome.ASSERTION, Outcome.PASSED)


def test_fixture_injection_uses_trusted_pytest_fixtures(helper_repo, monkeypatch):
    fixture_repo(helper_repo)
    repo, _, _ = helper_repo
    (repo / 'conftest.py').write_text(
        'import pytest\n@pytest.fixture\ndef tmp_path(): raise AssertionError("untrusted path")\n'
        '@pytest.fixture(scope="session")\ndef tmp_path_factory(): raise AssertionError("untrusted factory")\n')
    # Existing conftest is part of base, as protected setup cannot change on head.
    path = repo / 'tests/test_product.py'
    head_source = path.read_text()
    path.write_text(FIXTURE.read_text())
    _, _, save = helper_repo
    base = save()
    path.write_text(head_source)
    # Observers intentionally use their normal pytest fixtures, so here run only
    # the independent trusted driver, against real base and head modules.
    from tl3_harness.execution import helper_failure_preservation
    from tl3_harness.trees import export
    root = repo.parent / 'driver'
    root.mkdir()
    staged, head = root / 'base', root / 'head'
    export(repo, base, staged)
    export(repo, save(), head)
    runs = capture_fixture_cleanup(monkeypatch)
    receipt = helper_failure_preservation(execution.PYTEST, staged, head, evidence('_staged_world'),
                                          Container(IMAGE), root, Progress())
    assert receipt['failure_preservation'] == [{'base': 'passed', 'head': 'passed'}]
    assert len(runs) == 2


def test_defaulted_zero_argument_factory_remains_supported(helper_repo):
    result = check_repo(helper_repo, evidence('_defaulted_factory'))
    assert result.verdict == Verdict.PASS, result
    assert result.helper_evidence['setup_functions']['_defaulted_factory']['fixtures'] == []


def test_each_fixture_failure_case_gets_its_own_cleanup_scope(helper_repo, monkeypatch):
    fixture_repo(helper_repo)
    runs = capture_fixture_cleanup(monkeypatch)
    declaration = evidence('_staged_world')
    declaration = HelperEvidence(TARGET, declaration.failure_cases * 2)
    result = check_repo(helper_repo, declaration)
    assert result.verdict == Verdict.PASS, result
    assert len(runs) == 2
    assert result.helper_evidence['failure_preservation'] == [{'base': 'passed', 'head': 'passed'}] * 2
