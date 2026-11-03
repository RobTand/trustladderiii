"""Independent Checking must keep a frozen-row fixture's execution unit intact (#591)."""
import json
from pathlib import Path
import sys

import pytest

from tl3_harness import execution
from tl3_harness.model import Check, HarnessInputError, Kind, Outcome, Verdict
from tl3_harness.trees import git

RUNNER = Path(__file__).parents[1] / 'src/tl3_harness/runner.py'
CONTAINER = execution.Container('sha256:' + '1' * 64)
MODULE = 'tests/test_contract.py'
BINDING = MODULE + '::test_bindings_are_shared'
CALLS = MODULE + '::test_frozen_calls'
OTHER = 'tests/test_other.py::test_other'
PLAN = (Check('other', Kind.REGRESSION, (OTHER,)),)
CONFIG = '[tool.tl3.harness]\ncommand=["python", "-m", "pytest"]\ntest-files=["tests/*.py"]\n'
CONTRACT = '''from pathlib import Path
import os
import json
import pytest
from product import value

ROWS = json.loads(Path(__file__).with_name("frozen.json").read_text())
USED = set()

@pytest.fixture(scope="module", autouse=True)
def every_frozen_row_used():
    yield
    unused = set(ROWS) - USED
    assert not unused, f"{len(unused)} frozen row(s) that no passing call used"

@pytest.mark.parametrize("row", list(ROWS))
def test_frozen_calls(row):
    assert value() > 0, "real behavior failure"
    USED.add(row)

def test_bindings_are_shared():
    if value() == 2 and os.environ.get("TL3_GROUP_ATTEMPT") == "1":
        pytest.fail("one complete-group flake")
    assert callable(value)
'''


def save(repo, message):
    git(repo, 'add', '--all')
    git(repo, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
        'commit', '-qm', message)
    return git(repo, 'rev-parse', 'HEAD').decode().strip()


@pytest.fixture
def frozen_repo(tmp_path, monkeypatch):
    repo = tmp_path / 'repo'
    (repo / 'tests').mkdir(parents=True)
    git(repo, 'init', '-q')
    (repo / 'pyproject.toml').write_text(CONFIG)
    (repo / MODULE).write_text(CONTRACT)
    (repo / 'tests/frozen.json').write_text(json.dumps({str(row): row for row in range(393)}))
    (repo / 'tests/test_other.py').write_text('def test_other():\n    assert True\n')
    (repo / 'product.py').write_text('def value():\n    return 1\n')
    runs = []
    command = execution.container_command

    def real_runner(tree, container, tests):
        attempt = 1 + sum(name == tree.name and members == tests for name, members in runs)
        runs.append((tree.name, tests))
        return ["/usr/bin/env", f"TL3_GROUP_ATTEMPT={attempt}", sys.executable, '-I', '-B', str(RUNNER), '-', str(tree)]

    monkeypatch.setattr(execution, 'require_admission', lambda: None)
    monkeypatch.setattr(execution, 'container_command', real_runner)
    monkeypatch.setattr(execution.Progress, 'commit', lambda *_: None)
    monkeypatch.setenv('PRISMABUILD_CONTAINER_OWNER', str(repo))
    yield repo, runs
    monkeypatch.setattr(execution, 'container_command', command)


def head(repo, value=1):
    (repo / 'product.py').write_text(f'def value():\n    return {value}\n# changed\n')
    return save(repo, 'head')


def selected(repo, base, commit, limit=400, plan=PLAN, container=CONTAINER, failures=()):
    return execution.run_checks(repo, base, commit, plan, container=container, selected_tests=limit, selected_failures=failures)


def test_complete_fixture_unit_retries_without_manufacturing_393_unused_rows(frozen_repo):
    repo, runs = frozen_repo
    base = save(repo, 'base')
    results = selected(repo, base, head(repo, 2))
    result = results[-1]
    assert result.name == execution.SELECTED_CHECK
    assert result.verdict == Verdict.PASS, result
    assert [test.test for test in result.tests] == [MODULE]
    assert ('base', (MODULE,)) in runs
    assert [tests for tree, tests in runs if tree == 'head' and tests != (OTHER,)] == [(MODULE,), (MODULE,)]
    assert 'complete execution group' in result.tests[0].message
    receipt = json.loads(execution.result_json(results))[-1]
    assert receipt['tests'][0]['base'] == receipt['tests'][0]['head'] == 'passed'
    assert 'identical base/head collection 394 tests, sha256 ' in receipt['tests'][0]['message']


def test_frozen_guard_is_real_and_unchanged(frozen_repo, tmp_path):
    repo, _ = frozen_repo
    messages = {}
    outcome = execution.run_tree(repo, CONTAINER, (BINDING,), tmp_path / 'alone.jsonl',
                                 execution.Progress(), messages=messages)
    assert outcome[BINDING] == Outcome.ERROR
    assert '393 frozen row(s)' in messages[BINDING]
    complete = execution.run_tree(repo, CONTAINER, (MODULE,), tmp_path / 'complete.jsonl', execution.Progress())
    assert complete == {MODULE: Outcome.PASSED}
    assert len(json.loads((repo / 'tests/frozen.json').read_text())) == 393


def test_a_real_behavior_failure_in_a_complete_unit_still_fails(frozen_repo):
    repo, runs = frozen_repo
    base = save(repo, 'base')
    result = selected(repo, base, head(repo, 0))[-1]
    assert result.verdict == Verdict.FAIL
    assert 'real behavior failure' in result.tests[0].message
    assert [test.test for test in result.tests] == [MODULE]
    assert all(tests == (MODULE,) for tree, tests in runs if tree == 'head' and tests != (OTHER,))


def test_base_filter_drops_the_whole_failing_unit(frozen_repo):
    repo, runs = frozen_repo
    with (repo / MODULE).open('a') as target:
        target.write('\ndef test_upstream():\n    assert value() != 1\n')
    base = save(repo, 'base')
    results = selected(repo, base, head(repo, 3))
    assert [result.name for result in results] == ['other']
    assert ('base', (MODULE,)) in runs
    assert not any(tree == 'head' and MODULE in tests for tree, tests in runs)


def test_partial_base_cache_never_runs_a_missing_member_alone(frozen_repo):
    repo, runs = frozen_repo
    base = save(repo, 'base')
    commit = head(repo)
    selected(repo, base, commit, plan=PLAN + (Check('binding', Kind.REGRESSION, (BINDING,)),))
    runs.clear()
    result = selected(repo, base, commit)[-1]
    assert result.verdict == Verdict.PASS
    assert [test.test for test in result.tests] == [MODULE]
    assert not any(tree == 'base' and tests in ((BINDING,), (CALLS,)) for tree, tests in runs)


@pytest.mark.parametrize('cached', [False, True])
def test_whole_group_cost_obeys_the_original_node_budget(frozen_repo, cached):
    repo, _ = frozen_repo
    base = save(repo, 'base')
    commit = head(repo)
    if cached:
        selected(repo, base, commit)
    with pytest.raises(HarnessInputError, match='execution group.*394.*393'):
        selected(repo, base, commit, limit=393)


def test_product_driven_collection_change_is_a_selection_refusal(frozen_repo):
    repo, _ = frozen_repo
    path = repo / MODULE
    path.write_text(CONTRACT.replace('list(ROWS)', 'list(ROWS)[:392] if value() == 3 else list(ROWS)'))
    base = save(repo, 'base')
    with pytest.raises(HarnessInputError, match='collection.*execution group'):
        selected(repo, base, head(repo, 3))


@pytest.mark.parametrize('flaky', [False, True])
def test_declared_session_group_includes_unreached_companion_modules(frozen_repo, flaky):
    repo, runs = frozen_repo
    (repo / 'pyproject.toml').write_text(CONFIG + 'selection-groups=[["tests/test_contract.py", "tests/test_companion.py"]]\n')
    (repo / 'tests/conftest.py').write_text('''import pytest
SEEN = set()
@pytest.fixture(scope="session", autouse=True)
def all_calls():
    yield
    assert SEEN == {"first", "second"}, "session companions were split"
@pytest.fixture
def seen():
    return SEEN
''')
    (repo / MODULE).write_text('import os\nfrom product import value\n'
        'def test_first(seen):\n    seen.add("first")\n'
        '    if value() == 2 and os.environ.get("TL3_GROUP_ATTEMPT") == "1":\n        assert False\n')
    (repo / 'tests/test_companion.py').write_text('def test_second(seen):\n    seen.add("second")\n')
    # The plan does not use this shared conftest's session guard.
    (repo / 'other.py').write_text('def test_other():\n    assert True\n')
    (repo / 'pyproject.toml').write_text((repo / 'pyproject.toml').read_text().replace('"tests/*.py"', '"tests/*.py", "other.py"'))
    plan = (Check('other', Kind.REGRESSION, ('other.py::test_other',)),)
    base = save(repo, 'base')
    result = selected(repo, base, head(repo, 2 if flaky else 1), limit=2, plan=plan)[-1]
    assert result.verdict == Verdict.PASS, result
    group = (MODULE, 'tests/test_companion.py')
    assert tuple(test.test for test in result.tests) == group
    assert ('base', group) in runs and ('head', group) in runs
    assert len([tests for tree, tests in runs if tree == 'head' and MODULE in tests]) == (2 if flaky else 1)


def test_independent_module_flake_still_gets_a_bounded_retry(frozen_repo):
    repo, runs = frozen_repo
    (repo / MODULE).write_text('''import os
from product import value

def test_flaky():
    if value() == 2 and os.environ.get("TL3_GROUP_ATTEMPT") == "1":
        assert False, "independent flake"
''')
    base = save(repo, 'base')
    result = selected(repo, base, head(repo, 2), limit=1)[-1]
    assert result.verdict == Verdict.PASS
    assert len([tests for tree, tests in runs if tree == 'head' and tests != (OTHER,)]) == 2
    assert 'complete execution group' in result.tests[0].message


def test_cache_binds_container_and_never_reuses_an_unreported_group(frozen_repo, monkeypatch):
    repo, runs = frozen_repo
    base = save(repo, 'base')
    commit = head(repo)
    actual = execution.run_tree
    first = True

    def interrupted(tree, container, tests, *args, **kwargs):
        nonlocal first
        if tree.name == 'base' and any(test.startswith(MODULE) for test in tests) and first:
            first = False
            return dict.fromkeys(tests, Outcome.NO_REPORT)
        return actual(tree, container, tests, *args, **kwargs)

    monkeypatch.setattr(execution, 'run_tree', interrupted)
    assert len(selected(repo, base, commit)) == 1
    assert selected(repo, base, commit)[-1].verdict == Verdict.PASS
    runs.clear()
    assert selected(repo, base, commit)[-1].verdict == Verdict.PASS
    assert not any(tree == 'base' for tree, _ in runs)
    different = execution.Container('sha256:' + '2' * 64)
    assert selected(repo, base, commit, container=different)[-1].verdict == Verdict.PASS
    assert ('base', (MODULE,)) in runs


def test_a_cached_group_consumes_budget_before_a_new_group_is_admitted(frozen_repo):
    repo, _ = frozen_repo
    (repo / 'extra.py').write_text('VALUE = 1\n')
    (repo / 'tests/test_extra.py').write_text('import pytest\nimport extra\n'
        '@pytest.mark.parametrize("row", range(7))\ndef test_extra(row):\n    assert extra.VALUE\n')
    base = save(repo, 'base')
    selected(repo, base, head(repo))
    (repo / 'extra.py').write_text('VALUE = 2\n')
    commit = save(repo, 'select another group against the same base')
    result = selected(repo, base, commit)[-1]
    assert [test.test for test in result.tests] == [MODULE]
    receipt = json.loads(execution.result_json((result,)))[0]['selection']
    assert receipt['charged_cases'] == 394
    assert receipt['skipped_units'] == [['tests/test_extra.py']]
    assert receipt['saturation']['cases'] == 7 and receipt['saturation']['remaining'] == 6


@pytest.mark.parametrize('groups', [
    '[["tests/test_contract.py"], ["tests/test_contract.py"]]',
    '[["tests/test_contract.py::test_bindings_are_shared"]]',
    '[["tests/missing.py"]]',
    '[["tests/*.py"]]',
    '[[]]',
])
def test_group_declarations_reject_ambiguous_or_partial_modules(frozen_repo, groups):
    repo, _ = frozen_repo
    (repo / 'pyproject.toml').write_text(CONFIG + f'selection-groups={groups}\n')
    with pytest.raises(ValueError, match='selection group'):
        execution.configuration(repo)


def test_editing_a_passing_companion_of_a_failed_unit_cannot_hide_the_failure(frozen_repo):
    repo, _ = frozen_repo
    companion = 'tests/test_companion.py'
    (repo / 'pyproject.toml').write_text(CONFIG + f'selection-groups=[["{MODULE}", "{companion}"]]\n')
    (repo / MODULE).write_text('from product import value\ndef test_a():\n    assert value() == 1\n')
    (repo / companion).write_text('def test_b():\n    assert True\n')
    base = save(repo, 'complete unit passes on base')
    first = selected(repo, base, head(repo, 2))[-1]
    assert first.verdict == Verdict.FAIL
    outcomes = {test.test: test.head for test in first.tests}
    assert outcomes == {MODULE: Outcome.ASSERTION, companion: Outcome.PASSED}
    # This is the controller's persisted selected_failures input, before the next round.
    failures = {test['test'] for test in json.loads(execution.result_json((first,)))[0]['tests']
                if not test['passed']}
    (repo / companion).write_text('def test_b():\n    assert 2\n')
    commit = save(repo, 'edit only the passing companion without fixing A')
    second = selected(repo, base, commit)
    assert [result.name for result in second] == ['other'], 'the edited whole unit is excluded'
    assert failures == {MODULE, companion}, 'review must retain every member of the unresolved failed unit'
    assert 'companion' in first.tests[1].message.lower()


def test_twenty_passing_receipts_do_not_displace_failure_diagnostics(frozen_repo):
    repo, _ = frozen_repo
    for index in range(20):
        (repo / f'tests/test_{index:02}.py').write_text(
            'from product import value\ndef test_passes():\n    assert value() > 0\n')
    (repo / MODULE).write_text('from product import value\ndef test_fails():\n    assert value() == 1, "unresolved assertion"\n')
    base = save(repo, 'twenty passing modules precede a failing one')
    result = selected(repo, base, head(repo, 2), limit=21)[-1]
    assert [test.test for test in result.tests].index(MODULE) == 20
    serialized = json.loads(execution.result_json((result,)))[0]['tests']
    failure = next(test for test in serialized if not test['passed'])
    assert 'unresolved assertion' in failure['message']
    assert 'sha256 ' in failure['message'] and 'retry unit:' in failure['message']
    assert sum(bool(test['message']) for test in serialized) <= execution.MAX_DETAILED_FAILURES


@pytest.mark.parametrize('cached', ['none', 'prefix', 'suffix'])
def test_actual_cost_saturation_keeps_a_complete_priority_prefix(frozen_repo, cached):
    repo, runs = frozen_repo
    extra = 'tests/test_extra.py'
    tail = 'tests/test_tail.py'
    (repo / 'extra.py').write_text('VALUE = 1\n')
    (repo / extra).write_text('import pytest\nimport extra\n'
        '@pytest.mark.parametrize("row", range(7))\ndef test_extra(row):\n    assert extra.VALUE\n')
    (repo / tail).write_text('from product import value\ndef test_tail():\n    assert value() > 0\n')
    base = save(repo, '394-case guarded module, seven-case unit, one-case tail')
    if cached == 'prefix':
        selected(repo, base, head(repo))
    elif cached == 'suffix':
        (repo / 'extra.py').write_text('VALUE = 2\n')
        selected(repo, base, save(repo, 'cache the later seven-case unit only'))
    (repo / 'extra.py').write_text('VALUE = 3\n')
    commit = head(repo, 2)
    runs.clear()
    result = selected(repo, base, commit)[-1]
    assert result.verdict == Verdict.PASS, result
    assert [test.test for test in result.tests] == [MODULE]
    receipt = json.loads(execution.result_json((result,)))[0]['selection']
    assert receipt['limit'] == 400 and receipt['charged_cases'] == 394
    assert receipt['head_units'] == [[MODULE]]
    assert receipt['skipped_units'] == [[extra], [tail]]
    assert receipt['saturation']['unit'] == [extra]
    assert receipt['saturation']['cases'] == 7 and receipt['saturation']['remaining'] == 6
    assert receipt['skipped_checked'] is False
    assert all(tests == (MODULE,) for tree, tests in runs if tree == 'head' and tests != (OTHER,))
    assert 'retry unit:' in result.tests[0].message


def test_static_saturation_names_the_entire_unchecked_suffix(frozen_repo):
    repo, runs = frozen_repo
    (repo / MODULE).write_text('from product import value\ndef test_a():\n    assert value() > 0\n')
    names = [f'tests/test_{letter}.py' for letter in ('d', 'e', 'f')]
    for name in names:
        (repo / name).write_text('from product import value\ndef test_other():\n    assert value() > 0\n')
    base = save(repo, 'four ordered singleton units')
    result = selected(repo, base, head(repo), limit=2)[-1]
    receipt = json.loads(execution.result_json((result,)))[0]['selection']
    assert result.verdict == Verdict.PASS
    assert receipt['head_units'] == [[MODULE], [names[0]]]
    assert receipt['skipped_units'] == [[names[1]], [names[2]]]
    assert receipt['charged_cases'] == 2 and receipt['saturation']['remaining'] == 0
    assert receipt['skipped_checked'] is False
    assert all(names[1] not in tests and names[2] not in tests for tree, tests in runs if tree == 'head')


def test_saturation_is_reported_even_when_no_admitted_unit_passes_base(frozen_repo):
    repo, _ = frozen_repo
    (repo / MODULE).write_text('from product import value\ndef test_a():\n    assert value() != 1\n')
    (repo / 'tests/test_z.py').write_text('from product import value\ndef test_z():\n    assert value() > 0\n')
    base = save(repo, 'first complete unit fails on base')
    results = selected(repo, base, head(repo, 2), limit=1)
    assert [result.name for result in results] == ['other']
    receipt = json.loads(execution.result_json(results))[0]['selection']
    assert receipt['charged_cases'] == 1 and receipt['head_units'] == []
    assert receipt['skipped_units'] == [['tests/test_z.py']]
    assert receipt['skipped_checked'] is False


def test_unverified_base_collection_does_not_reuse_unknown_budget(frozen_repo, monkeypatch):
    repo, _ = frozen_repo
    (repo / MODULE).write_text('from product import value\n'
        'def test_a():\n    assert value() > 0\n'
        'def test_b():\n    assert value() > 0\n'
        'def test_c():\n    assert value() > 0\n')
    tail = 'tests/test_tail.py'
    (repo / tail).write_text('from product import value\ndef test_tail():\n    assert value() > 0\n')
    base = save(repo, 'one three-case unit followed by another unit')
    actual = execution.run_tree
    attempted = []

    def lost_collection(tree, container, tests, *args, **kwargs):
        if tree.name == 'base' and kwargs.get('selected') is not None:
            attempted.append(tests)
            return dict.fromkeys(tests, Outcome.NO_REPORT)
        return actual(tree, container, tests, *args, **kwargs)

    monkeypatch.setattr(execution, 'run_tree', lost_collection)
    results = selected(repo, base, head(repo), limit=3)
    assert attempted == [(MODULE,)], 'unknown consumed cases cannot fund another collection batch'
    receipt = json.loads(execution.result_json(results))[-1]['selection']
    assert receipt['skipped_units'] == [[MODULE], [tail]]
    assert 'unverified' in receipt['saturation']['reason'].lower()
    assert receipt['head_units'] == [] and receipt['skipped_checked'] is False


@pytest.mark.parametrize('fixed', [False, True])
def test_selection_displacement_rechecks_historical_failure_before_new_imports(frozen_repo, fixed):
    repo, runs = frozen_repo
    earlier = 'tests/test_aaa.py'
    (repo / 'extra.py').write_text('VALUE = 1\n')
    (repo / earlier).write_text('import pytest\nimport extra\n'
        '@pytest.mark.parametrize("row", range(150))\ndef test_extra(row):\n    assert extra.VALUE\n')
    base = save(repo, '394-case guarded unit and earlier-priority 150-case unit')
    first = selected(repo, base, head(repo, 0), limit=500)[-1]
    assert first.verdict == Verdict.FAIL
    failures = tuple(test['test'] for test in json.loads(execution.result_json((first,)))[0]['tests']
                     if not test['passed'])
    assert failures == (MODULE,)
    (repo / 'extra.py').write_text('VALUE = 2\n')
    if fixed:
        # The failed unit must still run even when it no longer imports the diff.
        (repo / 'product.py').write_text('def value():\n    return 1\n')
    commit = save(repo, 'another allowed source displaces the known failure without editing tests')
    runs.clear()
    result = selected(repo, base, commit, limit=500, failures=failures)[-1]
    assert [test.test for test in result.tests] == [MODULE], 'historical failure must precede new import candidates'
    assert result.verdict == (Verdict.PASS if fixed else Verdict.FAIL)
    receipt = json.loads(execution.result_json((result,)))[0]['selection']
    assert receipt['charged_cases'] == 394 and receipt['limit'] == 500
    assert receipt['head_units'] == [[MODULE]] and receipt['skipped_units'] == [[earlier]]
    assert receipt['skipped_checked'] is False
    assert all(tests == (MODULE,) for tree, tests in runs if tree == 'head' and tests != (OTHER,))


@pytest.mark.parametrize("cached", [False, True])
def test_selection_displacement_refuses_when_known_failed_units_alone_exceed_budget(frozen_repo, cached):
    repo, _ = frozen_repo
    earlier = 'tests/test_aaa.py'
    (repo / 'extra.py').write_text('VALUE = 1\n')
    (repo / earlier).write_text('import pytest\nimport extra\n'
        '@pytest.mark.parametrize("row", range(150))\ndef test_extra(row):\n    assert extra.VALUE\n')
    base = save(repo, 'historically failing units cost 544 in total')
    (repo / 'extra.py').write_text('VALUE = 2\n')
    if cached:
        selected(repo, base, save(repo, 'cache the later required unit'), limit=500)
    commit = head(repo, 0)
    with pytest.raises(HarnessInputError, match='Previously failing.*544.*500'):
        selected(repo, base, commit, limit=500, failures=(MODULE, earlier))
