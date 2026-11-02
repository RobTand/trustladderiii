"""Existing tests that import the change run as one more regression check (#359)."""
import pytest

from tl3_harness import execution
from tl3_harness.model import Check, HarnessInputError, Kind, Outcome, Verdict
from tl3_harness.trees import git

CONFIG = '[tool.tl3.harness]\ncommand=["python", "-m", "pytest"]\ntest-files=["tests/*.py"]\n'
OTHER = 'tests/test_other.py::test_other'
PLAN = (Check('other', Kind.REGRESSION, (OTHER,)),)
CONTAINER = execution.Container('sha256:' + '1' * 64)
SELECTED = execution.SELECTED_CHECK
FILES = {
    'pyproject.toml': CONFIG,
    'pkg/__init__.py': '',
    'pkg/core.py': 'def value():\n    return 1\n',
    'pkg/api.py': 'from pkg.core import value\n\ndef api():\n    return value()\n',
    'pkg/other.py': 'def other():\n    return 3\n',
    'tests/helpers.py': 'from pkg.core import value\n',
    'tests/test_core.py': ('from pkg import core\n\ndef test_core():\n    assert core.value() == 1\n\n'
                           'class TestCore:\n    def test_method(self):\n        assert core.value()\n'),
    'tests/test_api.py': 'import pkg.api\n\ndef test_api():\n    assert pkg.api.api() == 1\n',
    'tests/test_helped.py': 'from tests.helpers import value\n\ndef test_helped():\n    assert value() == 1\n',
    'tests/test_other.py': 'import pkg.other\n\ndef test_other():\n    assert pkg.other.other() == 3\n',
    'docs/guide.md': 'guide\n',
}
CORE = ['tests/test_core.py::test_core', 'tests/test_core.py::TestCore::test_method']
FARTHER = ['tests/test_api.py::test_api', 'tests/test_helped.py::test_helped']


def save(repo, message):
    git(repo, 'add', '--all')
    git(repo, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
        'commit', '-qm', message)
    return git(repo, 'rev-parse', 'HEAD').decode().strip()


def write(repo, files):
    for name, text in files.items():
        (repo / name).parent.mkdir(parents=True, exist_ok=True)
        (repo / name).write_text(text)


@pytest.fixture
def repo(tmp_path, monkeypatch):
    repo = tmp_path / 'repo'
    repo.mkdir()
    git(repo, 'init', '-q', '-b', 'main')
    write(repo, FILES)
    base = save(repo, 'base')
    runs, outcomes = [], {}

    def run_tree(tree, container, tests, *args, **kwargs):
        runs.append((tree.name, tests))
        return {test: outcomes.get((tree.name, test), Outcome.PASSED) for test in tests}

    monkeypatch.setattr(execution, 'require_admission', lambda: None)
    monkeypatch.setattr(execution, 'run_tree', run_tree)
    monkeypatch.setenv('PRISMABUILD_CONTAINER_OWNER', str(tmp_path))
    return repo, base, runs, outcomes


def change_core(repo):
    write(repo, {'pkg/core.py': 'def value():\n    return 1  # changed\n'})
    return save(repo, 'leaf changes pkg.core')


def test_tests_importing_the_change_run_as_a_regression_check(repo):
    repo, base, runs, outcomes = repo
    head = change_core(repo)
    results = execution.run_checks(repo, base, head, PLAN, container=CONTAINER, selected_tests=50)
    assert [result.name for result in results] == ['other', SELECTED]
    assert results[1].kind == Kind.REGRESSION
    assert [test.test for test in results[1].tests] == CORE + FARTHER
    assert [result.verdict for result in results] == [Verdict.PASS, Verdict.PASS]
    assert any(tree == 'base' and set(CORE) <= set(tests) for tree, tests in runs), runs


def test_a_selected_test_failing_on_the_base_is_dropped_and_a_head_failure_fails_the_round(repo):
    repo, base, runs, outcomes = repo
    head = change_core(repo)
    outcomes[('base', FARTHER[0])] = Outcome.ASSERTION
    outcomes[('base', FARTHER[1])] = Outcome.SKIPPED
    outcomes[('head', CORE[0])] = Outcome.ASSERTION
    results = execution.run_checks(repo, base, head, PLAN, container=CONTAINER, selected_tests=50)
    selected = results[1]
    assert [test.test for test in selected.tests] == CORE
    assert selected.verdict == Verdict.FAIL
    assert [test.passed for test in selected.tests] == [False, True]


def test_planned_and_edited_tests_are_not_selected(repo):
    repo, base, runs, outcomes = repo
    write(repo, {'pkg/core.py': 'def value():\n    return 1  # changed\n',
                 'tests/test_api.py': 'import pkg.api\n\ndef test_api():\n    assert pkg.api.api()\n'})
    head = save(repo, 'leaf changes pkg.core and edits an existing test')
    plan = PLAN + (Check('core', Kind.REGRESSION, (CORE[0],)),)
    results = execution.run_checks(repo, base, head, plan, container=CONTAINER, selected_tests=50)
    assert [test.test for test in results[-1].tests] == [CORE[1], FARTHER[1]]


def test_the_limit_keeps_the_closest_tests(repo):
    repo, base, runs, outcomes = repo
    head = change_core(repo)
    results = execution.run_checks(repo, base, head, PLAN, container=CONTAINER, selected_tests=2)
    assert [test.test for test in results[1].tests] == CORE


@pytest.mark.parametrize('selected_tests', [None, 50])
def test_nothing_is_added_without_selection_or_selected_tests(repo, selected_tests):
    repo, base, runs, outcomes = repo
    write(repo, {'docs/guide.md': 'edited\n'} if selected_tests else {'pkg/core.py': 'def value():\n    return 1  # changed\n'})
    head = save(repo, 'leaf')
    results = execution.run_checks(repo, base, head, PLAN, container=CONTAINER, selected_tests=selected_tests)
    assert [result.name for result in results] == ['other']


def test_a_plan_cannot_use_the_reserved_name(repo):
    repo, base, runs, outcomes = repo
    head = change_core(repo)
    with pytest.raises(HarnessInputError):
        execution.run_checks(repo, base, head, (Check(SELECTED, Kind.REGRESSION, (OTHER,)),),
                             container=CONTAINER, selected_tests=50)
