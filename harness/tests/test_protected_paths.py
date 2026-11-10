"""Checks refuse changes to the repository's protected paths (#390, per-repo rule of #297)."""
import pytest

from tl3_harness import execution
from tl3_harness.model import Check, Kind, Outcome, Verdict
from tl3_harness.trees import git
from conftest import flux_context

CONFIG = '[tool.tl3.harness]\ncommand=["python", "-m", "pytest"]\ntest-files=["tests/test_*.py"]\n'
TEST = 'tests/test_value.py::test_value'
PLAN = (Check('value', Kind.REGRESSION, (TEST,)),)
CONTAINER = execution.Container('sha256:' + '1' * 64)
PROTECTED = ('.github/**', 'Cargo.toml', 'deploy/*', 'release/**')


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
    write(repo, {'pyproject.toml': CONFIG, 'tests/test_value.py': 'def test_value(): assert True\n',
                 'Cargo.toml': '[workspace]\n', 'crates/a/Cargo.toml': '[package]\n',
                 'deploy/run.sh': 'run\n', 'deploy/units/tl3.service': 'unit\n',
                 'release/notes/1.md': 'one\n', 'docs/guide.md': 'guide\n'})
    base = save(repo, 'base')
    ran = []

    def run_tree(tree, container, tests, *args, **kwargs):
        ran.append(tree.name)
        return dict.fromkeys(tests, Outcome.PASSED)

    monkeypatch.setattr(execution, 'require_admission', lambda: None)
    monkeypatch.setattr(execution, 'run_tree', run_tree)
    flux_context(monkeypatch)
    return repo, base, ran


@pytest.mark.parametrize('changed', ['.github/workflows/gate.yml', 'Cargo.toml', 'deploy/run.sh',
                                     'release/notes/1.md'])
def test_a_protected_path_change_is_a_failing_check_naming_it(repo, changed):
    repo, base, ran = repo
    write(repo, {changed: 'changed by the leaf\n', 'docs/guide.md': 'edited\n'})
    head = save(repo, 'leaf')
    results = execution.run_checks(repo, base, head, PLAN, container=CONTAINER, protected=PROTECTED)
    assert [result.verdict for result in results] == [Verdict.FAIL]
    message = results[0].tests[0].message
    assert message.startswith('Restore the changed protected files')
    assert changed in message and 'docs/guide.md' not in message
    assert ran == [], 'a changed protected path must never run'


@pytest.mark.parametrize('changed', ['crates/a/Cargo.toml', 'deploy/units/tl3.service', 'docs/guide.md'])
def test_patterns_match_as_git_globs_do(repo, changed):
    # A pattern without a slash prefix is still anchored at the root, and `*` stops at `/`,
    # as in the worktree adapter's `:(top,glob)` pathspecs.
    repo, base, ran = repo
    write(repo, {changed: 'changed by the leaf\n'})
    head = save(repo, 'leaf')
    results = execution.run_checks(repo, base, head, PLAN, container=CONTAINER, protected=PROTECTED)
    assert [result.verdict for result in results] == [Verdict.PASS]


def test_main_changing_a_protected_path_before_the_head_is_not_refused(repo):
    repo, pinned, ran = repo
    write(repo, {'docs/guide.md': 'leaf\n'})
    head = save(repo, 'leaf')
    git(repo, 'checkout', '-q', '--detach', pinned)
    write(repo, {'deploy/run.sh': 'main moved\n'})
    main = save(repo, 'main changes a protected path')
    results = execution.run_checks(repo, main, head, PLAN, container=CONTAINER, protected=PROTECTED)
    assert [result.verdict for result in results] == [Verdict.PASS]


def test_reviewed_commit_protected_change_is_refused(repo):
    repo, base, ran = repo
    write(repo, {'deploy/run.sh': 'reviewed head changed this\n'})
    reviewed = save(repo, 'reviewed')
    # The head restores it, so only the reviewed tree, the reproducing run's before tree,
    # carries the change; it is judged as configuration changes there are.
    write(repo, {'deploy/run.sh': 'run\n', 'docs/guide.md': 'fix\n'})
    head = save(repo, 'fix round')
    plan = (Check('reproducer', Kind.REPRODUCING, (TEST,)),)
    results = execution.run_checks(repo, base, head, plan, container=CONTAINER, reviewed=reviewed,
                                   protected=PROTECTED)
    assert [result.verdict for result in results] == [Verdict.FAIL]
    assert 'deploy/run.sh' in results[0].tests[0].message


@pytest.mark.parametrize('filename', ['pytest.toml', '.pytest.toml'])
@pytest.mark.parametrize('directory', ['', 'nested/tests/'])
@pytest.mark.parametrize('operation', ['add', 'modify', 'delete', 'rename_in', 'rename_out'])
def test_c6_pytest_configuration_changes_are_refused(repo, filename, directory, operation):
    repo, base, ran = repo
    name = directory + filename
    if operation in ('modify', 'delete', 'rename_out'):
        write(repo, {name: '# before\n'})
    if operation == 'rename_in':
        write(repo, {directory + 'other.toml': ''})
    if operation != 'add':
        base = save(repo, 'configuration baseline')
    if operation in ('add', 'modify'):
        write(repo, {name: ''})
    elif operation == 'delete':
        (repo / name).unlink()
    elif operation == 'rename_in':
        (repo / (directory + 'other.toml')).rename(repo / name)
    else:
        (repo / name).rename(repo / (directory + 'other.toml'))
    head = save(repo, operation)
    results = execution.run_checks(repo, base, head, PLAN, container=CONTAINER)
    assert [r.verdict for r in results] == [Verdict.FAIL], 'pytest configuration must be refused'
    assert name in results[0].tests[0].message
    assert ran == []


@pytest.mark.parametrize('name', [
    'requirements.txt', 'requirements-dev.txt', 'requirements.in', 'uv.lock',
    'setup.py', 'setup.cfg', 'poetry.lock', 'Pipfile', 'Pipfile.lock',
    'constraints.txt', 'constraints-ci.txt', 'constraints.in', 'MANIFEST.in',
    'pyproject.toml',
])
@pytest.mark.parametrize('directory', ['', 'pkg/'])
def test_c6_dependency_changes_carry_coverage_note(repo, name, directory):
    import json
    repo, base, ran = repo
    name = directory + name
    write(repo, {name: CONFIG + '\n[project]\ndependencies=["new-package"]\n'
                 if name.endswith('pyproject.toml') else '# changed\n'})
    head = save(repo, 'dependency change')
    results = execution.run_checks(repo, base, head, PLAN, container=CONTAINER)
    expected = 'not covered by checks: dependency or packaging file changed: ' + name
    assert all(getattr(r, 'coverage_note', None) == expected for r in results), 'dependency change needs a coverage flag'
    assert all(r['coverage_note'] == expected for r in json.loads(execution.result_json(results)))
    # setup.cfg and pyproject.toml already configure tests, so their refusal wins.
    if name not in ('pyproject.toml', 'setup.cfg'):
        assert all(r.verdict == Verdict.PASS for r in results)
        assert ran


def test_c6_protected_refusal_keeps_dependency_note(repo):
    repo, base, ran = repo
    write(repo, {'pytest.toml': '', 'requirements.txt': '# changed\n'})
    head = save(repo, 'configuration and dependencies')
    results = execution.run_checks(repo, base, head, PLAN, container=CONTAINER)
    assert results[0].verdict == Verdict.FAIL, 'configuration refusal wins'
    assert getattr(results[0], 'coverage_note', None) == (
        'not covered by checks: dependency or packaging file changed: requirements.txt')
    assert ran == []


@pytest.mark.parametrize('name', ['PYTEST.TOML', '.Pytest.toml', 'nested/PyTest.toml'])
def test_c6_different_case_does_not_name_a_pytest_configuration(repo, name):
    repo, base, ran = repo
    write(repo, {name: ''})
    head = save(repo, 'different case')
    results = execution.run_checks(repo, base, head, PLAN, container=CONTAINER)
    assert all(r.verdict == Verdict.PASS for r in results)
    assert ran


def test_c6_configuration_lists_agree_and_are_complete():
    import ast
    from pathlib import Path
    import re
    from tl3_harness.trees import TEST_CONFIGURATION_PATHS

    source = (Path(__file__).resolve().parents[2] / 'crates/config/src/lib.rs').read_text()
    declaration = re.search(r'pub const TEST_CONFIGURATION_PATHS: &\[&str\] = &(\[.*?\]);',
                            source, re.S)
    assert declaration, 'the Rust defaults must expose the authoritative filename constant'
    assert tuple(ast.literal_eval(declaration[1])) == TEST_CONFIGURATION_PATHS
    assert {'**/conftest.py', 'pytest.toml', '.pytest.toml', '**/pytest.toml',
            '**/.pytest.toml', 'pytest.ini', '.pytest.ini', 'pyproject.toml',
            'tox.ini', 'setup.cfg', 'noxfile.py'} <= set(TEST_CONFIGURATION_PATHS)


def test_c6_every_configuration_pattern_is_exercised(tmp_path):
    from tl3_harness.trees import TEST_CONFIGURATION_PATHS, changed_configuration
    base, head = tmp_path / 'base', tmp_path / 'head'
    base.mkdir()
    head.mkdir()
    for pattern in TEST_CONFIGURATION_PATHS:
        for directory in ['', 'nested/tests/'] if pattern.startswith('**/') else ['']:
            name = directory + pattern.removeprefix('**/')
            write(head, {name: ''})
            assert changed_configuration(base, head) == [name]
            (head / name).unlink()


@pytest.mark.parametrize('operation', ['modify', 'delete', 'rename_in', 'rename_out'])
def test_c6_dependency_path_operations_are_flagged(repo, operation):
    repo, base, ran = repo
    before = 'nested/requirements-dev.txt'
    after = 'nested/constraints-ci.txt'
    write(repo, {before: '# before\n'})
    base = save(repo, 'dependency baseline')
    if operation == 'modify':
        write(repo, {before: '# after\n'})
        names = [before]
    elif operation == 'delete':
        (repo / before).unlink()
        names = [before]
    else:
        after = after if operation == 'rename_in' else 'nested/notes.txt'
        (repo / before).rename(repo / after)
        names = sorted([before, after]) if operation == 'rename_in' else [before]
    head = save(repo, operation)
    results = execution.run_checks(repo, base, head, PLAN, container=CONTAINER)
    assert all(r.verdict == Verdict.PASS for r in results)
    assert results[0].coverage_note == (
        'not covered by checks: dependency or packaging file changed: ' + ', '.join(names))


@pytest.mark.parametrize('name', ['tools/ci/gate_requirements.txt', 'resolved-requirements.txt'])
def test_c6_image_dependency_input_names_are_flagged(repo, name):
    repo, base, ran = repo
    write(repo, {name: '# dependency\n'})
    head = save(repo, 'frozen input convention')
    results = execution.run_checks(repo, base, head, PLAN, container=CONTAINER)
    assert all(r.verdict == Verdict.PASS for r in results)
    assert results[0].coverage_note.endswith(': ' + name)


def test_c6_unrelated_files_do_not_carry_a_coverage_note(repo):
    import json
    repo, base, ran = repo
    write(repo, {'docs/guide.md': '# documentation\n', 'requirements.txt.backup': '# backup\n'})
    head = save(repo, 'unrelated files')
    results = execution.run_checks(repo, base, head, PLAN, container=CONTAINER)
    assert all(r.verdict == Verdict.PASS and r.coverage_note is None for r in results)
    assert all('coverage_note' not in r for r in json.loads(execution.result_json(results)))


@pytest.mark.parametrize('name', ['pytest.toml', '.pytest.toml',
                                  'nested/tests/pytest.toml', 'nested/tests/.pytest.toml'])
def test_c6_default_configuration_patterns_match_committed_paths(repo, name):
    from tl3_harness.trees import TEST_CONFIGURATION_PATHS, changed_protected
    repo, base, ran = repo
    write(repo, {name: ''})
    head = save(repo, 'pytest configuration')
    assert changed_protected(repo, base, head, TEST_CONFIGURATION_PATHS) == [name]
