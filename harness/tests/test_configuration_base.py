"""Test configuration is judged against the commit the head descends from (#326)."""
import pytest

from tl3_harness import execution
from tl3_harness.model import Check, Kind, Outcome, Verdict
from tl3_harness.trees import git

CONFIG = '[tool.tl3.harness]\ncommand=["python", "-m", "pytest"]\ntest-files=["tests/test_*.py"]\n'
TEST = 'tests/test_value.py::test_value'
PLAN = (Check('value', Kind.REGRESSION, (TEST,)),)
CONTAINER = execution.Container('sha256:' + '1' * 64)


def save(repo, message):
    git(repo, 'add', '--all')
    git(repo, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
        'commit', '-qm', message)
    return git(repo, 'rev-parse', 'HEAD').decode().strip()


@pytest.fixture
def repo(tmp_path, monkeypatch):
    repo = tmp_path / 'repo'
    (repo / 'tests').mkdir(parents=True)
    git(repo, 'init', '-q', '-b', 'main')
    (repo / 'pyproject.toml').write_text(CONFIG)
    (repo / 'tests/conftest.py').write_text('# shared fixtures, version 1\n')
    (repo / 'tests/test_value.py').write_text('from product import value\ndef test_value(): assert value() == 1\n')
    (repo / 'product.py').write_text('def value(): return 1\n')
    pinned = save(repo, 'main when the request was pinned')
    ran = []

    def run_tree(tree, container, tests, *args, **kwargs):
        ran.append(tree.name)
        return dict.fromkeys(tests, Outcome.PASSED)

    monkeypatch.setattr(execution, 'require_admission', lambda: None)
    monkeypatch.setattr(execution, 'run_tree', run_tree)
    monkeypatch.setenv('PRISMABUILD_CONTAINER_OWNER', str(tmp_path))
    return repo, pinned, ran


def test_main_changing_configuration_before_the_head_is_not_refused(repo):
    repo, pinned, ran = repo
    (repo / 'tests/conftest.py').write_text('# shared fixtures, version 2\n')
    main = save(repo, 'main isolates queue-space observations')
    (repo / 'product.py').write_text('def value(): return 1  # maker\n')
    head = save(repo, 'maker')
    results = execution.run_checks(repo, main, head, PLAN, container=CONTAINER)
    assert [result.verdict for result in results] == [Verdict.PASS]


def test_configuration_is_judged_against_the_base_the_head_descends_from(repo):
    # The head sits on the pinned main; the check base is a newer main that changed
    # conftest.py. Only the head's own changes may be judged.
    repo, pinned, ran = repo
    (repo / 'product.py').write_text('def value(): return 1  # maker\n')
    head = save(repo, 'maker')
    git(repo, 'checkout', '-q', '--detach', pinned)
    (repo / 'tests/conftest.py').write_text('# shared fixtures, version 2\n')
    main = save(repo, 'main isolates queue-space observations')
    results = execution.run_checks(repo, main, head, PLAN, container=CONTAINER)
    assert [result.verdict for result in results] == [Verdict.PASS]
    assert ran, 'the checks must run'


def test_head_configuration_change_is_a_failing_check_naming_every_file(repo):
    repo, pinned, ran = repo
    (repo / 'tests/conftest.py').write_text('def pytest_collection_modifyitems(items): items.clear()\n')
    (repo / 'pytest.ini').write_text('[pytest]\naddopts = -p no:randomly\n')
    head = save(repo, 'maker edits configuration')
    results = execution.run_checks(repo, pinned, head, PLAN, container=CONTAINER)
    assert [result.verdict for result in results] == [Verdict.FAIL]
    message = results[0].tests[0].message
    assert 'tests/conftest.py' in message and 'pytest.ini' in message
    assert not results[0].tests[0].passed
    assert ran == [], 'changed test configuration must never run'


def test_edited_existing_test_is_refused_beside_a_reproducing_check(repo):
    # Every review fix round carries a reproducing check. Judging the reviewed tree's
    # configuration must not hide an existing test the fix round edited to pass.
    repo, pinned, ran = repo
    (repo / 'product.py').write_text('def value(): return 2\n')
    reviewed = save(repo, 'reviewed head')
    (repo / 'tests/test_value.py').write_text('from product import value\ndef test_value(): assert value() == 2\n')
    (repo / 'tests/test_fix.py').write_text('def test_fix(): assert True\n')
    head = save(repo, 'fix round edits the existing test')
    plan = (Check('reproducer', Kind.REPRODUCING, ('tests/test_fix.py::test_fix',)),
            Check('value', Kind.REGRESSION, (TEST,)))
    results = execution.run_checks(repo, pinned, head, plan, container=CONTAINER, reviewed=reviewed)
    assert results[1].verdict == Verdict.FAIL
    assert results[1].tests[0].message.startswith(f'Restore the existing test `{TEST}`')


def test_reviewed_configuration_change_is_a_failing_check(repo):
    repo, pinned, ran = repo
    (repo / 'tests/conftest.py').write_text('def pytest_collection_modifyitems(items): items.clear()\n')
    reviewed = save(repo, 'reviewed head edits configuration')
    (repo / 'product.py').write_text('def value(): return 1  # fix\n')
    head = save(repo, 'fix round')
    plan = (Check('reproducer', Kind.REPRODUCING, (TEST,)),)
    results = execution.run_checks(repo, pinned, head, plan, container=CONTAINER, reviewed=reviewed)
    assert [result.verdict for result in results] == [Verdict.FAIL]
    assert 'tests/conftest.py' in results[0].tests[0].message
    assert ran == []
