"""Checks refuse changes to the repository's protected paths (#390, per-repo rule of #297)."""
import pytest

from tl3_harness import execution
from tl3_harness.model import Check, Kind, Outcome, Verdict
from tl3_harness.trees import git

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
    monkeypatch.setenv('PRISMABUILD_CONTAINER_OWNER', str(tmp_path))
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
