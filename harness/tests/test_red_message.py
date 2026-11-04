"""#709: bind RED to the complete failure before bounding display text."""
from pathlib import Path
import sys

import pytest

from tl3_harness import execution
from tl3_harness.model import Check, FailureNotes, Kind, Outcome, Proof, Verdict, judge
from tl3_harness.report import InvalidReport, decode, parse
from tl3_harness.trees import git


NODE = 'tests/test_holds.py::test_hold'
ASSERTION = 'assert holds["transition_held_max_s"] >= SLOW_S'
RUNNER = Path(execution.__file__).with_name('runner.py')
IMAGE = execution.Container('sha256:' + '1' * 64)


@pytest.fixture
def checking(tmp_path, monkeypatch):
    repo = tmp_path / 'repo'
    (repo / 'tests').mkdir(parents=True)
    (repo / 'pyproject.toml').write_text(
        '[tool.tl3.harness]\ncommand=["python", "-m", "pytest"]\ntest-files=["tests/*.py"]\n')
    git(repo, 'init', '-q')
    monkeypatch.setattr(execution, 'require_admission', lambda: None)
    monkeypatch.setattr(execution.Progress, 'commit', lambda *_: None)
    monkeypatch.setenv('PRISMABUILD_CONTAINER_OWNER', str(repo))
    monkeypatch.setattr(execution, 'container_command', lambda tree, container, tests: [
        sys.executable, '-I', '-B', str(RUNNER), '-', str(tree)])
    reports = []
    actual = execution.run_tree

    def capture(tree, container, tests, report, *args, **kwargs):
        outcomes = actual(tree, container, tests, report, *args, **kwargs)
        reports.extend(decode(line) for line in report.read_bytes().splitlines())
        return outcomes

    monkeypatch.setattr(execution, 'run_tree', capture)

    def run(statement, declared, *, padding='', elsewhere=''):
        source = ('import product\nimport pytest\nSLOW_S = 1\n' + elsewhere
                  + 'def test_hold():\n'
                  + '    holds = {"transition_held_max_s": product.value()}\n'
                  + (f'    padding = {padding!r}\n' if padding else '')
                  + '    ' + statement + '\n')
        (repo / 'tests/test_holds.py').write_text(source)

        def save(value):
            (repo / 'product.py').write_text(f'def value(): return {value}\n')
            git(repo, 'add', '.')
            git(repo, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                'commit', '-qm', 'fixture')
            return git(repo, 'rev-parse', 'HEAD').decode().strip()

        base, head = save(0), save(1)
        check = Check('complete RED reason', Kind.DEMONSTRATING, (NODE,),
                      proof=Proof(('product.py::value',), (), declared))
        result, = execution.run_checks(repo, base, head, (check,), container=IMAGE)
        return result, next(record for record in reports
                            if record.get('event') == 'phase' and record.get('when') == 'call'
                            and record['outcome'] == 'failed')

    return run


def test_pb2722_source_assertion_matches_beyond_display_limit(checking):
    result, failure = checking(ASSERTION, ASSERTION, padding='x' * 2000)
    assert len(failure['message']) == 1000
    assert ASSERTION not in failure['message']
    assert result.tests[0].base == Outcome.ASSERTION
    assert result.verdict == Verdict.PASS, result
    assert failure['red_message_match'] == 'source-assertion'
    assert result.proof['red_message_matches'][NODE] == 'source-assertion'


def test_one_character_difference_beyond_display_limit_refuses(checking):
    result, _ = checking(ASSERTION, ASSERTION[:-1] + 'X', padding='x' * 2000)
    assert result.verdict == Verdict.FAIL
    assert 'declared assertion reason' in result.tests[0].message


def test_same_assertion_elsewhere_cannot_supply_the_failing_location(checking):
    result, _ = checking('assert product.value() == 1', ASSERTION,
                         elsewhere='def unused(holds):\n    ' + ASSERTION + '\n')
    assert result.verdict == Verdict.FAIL


@pytest.mark.parametrize('declared', [ASSERTION, 'assert  holds["transition_held_max_s"]   >=   SLOW_S'])
def test_short_bare_assertion_matches_exact_source_with_whitespace_normalization(checking, declared):
    result, _ = checking(ASSERTION, declared)
    assert result.verdict == Verdict.PASS, result


@pytest.mark.parametrize('declared,verdict', [
    ('hold too short', Verdict.PASS),
    (ASSERTION, Verdict.FAIL),
    ('hold too', Verdict.FAIL),
])
def test_explicit_assertion_message_keeps_whole_message_equality(checking, declared, verdict):
    result, _ = checking(ASSERTION + ', "hold too short"', declared)
    assert result.verdict == verdict, result


def test_source_assertion_inside_a_manual_error_cannot_supply_red(checking):
    result, _ = checking('if product.value() != 1: raise AssertionError(' + repr('other\n' + ASSERTION) + ')',
                         ASSERTION)
    assert result.verdict == Verdict.FAIL


def test_whitespace_inside_a_string_literal_is_not_normalized(checking):
    result, _ = checking('assert product.value() == 1 and "a b"',
                         'assert product.value() == 1 and "a  b"')
    assert result.verdict == Verdict.FAIL


def failure_report(match):
    phases = [{'event': 'phase', 'test': NODE, 'when': when, 'outcome': outcome,
               'assertion': when == 'call', 'missing': None}
              for when, outcome in [('setup', 'passed'), ('call', 'failed'), ('teardown', 'passed')]]
    phases[1].update(reason=ASSERTION, message='x' * 1000)
    if match is not None:
        phases[1]['red_message_match'] = match
    return [{'event': 'start', 'schema': 1}, {'event': 'collection', 'tests': [NODE]},
            *phases, {'event': 'finish', 'exit_code': 1}]


@pytest.mark.parametrize('match', [None, True, False, 'source', {'matched': True}])
def test_missing_or_untyped_capture_match_is_not_proof(match):
    with pytest.raises(InvalidReport, match='RED message match'):
        parse(failure_report(match), (NODE,), set(), 1, messages=FailureNotes(ASSERTION))


def test_bounded_reason_text_cannot_override_the_capture_verdict():
    notes = FailureNotes(ASSERTION)
    outcomes = parse(failure_report('mismatch'), (NODE,), set(), 1, messages=notes)
    result = judge(Check('capture verdict', Kind.DEMONSTRATING, (NODE,), proof=Proof((), (), ASSERTION)),
                   outcomes, {NODE: Outcome.PASSED}, base_notes=notes)
    assert result.verdict == Verdict.FAIL


def test_capture_match_cannot_be_reused_for_another_declaration():
    notes = FailureNotes(ASSERTION)
    outcomes = parse(failure_report('source-assertion'), (NODE,), set(), 1, messages=notes)
    result = judge(Check('another declaration', Kind.DEMONSTRATING, (NODE,),
                         proof=Proof((), (), ASSERTION[:-1] + 'X')),
                   outcomes, {NODE: Outcome.PASSED}, base_notes=notes)
    assert result.verdict == Verdict.FAIL
