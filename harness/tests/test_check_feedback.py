"""#426: a refused demonstrating or reproducing check says what the base run showed and which rule refused it."""
import json
from pathlib import Path
import subprocess
import sys

import pytest

from tl3_harness import execution
from tl3_harness.model import Check, Proof, Kind, Outcome, Verdict
from tl3_harness.report import decode, parse
from tl3_harness.trees import git

RUNNER = Path(__file__).parents[1] / 'src/tl3_harness/runner.py'
BASE_PRODUCT = 'PROFILES = {"json": 1}\n\n\ndef profile(name):\n    return PROFILES[name]\n'
HEAD_PRODUCT = 'PROFILES = {"json": 1, "default_repr": 2}\n\n\ndef profile(name):\n    return PROFILES[name]\n'
TESTS = '''\
import pytest
from product import profile


@pytest.fixture
def known():
    return profile("default_repr")


def test_key_error():
    assert profile("default_repr") == 2


def test_setup_error(known):
    assert known == 2


def test_base_pass():
    assert profile("json") == 1


def test_long_message():
    if "default_repr" not in __import__("product").PROFILES:
        raise ValueError("long " * 800)


def test_assertion():
    assert "default_repr" in __import__("product").PROFILES, "old assertion"
'''
UNRELATED = '''\
import product

if "default_repr" not in product.PROFILES:
    raise RuntimeError("profiles are not loaded")


def test_collected():
    assert product.profile("default_repr") == 2
'''


def run(tree, container, tests, report, progress, additions=frozenset(), *, messages=None):
    request = {'tests': tests, 'red_message': messages.red_message} if getattr(messages, 'red_message', None) is not None else tests
    child = subprocess.run([sys.executable, '-I', '-B', str(RUNNER), json.dumps(request), str(tree)],
                           capture_output=True)
    return parse([decode(line) for line in child.stdout.splitlines()], tests, set(),
                 child.returncode, additions, messages=messages)


def save(repo, message):
    git(repo, 'add', '--all')
    git(repo, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', message)
    return git(repo, 'rev-parse', 'HEAD').decode().strip()


@pytest.fixture
def round_repo(tmp_path, monkeypatch):
    repo = tmp_path / 'repo'
    (repo / 'tests').mkdir(parents=True)
    git(repo, 'init', '-q')
    (repo / 'pyproject.toml').write_text(
        '[tool.tl3.harness]\ncommand=["python", "-m", "pytest"]\ntest-files=["tests/*.py"]\n')
    (repo / 'product.py').write_text(BASE_PRODUCT)
    base = save(repo, 'base')
    (repo / 'product.py').write_text(HEAD_PRODUCT)
    (repo / 'tests/test_digest.py').write_text(TESTS)
    (repo / 'tests/test_unrelated.py').write_text(UNRELATED)
    head = save(repo, 'head')
    monkeypatch.setattr(execution, 'require_admission', lambda: None)
    monkeypatch.setattr(execution, 'run_tree', run)
    monkeypatch.setenv('PRISMABUILD_CONTAINER_OWNER', str(repo))
    return repo, base, head


def refused(round_repo, name, kind=Kind.DEMONSTRATING, file='test_digest'):
    repo, base, head = round_repo
    test = f'tests/{file}.py::{name}'
    result, = execution.run_checks(repo, base, head, (Check('check', kind, (test,)),),
                                   container=execution.Container('sha256:' + '1' * 64),
                                   reviewed=base if kind == Kind.REPRODUCING else None)
    assert result.verdict == Verdict.FAIL
    return result.tests[0]


@pytest.mark.parametrize('kind,verb', [(Kind.DEMONSTRATING, 'demonstrate'), (Kind.REPRODUCING, 'reproduce')])
def test_base_exception_in_the_call_names_the_error_and_the_rule(round_repo, kind, verb):
    test = refused(round_repo, 'test_key_error', kind)
    assert (test.base, test.head) == (Outcome.ERROR, Outcome.PASSED)
    assert "KeyError: 'default_repr'" in test.message
    assert 'during the call' in test.message
    assert f'does not {verb} the change' in test.message
    assert 'import the new name inside the test body' in test.message


def test_base_setup_error_says_it_happened_in_setup(round_repo):
    test = refused(round_repo, 'test_setup_error')
    assert test.base == Outcome.ERROR
    assert "KeyError: 'default_repr'" in test.message
    assert 'in setup' in test.message
    assert 'does not demonstrate the change' in test.message


def test_base_pass_says_the_test_must_fail_on_the_base(round_repo):
    test = refused(round_repo, 'test_base_pass')
    assert (test.base, test.head) == (Outcome.PASSED, Outcome.PASSED)
    assert 'passed on the base' in test.message
    assert 'does not demonstrate the change' in test.message


def test_unrelated_collection_failure_names_the_error(round_repo):
    test = refused(round_repo, 'test_collected', file='test_unrelated')
    assert test.base == Outcome.ERROR
    assert 'RuntimeError: profiles are not loaded' in test.message
    assert 'could not be collected' in test.message


def test_message_stays_bounded_and_keeps_the_exception_type(round_repo):
    test = refused(round_repo, 'test_long_message')
    assert 'ValueError: long long' in test.message
    assert len(test.message) <= 1000


def test_a_real_assertion_on_the_base_still_demonstrates(round_repo):
    repo, base, head = round_repo
    result, = execution.run_checks(
        repo, base, head,
        (Check('check', Kind.DEMONSTRATING, ('tests/test_digest.py::test_assertion',), proof=Proof((), (), 'old assertion')),),
        container=execution.Container('sha256:' + '1' * 64))
    assert result.verdict == Verdict.PASS
    assert result.tests[0].message == ''


def test_refusal_and_head_failure_are_truncated_with_a_marker():
    from tl3_harness.model import Failure, FailureNotes, judge
    test = 'tests/test_digest.py::test_x'
    notes = FailureNotes()
    notes.details[test] = Failure('call', 'KeyError: ' + "'k'" * 60, 'trace')
    result = judge(Check('check', Kind.DEMONSTRATING, (test,)), {test: Outcome.ERROR}, {test: Outcome.ASSERTION},
                   messages={test: 'head ' * 400}, base_notes=notes)
    message = result.tests[0].message
    assert result.verdict == Verdict.FAIL
    assert len(message) == 1000 and message.endswith(' [truncated]')
    assert message.startswith('On the base this test raised `KeyError')


def test_an_error_without_evidence_keeps_the_plain_line():
    from tl3_harness.model import judge
    test = 'tests/test_digest.py::test_x'
    result = judge(Check('check', Kind.DEMONSTRATING, (test,)), {test: Outcome.ERROR}, {test: Outcome.PASSED})
    assert result.tests[0].message == 'Check demonstrating: base error, head passed.'
