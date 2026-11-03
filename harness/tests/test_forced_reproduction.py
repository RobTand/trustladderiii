"""Forced reproduction (#393): deflake evidence from a forcing the harness applies."""
import json
from pathlib import Path
import subprocess
import sys

import pytest

from tl3_harness import execution
from tl3_harness.model import Check, Forcing, HarnessInputError, Kind, Outcome, Verdict
from tl3_harness.report import decode, parse
from tl3_harness.trees import git

RUNNER = Path(__file__).parents[1] / 'src/tl3_harness/runner.py'
CONTAINER = execution.Container('sha256:' + '1' * 64)
TEST = 'tests/test_sampler.py::test_departed_process_is_not_live'

# PB #2625's shape: the sampler rechecks a PID that left the sample. A real
# process holding that PID in another cgroup can't be decided, so it stays live.
SAMPLER = '''\
def process_in_scope(pid, cgroup):
    try:
        with open(f"/proc/{pid}/cgroup") as source:
            membership = source.read()
    except FileNotFoundError:
        return False
    if cgroup not in membership:
        raise ValueError(f"cannot decide whether {pid} is in {cgroup}")
    return True


def live_after(previous, current, cgroup):
    live = 0
    for pid in previous:
        if pid in current:
            live += 1
            continue
        try:
            live += process_in_scope(pid, cgroup)
        except ValueError:
            live += 1
    return live
'''
OLD_TEST = '''\
import sampler


def test_departed_process_is_not_live():
    assert sampler.live_after([4242], [], "/fake/cgroup") == 0
'''
FIXED_TEST = '''\
import os

import sampler


def test_departed_process_is_not_live(monkeypatch):
    # The departure is the test's premise; it must not depend on PID 4242 being free.
    monkeypatch.setattr(sampler, "process_in_scope", lambda pid, cgroup: False)
    assert sampler.live_after([os.getpid()], [], "/fake/cgroup") == 0
'''
PID_REUSE = {
    'symptom': 'A real, unrelated process holds PID 4242, so the sampler cannot decide whether it left.',
    'environment': {},
    'patches': [{'target': 'sampler.process_in_scope', 'raises': 'ValueError'}],
}


def run(tree, container, tests, report, progress, additions=frozenset(), *, messages=None, forcing=None):
    selection = tests if forcing is None else {'tests': tests, 'forcing': forcing.as_json()}
    child = subprocess.run([sys.executable, '-I', '-B', str(RUNNER), json.dumps(selection), str(tree)],
                           capture_output=True)
    missing = {test for test in tests if not (tree / test.split('::')[0]).is_file()}
    return parse([decode(line) for line in child.stdout.splitlines()], tests, missing,
                 child.returncode, additions, messages=messages)


def save(repo, message):
    git(repo, 'add', '--all')
    git(repo, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
        'commit', '-qm', message)
    return git(repo, 'rev-parse', 'HEAD').decode().strip()


@pytest.fixture
def pid_repo(tmp_path, monkeypatch):
    repo = tmp_path / 'repo'
    (repo / 'tests').mkdir(parents=True)
    git(repo, 'init', '-q')
    (repo / 'pyproject.toml').write_text(
        '[tool.tl3.harness]\ncommand=["python", "-m", "pytest"]\ntest-files=["tests/*.py"]\n')
    (repo / 'sampler.py').write_text(SAMPLER)
    (repo / 'tests/test_sampler.py').write_text(OLD_TEST)
    base = save(repo, 'base')
    monkeypatch.setattr(execution, 'require_admission', lambda: None)
    monkeypatch.setattr(execution, 'run_tree', run)
    monkeypatch.setenv('PRISMABUILD_CONTAINER_OWNER', str(repo))
    return repo, base


def forced(forcing=PID_REUSE, tests=(TEST,)):
    return Check('deflake', Kind.DEMONSTRATING, tests, forcing=Forcing.parse(forcing))


def head_with(repo, test_source):
    (repo / 'tests/test_sampler.py').write_text(test_source)
    return save(repo, 'head')


def test_test_only_deflake_cannot_demonstrate_without_a_forcing(pid_repo):
    # The base run applies the head's test edits, so a fix inside the test already passes there.
    repo, base = pid_repo
    head = head_with(repo, FIXED_TEST)
    result, = execution.run_checks(repo, base, head, (Check('deflake', Kind.DEMONSTRATING, (TEST,)),),
                                   container=CONTAINER)
    assert result.verdict == Verdict.FAIL
    assert result.tests[0].base == Outcome.PASSED


def test_forced_reproduction_of_pid_reuse_demonstrates_the_deflake(pid_repo):
    repo, base = pid_repo
    head = head_with(repo, FIXED_TEST)
    result, = execution.run_checks(repo, base, head, (forced(),), container=CONTAINER)
    assert result.verdict == Verdict.PASS, result.tests[0].message
    test, = result.tests
    assert (test.base, test.head_forced, test.head) == (Outcome.ASSERTION, Outcome.PASSED, Outcome.PASSED)
    assert result.forcing == Forcing.parse(PID_REUSE)


def test_fix_that_still_depends_on_the_free_pid_fails_under_the_forcing(pid_repo):
    repo, base = pid_repo
    head = head_with(repo, OLD_TEST.replace('4242', '4243'))
    result, = execution.run_checks(repo, base, head, (forced(),), container=CONTAINER)
    assert result.verdict == Verdict.FAIL
    test, = result.tests
    assert (test.base, test.head_forced, test.head) == (Outcome.ASSERTION, Outcome.ASSERTION, Outcome.PASSED)
    assert 'under the forcing' in test.message
    assert 'assert 1 == 0' in test.message


def test_fix_that_needs_the_forcing_fails_without_it(pid_repo):
    repo, base = pid_repo
    forcing = {'symptom': 'The scheduler runs the sampler late.', 'environment': {'SAMPLER_LATE': '1'}}
    (repo / 'tests/test_sampler.py').write_text(
        OLD_TEST + '\n\ndef test_late():\n    import os\n    assert os.environ.get("SAMPLER_LATE") != "1"\n')
    base = save(repo, 'base with the late test')
    late = 'tests/test_sampler.py::test_late'
    head = head_with(repo, OLD_TEST + '\n\ndef test_late():\n    import os\n'
                     '    assert os.environ.get("SAMPLER_LATE") == "1"\n')
    result, = execution.run_checks(repo, base, head, (forced(forcing, (late,)),), container=CONTAINER)
    assert result.verdict == Verdict.FAIL
    test, = result.tests
    # The environment reached the test process in both forced runs.
    assert (test.base, test.head_forced, test.head) == (Outcome.ASSERTION, Outcome.PASSED, Outcome.ASSERTION)
    assert 'without the forcing' in test.message


def test_forcing_that_does_not_reproduce_is_refused(pid_repo):
    repo, base = pid_repo
    head = head_with(repo, FIXED_TEST)
    harmless = {'symptom': 'The recheck runs slowly.',
                'patches': [{'target': 'sampler.process_in_scope', 'delay_seconds': 0.01}]}
    result, = execution.run_checks(repo, base, head, (forced(harmless),), container=CONTAINER)
    assert result.verdict == Verdict.FAIL
    test, = result.tests
    # The delay wraps the original, which finds no process 4242 and returns False.
    assert test.base == Outcome.PASSED
    assert 'passed on the base under the forcing' in test.message


def test_forcing_outside_the_repository_is_a_forcing_error(pid_repo):
    repo, base = pid_repo
    head = head_with(repo, FIXED_TEST)
    outside = {'symptom': 'Another process holds the PID.',
               'patches': [{'target': 'os.getpid', 'returns': 4242}]}
    result, = execution.run_checks(repo, base, head, (forced(outside),), container=CONTAINER)
    assert result.verdict == Verdict.FAIL
    test, = result.tests
    assert test.base == Outcome.ERROR
    assert "The plan's forcing could not be applied" in test.message
    assert 'os.getpid' in test.message


def test_forced_check_runs_the_old_test_as_the_base_wrote_it(pid_repo):
    repo, base = pid_repo
    new = 'tests/test_sampler.py::test_departure_is_stated'
    head = head_with(repo, FIXED_TEST + '\n\ndef test_departure_is_stated():\n    assert True\n')
    result, = execution.run_checks(repo, base, head, (forced(tests=(new,)),), container=CONTAINER)
    assert result.verdict == Verdict.FAIL
    test, = result.tests
    assert test.base != Outcome.PASSED
    assert 'defined at the base' in test.message


def test_result_json_carries_the_forcing_for_review(pid_repo):
    repo, base = pid_repo
    (repo / 'tests/test_other.py').write_text('def test_other():\n    assert True\n')
    base = save(repo, 'base with another test')
    head = head_with(repo, FIXED_TEST)
    results = execution.run_checks(repo, base, head,
                                   (forced(), Check('plain', Kind.REGRESSION, ('tests/test_other.py::test_other',))),
                                   container=CONTAINER)
    forced_report, plain_report = json.loads(execution.result_json(results))
    assert forced_report['forcing'] == Forcing.parse(PID_REUSE).as_json()
    assert forced_report['tests'][0]['head_forced'] == 'passed'
    # Unforced results keep today's shape.
    assert 'forcing' not in plain_report
    assert 'head_forced' not in plain_report['tests'][0]


@pytest.mark.parametrize('forcing, reason', [
    ({'symptom': 'x', 'patches': [{'target': 'sampler.f', 'raises': 'AssertionError'}]}, 'AssertionError'),
    ({'symptom': 'x', 'patches': [{'target': 'sampler.f', 'raises': 'SystemExit'}]}, 'SystemExit'),
    ({'symptom': 'x', 'patches': [{'target': 'sampler.f', 'raises': 'NotAName'}]}, 'NotAName'),
    ({'symptom': 'x', 'environment': {'PYTHONPATH': '/x'}}, 'PYTHONPATH'),
    ({'symptom': 'x', 'environment': {'HOME': '/x'}}, 'HOME'),
    ({'symptom': 'x', 'environment': {'lower': '1'}}, 'lower'),
    ({'symptom': 'x', 'environment': {}}, 'at least one'),
    ({'symptom': '', 'environment': {'A': '1'}}, 'symptom'),
    ({'symptom': 'x', 'patches': [{'target': 'sampler.f', 'value': 1, 'returns': 2}]}, 'exactly one'),
    ({'symptom': 'x', 'patches': [{'target': 'sampler', 'value': 1}]}, 'dotted'),
    ({'symptom': 'x', 'patches': [{'target': 'sampler.f', 'delay_seconds': 6}]}, 'delay_seconds'),
    ({'symptom': 'x', 'patches': [{'target': 'sampler.f', 'value': 'v' * 5000}]}, '4096'),
    ({'symptom': 'x', 'patches': [{'target': 'sampler.f', 'value': 1}], 'code': 'print(1)'}, 'code'),
])
def test_forcing_is_declarative_and_bounded(forcing, reason):
    with pytest.raises(HarnessInputError, match=reason):
        Forcing.parse(forcing)


@pytest.mark.parametrize('kind', [Kind.REGRESSION, Kind.BEHAVIOR_PRESERVING, Kind.REPRODUCING])
def test_only_demonstrating_checks_take_a_forcing(kind):
    with pytest.raises(HarnessInputError, match='demonstrating'):
        Check('deflake', kind, (TEST,), forcing=Forcing.parse(PID_REUSE))


def test_forcing_round_trips_through_its_json():
    forcing = Forcing.parse({**PID_REUSE, 'environment': {'B': '2', 'A': '1'},
                             'patches': PID_REUSE['patches'] + [{'target': 'sampler.LIMIT', 'value': {'n': [1]}}]})
    assert Forcing.parse(forcing.as_json()) == forcing
    assert list(forcing.as_json()['environment']) == ['A', 'B']


CLOCK = '''\
class Clock:
    def now(self):
        return 0


LIMIT = 3


def late():
    return Clock().now() > LIMIT
'''
ON_TIME = '''\
import clock


def test_on_time():
    assert not clock.late()
'''
ON_TIME_FIXED = '''\
import clock


def test_on_time(monkeypatch):
    # The premise is stated: the clock reads 0 against a limit of 3.
    monkeypatch.setattr(clock, "LIMIT", 3)
    monkeypatch.setattr(clock.Clock, "now", lambda self: 0)
    assert not clock.late()
'''


@pytest.mark.parametrize('patch', [
    {'target': 'clock.LIMIT', 'value': -1},
    {'target': 'clock.Clock.now', 'returns': 10},
])
def test_value_and_returns_forcings_including_a_class_owner(pid_repo, patch):
    repo, _ = pid_repo
    (repo / 'clock.py').write_text(CLOCK)
    (repo / 'tests/test_clock.py').write_text(ON_TIME)
    base = save(repo, 'base with a clock')
    (repo / 'tests/test_clock.py').write_text(ON_TIME_FIXED)
    head = save(repo, 'head')
    forcing = {'symptom': 'The sampler reads the clock late.', 'environment': {}, 'patches': [patch]}
    result, = execution.run_checks(repo, base, head, (forced(forcing, ('tests/test_clock.py::test_on_time',)),),
                                   container=CONTAINER)
    assert result.verdict == Verdict.PASS, result.tests[0].message
    assert (result.tests[0].base, result.tests[0].head_forced) == (Outcome.ASSERTION, Outcome.PASSED)


def test_forcing_reaches_the_runner_through_stdin(pid_repo, tmp_path, monkeypatch):
    # The real wire path: run_tree writes {tests, forcing} to the runner's stdin.
    repo, _ = pid_repo
    monkeypatch.undo()
    monkeypatch.setattr(execution, 'container_command',
                        lambda tree, container, tests: [sys.executable, '-I', '-B', str(RUNNER), '-', str(tree)])
    monkeypatch.setenv('PRISMABUILD_ACTION_PROGRESS_PATH', str(tmp_path / 'progress.json'))
    monkeypatch.setenv('PRISMABUILD_ACTION_PROGRESS_TOKEN', 'token')
    outcomes = execution.run_tree(repo, CONTAINER, (TEST,), tmp_path / 'report.jsonl', execution.Progress(),
                                  forcing=Forcing.parse(PID_REUSE))
    assert outcomes == {TEST: Outcome.ASSERTION}


def test_delays_are_capped_per_test(tmp_path):
    import importlib.util
    spec = importlib.util.spec_from_file_location('tl3_runner_under_test', RUNNER)
    runner = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(runner)

    class Owner:
        @staticmethod
        def call():
            return 'called'
    slept = []
    accounting = runner.DelayAccounting(tmp_path, ['Owner.call'])
    delayed = runner.forced_value(Owner, 'call', 'delay_seconds', 4, sleep=slept.append,
                                  accounting=accounting, target='Owner.call')
    assert [delayed() for _ in range(5)] == ['called'] * 5
    assert sum(slept) == runner.MAX_TOTAL_DELAY_SECONDS == 10
    assert slept[:3] == [4, 4, 2]
    assert accounting.finish() == {'Owner.call': 5}


@pytest.mark.parametrize('reference', [
    'tl3-forcing', '/tmp/*/node-*', 'tl3_harness.runner',
    'from tl3_harness import runner', 'tl3_harness.forcing_ownership',
])
def test_forced_head_diff_cannot_reference_forcing_internals(pid_repo, monkeypatch, reference):
    repo, base = pid_repo
    head = head_with(repo, FIXED_TEST + '\n# ' + reference + '\n')
    calls = []
    original = execution.run_tree

    def observed(*args, **kwargs):
        calls.append(args)
        return original(*args, **kwargs)

    monkeypatch.setattr(execution, 'run_tree', observed)
    result, = execution.run_checks(repo, base, head, (forced(),), container=CONTAINER)
    assert result.verdict == Verdict.FAIL
    assert 'forcing_tampered' in result.tests[0].message
    assert 'tests/test_sampler.py' in result.tests[0].message
    assert 'forcing internals' in result.tests[0].message
    assert calls == []


def test_unchanged_base_references_do_not_trip_the_forced_head_guard(pid_repo):
    repo, _ = pid_repo
    (repo / 'sampler.py').write_text(SAMPLER + '\n# tl3-forcing is unrelated base documentation.\n')
    base = save(repo, 'base documentation')
    head = head_with(repo, FIXED_TEST)
    result, = execution.run_checks(repo, base, head, (forced(),), container=CONTAINER)
    assert result.verdict == Verdict.PASS, result.tests[0].message
