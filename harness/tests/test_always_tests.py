"""Repo-wide scanning rules run alongside the plan on every round (#498)."""
import json
from pathlib import Path
import subprocess
import sys

import pytest

from tl3_harness import execution
from tl3_harness.model import Check, HarnessInputError, Kind, Outcome, Verdict
from tl3_harness.report import decode, parse
from tl3_harness.trees import git

RULE = 'tests/test_rules.py'
CONTAINER = execution.Container('sha256:' + '1' * 64)
PLAN = (Check('planned', Kind.REGRESSION, ('tests/test_other.py::test_other',)),)
RUNNER = Path(__file__).parents[1] / 'src/tl3_harness/runner.py'


def save(repo):
    git(repo, 'add', '--all')
    git(repo, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'fixture')
    return git(repo, 'rev-parse', 'HEAD').decode().strip()


@pytest.fixture
def repo(tmp_path, monkeypatch):
    repo = tmp_path / 'repo'
    (repo / 'tests').mkdir(parents=True)
    git(repo, 'init', '-q')
    (repo / 'pyproject.toml').write_text(
        '[tool.tl3.harness]\ncommand=["python", "-m", "pytest"]\ntest-files=["tests/*.py"]\n')
    (repo / RULE).write_text('''from pathlib import Path


def test_no_sleep():
    for test in Path("tests").glob("test_*.py"):
        assert "time.sleep(" not in test.read_text(), "Tests must not sleep: " + str(test)
'''.replace('"time.sleep("', '"time." + "sleep("'))
    (repo / 'tests/test_other.py').write_text('def test_other():\n    assert True\n')
    monkeypatch.setattr(execution, 'require_admission', lambda: None)
    monkeypatch.setenv('PRISMABUILD_CONTAINER_OWNER', str(repo))

    def run(tree, container, tests, report, progress, additions=frozenset(), *, messages=None):
        child = subprocess.run([sys.executable, '-I', '-B', str(RUNNER), json.dumps(tests), str(tree)],
                               capture_output=True)
        return parse([decode(line) for line in child.stdout.splitlines()], tests, set(),
                     child.returncode, additions, messages=messages)

    monkeypatch.setattr(execution, 'run_tree', run)
    return repo


def break_rule(repo):
    (repo / 'tests/test_new.py').write_text('import time\n\ndef test_new():\n    time.sleep(0)\n')
    return save(repo)


@pytest.mark.parametrize('selector', [RULE, RULE + '::test_no_sleep'])
def test_scanning_rule_fails_round_zero_and_fix_round_with_assertion(repo, selector):
    base = save(repo)
    for round_number in range(2):
        head = break_rule(repo)
        results = execution.run_checks(repo, base, head, PLAN, container=CONTAINER, always_tests=(selector,))
        assert len(results) == 2
        result = results[1]
        assert result.kind == Kind.REGRESSION
        assert result.verdict == Verdict.FAIL
        test, = result.tests
        assert test.test == selector
        assert (test.base, test.head, test.pre_existing) == (Outcome.PASSED, Outcome.ASSERTION, False)
        assert 'Tests must not sleep: tests/test_new.py' in test.message
        (repo / 'tests/test_other.py').write_text(f'def test_other():\n    assert {round_number} >= 0\n')


def test_scanning_rule_already_failing_on_base_is_pre_existing(repo):
    base = break_rule(repo)
    (repo / 'tests/test_other.py').write_text('def test_other():\n    assert 1 == 1\n')
    results = execution.run_checks(repo, base, save(repo), PLAN, container=CONTAINER, always_tests=(RULE,))
    assert len(results) == 2
    test, = results[1].tests
    assert results[1].verdict == Verdict.PASS
    assert test.pre_existing and test.passed
    assert (test.base, test.head) == (Outcome.ASSERTION, Outcome.ASSERTION)


def test_empty_list_leaves_the_plan_unchanged(repo):
    base = save(repo)
    result, = execution.run_checks(repo, base, break_rule(repo), PLAN, container=CONTAINER, always_tests=())
    assert result.name == 'planned' and result.verdict == Verdict.PASS


@pytest.mark.parametrize('selector', ['/tmp/test_rule.py', '../test_rule.py', 'tests/../test_rule.py',
                                     'tests/*.py', 'tests/test_?.py', 'tests/[ab].py', 'C:/test_rule.py',
                                     'tests/test_rule.py::', '-test_rule.py'])
def test_always_tests_rejects_unsafe_selectors(repo, selector):
    base = save(repo)
    with pytest.raises(HarnessInputError):
        execution.run_checks(repo, base, base, PLAN, container=CONTAINER, always_tests=(selector,))


@pytest.mark.parametrize('contents', ['', 'import pytest\n\n@pytest.mark.skip(reason="fixture")\ndef test_rule():\n    pass\n'])
def test_empty_or_skipped_rule_cannot_pass(repo, contents):
    (repo / RULE).write_text(contents)
    base = save(repo)
    results = execution.run_checks(repo, base, base, PLAN, container=CONTAINER, always_tests=(RULE,))
    assert len(results) == 2
    assert results[1].verdict == Verdict.FAIL


def test_passing_rule_and_duplicate_entries_run_once(repo):
    base = save(repo)
    results = execution.run_checks(repo, base, base, PLAN, container=CONTAINER, always_tests=(RULE, RULE))
    assert len(results) == 2
    assert results[1].verdict == Verdict.PASS
    assert [test.test for test in results[1].tests] == [RULE]


@pytest.mark.parametrize('selector', [RULE, 'old_rules/test_removed.py', RULE + '::test_removed',
                                     RULE + '::test_no_sleep[removed]'])
def test_stale_rule_is_nonblocking_and_its_note_survives_serialization(repo, monkeypatch, selector):
    if '::' not in selector:
        (repo / RULE).unlink()
    base = save(repo)
    head = break_rule(repo)
    runs = []
    original = execution.run_tree

    def record(tree, container, tests, *args, **kwargs):
        runs.extend(tests)
        return original(tree, container, tests, *args, **kwargs)

    monkeypatch.setattr(execution, 'run_tree', record)
    results = execution.run_checks(repo, base, head, PLAN, container=CONTAINER, always_tests=(selector,))
    assert all(result.verdict == Verdict.PASS for result in results)
    ignored, = results[1].tests
    assert ignored.passed and not ignored.pre_existing
    assert selector in ignored.message and 'stale' in ignored.message and 'both' in ignored.message
    report = json.loads(execution.result_json(results))
    assert report[1]['tests'][0]['message'] == ignored.message
    if '::' not in selector:
        assert selector not in runs


@pytest.mark.parametrize('selector', [RULE, RULE + '::test_no_sleep'])
def test_head_deletion_of_a_rule_that_existed_on_base_still_fails(repo, selector):
    base = save(repo)
    if '::' in selector:
        (repo / RULE).write_text('def test_replacement():\n    assert True\n')
    else:
        (repo / RULE).unlink()
    results = execution.run_checks(repo, base, save(repo), PLAN, container=CONTAINER, always_tests=(selector,))
    assert results[1].verdict == Verdict.FAIL
    test, = results[1].tests
    assert not test.passed and test.base == Outcome.PASSED
    assert 'stale' not in test.message


@pytest.mark.parametrize('kind', list(Kind))
def test_planned_checks_still_require_exact_node_ids(kind):
    with pytest.raises(HarnessInputError):
        Check('planned', kind, (RULE,))


@pytest.mark.parametrize('selector', [RULE, RULE + '::test_no_sleep'])
def test_rule_test_cannot_be_weakened_to_hide_a_violation(repo, selector):
    base = save(repo)
    (repo / RULE).write_text('def test_no_sleep():\n    assert True\n')
    head = break_rule(repo)
    results = execution.run_checks(repo, base, head, PLAN, container=CONTAINER, always_tests=(selector,))
    test, = results[1].tests
    assert results[1].verdict == Verdict.FAIL and not test.passed
    assert 'Restore the existing test' in test.message


def test_stale_notes_survive_when_planned_failures_fill_the_message_budget(repo):
    (repo / RULE).unlink()
    base = save(repo)
    (repo / 'tests/test_other.py').write_text('def test_other():\n    assert False\n')
    plan = tuple(Check(f'planned-{n}', Kind.REGRESSION, PLAN[0].tests) for n in range(20))
    results = execution.run_checks(repo, base, save(repo), plan, container=CONTAINER, always_tests=(RULE,))
    assert results[-1].verdict == Verdict.PASS
    report = json.loads(execution.result_json(results))
    assert RULE in report[-1]['tests'][0]['message']


def test_harness_also_bounds_always_tests_before_execution(repo):
    base = save(repo)
    entries = tuple(f'tests/test_rule_{n}.py' for n in range(21))
    with pytest.raises(HarnessInputError, match='always_tests.*at most 20'):
        execution.run_checks(repo, base, base, PLAN, container=CONTAINER, always_tests=entries)
