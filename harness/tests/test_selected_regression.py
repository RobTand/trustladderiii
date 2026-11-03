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
CORE_UNIT = ['tests/test_core.py']
FARTHER_UNITS = ['tests/test_api.py', 'tests/test_helped.py']


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
        observed = {}
        collected = []
        for test in tests:
            members = execution.selection.node_ids(tree, test) if '::' not in test else [test]
            collected.extend(members)
            observed[test] = next((outcomes[tree.name, member] for member in members
                                   if outcomes.get((tree.name, member), Outcome.PASSED) != Outcome.PASSED), Outcome.PASSED)
        if kwargs.get('selected') is not None:
            kwargs['selected'].collected = tuple(collected)
        return observed

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
    assert [test.test for test in results[1].tests] == CORE_UNIT + FARTHER_UNITS
    assert [result.verdict for result in results] == [Verdict.PASS, Verdict.PASS]
    assert any(tree == 'base' and set(CORE_UNIT) <= set(tests) for tree, tests in runs), runs


def test_a_selected_test_failing_on_the_base_is_dropped_and_a_head_failure_fails_the_round(repo):
    repo, base, runs, outcomes = repo
    head = change_core(repo)
    outcomes[('base', FARTHER[0])] = Outcome.ASSERTION
    outcomes[('base', FARTHER[1])] = Outcome.SKIPPED
    outcomes[('head', CORE[0])] = Outcome.ASSERTION
    results = execution.run_checks(repo, base, head, PLAN, container=CONTAINER, selected_tests=50)
    selected = results[1]
    assert [test.test for test in selected.tests] == CORE_UNIT
    assert selected.verdict == Verdict.FAIL
    assert [test.passed for test in selected.tests] == [False]


def test_edited_modules_are_excluded_without_removing_planned_companions(repo):
    repo, base, runs, outcomes = repo
    write(repo, {'pkg/core.py': 'def value():\n    return 1  # changed\n',
                 'tests/test_api.py': 'import pkg.api\n\ndef test_api():\n    assert pkg.api.api()\n'})
    head = save(repo, 'leaf changes pkg.core and edits an existing test')
    plan = PLAN + (Check('core', Kind.REGRESSION, (CORE[0],)),)
    results = execution.run_checks(repo, base, head, plan, container=CONTAINER, selected_tests=50)
    assert [test.test for test in results[-1].tests] == CORE_UNIT + [FARTHER_UNITS[1]]


def test_the_limit_keeps_the_closest_tests(repo):
    repo, base, runs, outcomes = repo
    head = change_core(repo)
    results = execution.run_checks(repo, base, head, PLAN, container=CONTAINER, selected_tests=2)
    assert [test.test for test in results[1].tests] == CORE_UNIT


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


def test_selection_expands_lazily_and_stops_at_the_limit(repo, monkeypatch):
    # Review of #409: PrismaQuant's allocator.py reaches 10,908 tests in 1,003 files.
    repo, base, runs, outcomes = repo
    head = change_core(repo)
    expanded, judged = [], []
    node_ids, changed_existing_tests = execution.selection.node_ids, execution.Pytest.changed_existing_tests
    monkeypatch.setattr(execution.selection, 'node_ids',
                        lambda tree, name: expanded.append(name) or node_ids(tree, name))
    monkeypatch.setattr(execution.Pytest, 'changed_existing_tests',
                        lambda self, old, new, tests: judged.extend(tests) or changed_existing_tests(self, old, new, tests))
    results = execution.run_checks(repo, base, head, PLAN, container=CONTAINER, selected_tests=2)
    assert [test.test for test in results[1].tests] == CORE_UNIT
    assert set(expanded) == {'tests/test_core.py'}, 'stop expanding files once the limit is reached'
    assert set(judged) <= {OTHER, *CORE_UNIT}, 'only judge edits of tests that can still be selected'


def test_candidates_run_on_the_base_apart_from_the_plan(repo):
    repo, base, runs, outcomes = repo
    head = change_core(repo)
    execution.run_checks(repo, base, head, PLAN, container=CONTAINER, selected_tests=50)
    assert ('base', (OTHER,)) in runs, 'the plan base run is exactly as without selection'
    assert ('base', tuple(CORE_UNIT + FARTHER_UNITS)) in runs
    runs.clear()
    write(repo, {'pkg/core.py': 'def value():\n    return 1  # changed again\n'})
    newer = save(repo, 'a newer head')
    execution.run_checks(repo, base, newer, PLAN, container=CONTAINER, selected_tests=50)
    assert [tests for tree, tests in runs if tree == 'base'] == [], 'base results are cached per complete unit'


def test_a_candidate_crashing_the_base_leaves_the_plan_alone(repo, monkeypatch):
    repo, base, runs, outcomes = repo
    head = change_core(repo)
    real = execution.run_tree

    def crashing(tree, container, tests, *args, **kwargs):
        observed = real(tree, container, tests, *args, **kwargs)
        return dict.fromkeys(tests, Outcome.NO_REPORT) if CORE_UNIT[0] in tests and tree.name == 'base' else observed

    monkeypatch.setattr(execution, 'run_tree', crashing)
    results = execution.run_checks(repo, base, head, PLAN, container=CONTAINER, selected_tests=50)
    assert [result.name for result in results] == ['other']
    assert results[0].verdict == Verdict.PASS


def test_a_head_failure_that_passes_in_its_complete_unit_is_reported_as_flaky(repo):
    repo, base, runs, outcomes = repo
    head = change_core(repo)
    calls = []

    def once(tree, test):
        calls.append(test)
        return len(calls) == 1

    real = execution.run_tree
    execution.run_tree = lambda tree, container, tests, *args, **kwargs: {
        test: (Outcome.ASSERTION if tree.name == 'head' and test == CORE_UNIT[0] and once(tree, test) else outcome)
        for test, outcome in real(tree, container, tests, *args, **kwargs).items()}
    try:
        results = execution.run_checks(repo, base, head, PLAN, container=CONTAINER, selected_tests=50)
    finally:
        execution.run_tree = real
    selected = results[1]
    assert selected.verdict == Verdict.PASS
    flaky = next(test for test in selected.tests if test.test == CORE_UNIT[0])
    assert flaky.passed and 'complete execution group' in flaky.message
    assert ('head', tuple(CORE_UNIT)) in runs, 'the failing unit reruns complete'
    assert ('head', (OTHER,)) in runs, 'the plan head run is exactly as without selection'


def test_a_unit_crashing_the_head_fails_complete_without_infrastructure(repo, monkeypatch):
    repo, base, runs, outcomes = repo
    head = change_core(repo)
    real = execution.run_tree

    def crashing(tree, container, tests, *args, **kwargs):
        observed = real(tree, container, tests, *args, **kwargs)
        if tree.name == 'head' and CORE_UNIT[0] in tests:
            return dict.fromkeys(tests, Outcome.NO_REPORT)
        return observed

    monkeypatch.setattr(execution, 'run_tree', crashing)
    results = execution.run_checks(repo, base, head, PLAN, container=CONTAINER, selected_tests=50)
    assert results[0].verdict == Verdict.PASS
    selected = results[1]
    assert selected.verdict == Verdict.FAIL
    assert [test.test for test in selected.tests if not test.passed] == CORE_UNIT
    assert selected.tests[0].head == Outcome.ERROR


@pytest.mark.parametrize('pathological', ['a = ' + 'a+' * 200_000 + 'a\n', 'def broken(:\n'],
                         ids=['200000-term-expression', 'syntax-error'])
def test_a_pathological_base_file_never_fails_the_round(repo, pathological):
    repo, base, runs, outcomes = repo
    write(repo, {'pkg/huge.py': pathological, 'tests/test_huge.py': 'import pkg.huge\nimport pkg.core\n'
                 'def test_huge():\n    assert True\n'})
    base = save(repo, 'base with a pathological module')
    head = change_core(repo)
    results = execution.run_checks(repo, base, head, PLAN, container=CONTAINER, selected_tests=50)
    assert results[0].name == 'other'


def selected(tmp_path, files, changed, tests_pattern='tests/*.py'):
    from tl3_harness import selection
    from tl3_harness.trees import is_test
    write(tmp_path, files)
    return list(selection.select(tmp_path, changed, lambda name: is_test(name, [tests_pattern])))


def test_relative_imports_reach_their_package(tmp_path):
    assert selected(tmp_path, {
        'pkg/__init__.py': '', 'pkg/core.py': 'X = 1\n',
        'pkg/sub/__init__.py': '', 'pkg/sub/user.py': 'from ..core import X\n',
        'pkg/sub/other.py': 'from . import user\n',
        'tests/test_other.py': 'from pkg.sub import other\ndef test_other():\n    pass\n',
    }, ['pkg/core.py']) == ['tests/test_other.py::test_other']


def test_src_layouts_import_without_src(tmp_path):
    assert selected(tmp_path, {
        'src/pkg/__init__.py': '', 'src/pkg/core.py': 'X = 1\n',
        'tests/test_core.py': 'from pkg.core import X\ndef test_core():\n    pass\n',
    }, ['src/pkg/core.py']) == ['tests/test_core.py::test_core']


def test_import_cycles_terminate(tmp_path):
    assert selected(tmp_path, {
        'pkg/__init__.py': '', 'pkg/a.py': 'import pkg.b\n', 'pkg/b.py': 'import pkg.a\n',
        'tests/test_b.py': 'import pkg.b\ndef test_b():\n    pass\n',
    }, ['pkg/a.py']) == ['tests/test_b.py::test_b']


def test_an_unparseable_file_is_skipped(tmp_path):
    assert selected(tmp_path, {
        'pkg/__init__.py': '', 'pkg/core.py': 'X = 1\n', 'pkg/broken.py': 'def broken(:\n',
        'tests/test_broken.py': 'import pkg.core\ndef broken(:\n',
        'tests/test_core.py': 'import pkg.core\ndef test_core():\n    pass\n',
    }, ['pkg/core.py']) == ['tests/test_core.py::test_core']


def test_a_changed_fixture_selects_the_test_that_reads_it_by_path(tmp_path):
    # #424: PQ's pin tests read a fixture by path and hash it; the change edited only the JSON.
    files = {
        'pkg/__init__.py': '', 'pkg/core.py': 'X = 1\n',
        'tests/fixtures/baseline.json': '{"scopes": 4}\n',
        'tests/test_pin.py': ('from pathlib import Path\nREPO = Path(__file__).resolve().parents[1]\n'
                              'def test_pin():\n    assert (REPO / "tests/fixtures/baseline.json").read_bytes()\n'),
        'tests/test_core.py': 'import pkg.core\ndef test_core():\n    pass\n',
    }
    assert selected(tmp_path, files, ['tests/fixtures/baseline.json']) == ['tests/test_pin.py::test_pin']


def test_a_path_chain_names_the_changed_file(tmp_path):
    assert selected(tmp_path, {
        'tests/fixtures/baseline.json': '{}\n',
        'tests/test_chain.py': ('from pathlib import Path\ndef test_chain():\n'
                                '    assert (Path(__file__).parent / "fixtures" / "baseline.json").exists()\n'),
    }, ['tests/fixtures/baseline.json']) == ['tests/test_chain.py::test_chain']


def test_a_literal_naming_another_file_selects_nothing(tmp_path):
    assert selected(tmp_path, {
        'tests/fixtures/baseline.json': '{}\n', 'tests/fixtures/other.json': '{}\n',
        'tests/test_other.py': ('from pathlib import Path\ndef test_other():\n'
                                '    assert Path("tests/fixtures/other.json").exists()\n'
                                '    assert Path("other/tests/fixtures/baseline.json.bak")\n'),
    }, ['tests/fixtures/baseline.json']) == []


def test_a_python_file_read_by_path_is_selected_like_any_file(tmp_path):
    assert selected(tmp_path, {
        'tools/generate.py': 'print("generated")\n',
        'tests/test_generate.py': ('import runpy\ndef test_generate():\n'
                                   '    runpy.run_path("tools/generate.py")\n'),
    }, ['tools/generate.py']) == ['tests/test_generate.py::test_generate']


def test_path_literal_hits_rank_with_direct_changes(tmp_path):
    # A test naming the changed file by path ranks at distance 0, before any importer.
    assert selected(tmp_path, {
        'pkg/__init__.py': '', 'pkg/core.py': 'X = 1\n',
        'tests/test_importer.py': 'import pkg.core\ndef test_importer():\n    pass\n',
        'tests/test_reader.py': 'def test_reader():\n    assert open("pkg/core.py").read()\n',
    }, ['pkg/core.py']) == ['tests/test_reader.py::test_reader', 'tests/test_importer.py::test_importer']


def test_a_fixture_only_change_adds_the_selected_check(repo):
    repo, base, runs, outcomes = repo
    write(repo, {'tests/fixtures/baseline.json': '{"scopes": 4}\n',
                 'tests/test_pin.py': ('from pathlib import Path\ndef test_pin():\n'
                                       '    assert Path("tests/fixtures/baseline.json").read_bytes()\n')})
    base = save(repo, 'base with a pin test')
    write(repo, {'tests/fixtures/baseline.json': '{"scopes": 3}\n'})
    head = save(repo, 'leaf edits only the fixture')
    results = execution.run_checks(repo, base, head, PLAN, container=CONTAINER, selected_tests=50)
    assert [result.name for result in results] == ['other', SELECTED]
    assert [test.test for test in results[1].tests] == ['tests/test_pin.py']
