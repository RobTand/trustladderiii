"""Existing failures excuse only the same failure on the head."""
import json
import os
from pathlib import Path
import subprocess
import sys

import pytest

from tl3_harness.execution import result_json
from tl3_harness.model import ALWAYS_CHECK, Check, FailureNotes, Kind, Outcome, Verdict, judge
from tl3_harness.report import decode, parse

RUNNER = Path(__file__).parents[1] / 'src/tl3_harness/runner.py'
NODE = 'test_existing.py::test_existing'


def source(phase, exception):
    failure = f'raise {exception}("existing failure")' if exception else 'pass'
    if phase == 'collection':
        return f'{failure}\ndef test_existing(): pass\n'
    return ('import pytest\n@pytest.fixture(autouse=True)\ndef context():\n'
            f'    {failure if phase == "setup" else "pass"}\n    yield\n'
            f'    {failure if phase == "teardown" else "pass"}\n'
            f'def test_existing():\n    {failure if phase == "call" else "pass"}\n')


def observe(tree, code, selector=NODE):
    tree.mkdir()
    (tree / 'test_existing.py').write_text(code)
    notes = FailureNotes()
    child = subprocess.run([sys.executable, '-I', '-B', str(RUNNER), json.dumps([selector]), str(tree)],
                           capture_output=True, env={'PATH': os.environ['PATH'],
                                                    'PYTEST_DISABLE_PLUGIN_AUTOLOAD': '1'})
    outcomes = parse([decode(line) for line in child.stdout.splitlines()], (selector,), set(),
                     child.returncode, messages=notes)
    return outcomes, notes


@pytest.mark.parametrize('base_phase,base_type,head_phase,head_type,expected', [
    ('call', 'AssertionError', 'call', 'AssertionError', Verdict.PASS),
    ('call', 'AssertionError', 'call', 'CustomAssertion', Verdict.FAIL),
    ('call', 'AssertionError', 'call', 'ValueError', Verdict.FAIL),
    ('call', 'ValueError', 'call', 'AssertionError', Verdict.FAIL),
    ('setup', 'AssertionError', 'setup', 'ValueError', Verdict.FAIL),
    ('call', 'ValueError', 'call', 'ValueError', Verdict.PASS),
    ('setup', 'ValueError', 'setup', 'ValueError', Verdict.PASS),
    ('teardown', 'ValueError', 'teardown', 'ValueError', Verdict.PASS),
    ('call', 'ValueError', 'setup', 'ValueError', Verdict.FAIL),
    ('setup', 'ValueError', 'teardown', 'ValueError', Verdict.FAIL),
    ('teardown', 'ValueError', 'call', 'ValueError', Verdict.FAIL),
    ('collection', 'ValueError', 'collection', 'ValueError', Verdict.PASS),
    ('collection', 'ValueError', 'collection', 'TypeError', Verdict.FAIL),
    ('call', 'ValueError', 'call', None, Verdict.PASS),
    ('call', None, 'call', None, Verdict.PASS),
])
def test_existing_failure_identity(tmp_path, base_phase, base_type, head_phase, head_type, expected):
    before, base_notes = observe(tmp_path / 'base', source(base_phase, base_type))
    after, head_notes = observe(tmp_path / 'head',
        'class CustomAssertion(AssertionError): pass\n' + source(head_phase, head_type))
    result = judge(Check('existing', Kind.REGRESSION, (NODE,)), before, after,
                   base_notes=base_notes, messages=head_notes)
    assert result.verdict == expected, result
    if expected == Verdict.FAIL:
        assert 'base' in result.tests[0].message.lower()
        assert 'head' in result.tests[0].message.lower()
        assert base_type in result.tests[0].message
        assert head_type in result.tests[0].message


@pytest.mark.parametrize('head_outcome', [Outcome.ERROR, Outcome.NO_REPORT])
def test_existing_failure_needs_head_detail(head_outcome):
    result = judge(Check('missing detail', Kind.REGRESSION, (NODE,)),
                   {NODE: Outcome.ERROR}, {NODE: head_outcome})
    assert not result.tests[0].passed
    assert result.verdict == (Verdict.INFRASTRUCTURE_FAILURE if head_outcome == Outcome.NO_REPORT
                              else Verdict.FAIL)
    assert 'base' in result.tests[0].message.lower()
    assert 'head' in result.tests[0].message.lower()


def test_failed_on_base_not_checked_count(tmp_path):
    before, base_notes = observe(tmp_path / 'base', source('setup', 'ValueError'))
    after, head_notes = observe(tmp_path / 'head', source('setup', 'ValueError'))
    result = judge(Check('existing', Kind.REGRESSION, (NODE,)), before, after,
                   base_notes=base_notes, messages=head_notes)
    assert result.verdict == Verdict.PASS
    report, = json.loads(result_json((result,)))
    assert report.get('base_failures_not_checked') == 1, report
    assert 'failed on base, not checked' in report['tests'][0]['message']


@pytest.mark.parametrize('selector', [NODE + '[a]', 'test_existing.py'])
def test_parametrized_failures_compare_case_ids(tmp_path, selector):
    code = ('import pytest\n@pytest.mark.parametrize("case", ["a", "b"])\n'
            'def test_existing(case):\n    if case == "a": raise ValueError("old")\n'
            '    if case == "b": {second}\n')
    before, base_notes = observe(tmp_path / 'base', code.format(second='raise TypeError("old")'), selector)
    after, head_notes = observe(tmp_path / 'head', code.format(second='raise RuntimeError("new")'), selector)
    result = judge(Check(ALWAYS_CHECK, Kind.REGRESSION, (selector,)), before, after,
                   base_notes=base_notes, messages=head_notes)
    assert result.verdict == (Verdict.PASS if '[' in selector else Verdict.FAIL), result


def test_same_failure_does_not_require_the_same_message(tmp_path):
    before, base_notes = observe(tmp_path / 'base', source('call', 'ValueError'))
    after, head_notes = observe(tmp_path / 'head',
        source('call', 'ValueError').replace('existing failure', 'a different message'))
    result = judge(Check('messages', Kind.REGRESSION, (NODE,)), before, after,
                   base_notes=base_notes, messages=head_notes)
    assert result.verdict == Verdict.PASS


@pytest.mark.parametrize('base_type', [None, 'ValueError'])
def test_passing_head_does_not_count_as_unchecked(tmp_path, base_type):
    before, base_notes = observe(tmp_path / 'base', source('call', base_type))
    after, head_notes = observe(tmp_path / 'head', source('call', None))
    result = judge(Check('passing head', Kind.REGRESSION, (NODE,)), before, after,
                   base_notes=base_notes, messages=head_notes)
    report, = json.loads(result_json((result,)))
    assert result.verdict == Verdict.PASS
    assert report.get('base_failures_not_checked', 0) == 0


@pytest.mark.parametrize('base_phase,head_phase', [
    (before, after) for before in ('collection', 'setup', 'call', 'teardown')
    for after in ('collection', 'setup', 'call', 'teardown') if before != after
])
def test_same_type_in_every_different_phase_fails(tmp_path, base_phase, head_phase):
    before, base_notes = observe(tmp_path / 'base', source(base_phase, 'ValueError'))
    after, head_notes = observe(tmp_path / 'head', source(head_phase, 'ValueError'))
    result = judge(Check('phase transition', Kind.REGRESSION, (NODE,)), before, after,
                   base_notes=base_notes, messages=head_notes)
    assert result.verdict == Verdict.FAIL
    assert base_phase in result.tests[0].message
    assert head_phase in result.tests[0].message


def test_long_exception_types_cannot_match_by_a_clipped_prefix(tmp_path):
    observations = []
    for side in ('base', 'head'):
        name = 'Deadline' + 'X' * 210 + side
        code = f'class {name}(ValueError): pass\ndef test_existing(): raise {name}("failure")\n'
        observations.append(observe(tmp_path / side, code))
    (before, base_notes), (after, head_notes) = observations
    result = judge(Check('long types', Kind.REGRESSION, (NODE,)), before, after,
                   base_notes=base_notes, messages=head_notes)
    assert result.verdict == Verdict.FAIL
    assert not result.tests[0].passed
