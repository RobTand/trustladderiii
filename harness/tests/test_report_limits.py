import json
from pathlib import Path
import sys

import pytest

from tl3_harness import execution
from tl3_harness.model import Check, Kind, Outcome, Verdict, judge

RUNNER = Path(__file__).parents[1] / 'src/tl3_harness/runner.py'
IMAGE = execution.Container('sha256:' + '1' * 64)


class Progress:
    def commit(self, test):
        pass


def test_collection_above_64_kib_has_a_normal_report(tmp_path, monkeypatch):
    (tmp_path / 'test_many.py').write_text(
        'import pytest\n@pytest.mark.parametrize("value", range(1000), '
        'ids=lambda value: "case_" + str(value) + "x" * 80)\n'
        'def test_many(value): assert value >= 0\n')
    monkeypatch.setattr(execution, 'container_command', lambda tree, container, tests: [
        sys.executable, '-I', '-B', str(RUNNER), json.dumps(tests), str(tree)])
    report = tmp_path / 'report.jsonl'
    outcomes = execution.run_tree(tmp_path, IMAGE, ('test_many.py::test_many',), report, Progress())
    assert outcomes == {'test_many.py::test_many': Outcome.PASSED}
    collection = next(line for line in report.read_bytes().splitlines()
                      if json.loads(line)['event'] == 'collection')
    assert len(collection) > 65_536


def test_large_selection_does_not_become_one_large_exec_argument(tmp_path):
    tests = tuple(f'test_many.py::test_{i}_' + 'x' * 90 for i in range(2000))
    command = execution.container_command(tmp_path, IMAGE, tests)
    assert max(len(arg.encode()) for arg in command) < 131_072


def test_plan_above_report_budget_is_rejected():
    from tl3_harness.limits import validate_plan_size
    with pytest.raises(ValueError, match='4194304.*reduce'):
        validate_plan_size(('test_many.py::test_' + 'x' * (4 * 1024 * 1024),))


def test_report_record_count_stays_bounded(tmp_path, monkeypatch):
    script = tmp_path / 'flood.py'
    script.write_text('print(\'{"event":"start","schema":1}\')\n'
                      'for _ in range(100): print(\'{"event":"collection","tests":[]}\')\n')
    monkeypatch.setattr(execution, 'container_command', lambda *args: [sys.executable, str(script)])
    report = tmp_path / 'report.jsonl'
    tests = ('test_many.py::test_many',)
    outcomes = execution.run_tree(tmp_path, IMAGE, tests, report, Progress())
    assert outcomes == dict.fromkeys(tests, Outcome.NO_REPORT)
    assert len(report.read_bytes().splitlines()) <= 4 * len(tests) + 17


def test_expansion_above_budget_is_a_plan_error(tmp_path, monkeypatch):
    (tmp_path / 'test_many.py').write_text(
        'import pytest\n@pytest.mark.parametrize("value", [1], '
        'ids=["x" * (4 * 1024 * 1024)])\n'
        'def test_many(value): assert value == 1\n')
    monkeypatch.setattr(execution, 'container_command', lambda tree, container, tests: [
        sys.executable, '-I', '-B', str(RUNNER), json.dumps(tests), str(tree)])
    messages = {}
    outcomes = execution.run_tree(tmp_path, IMAGE, ('test_many.py::test_many',),
                                  tmp_path / 'report.jsonl', Progress(), messages=messages)
    assert outcomes == {'test_many.py::test_many': Outcome.UNCOLLECTED}
    check = Check('large expansion', Kind.REGRESSION, ('test_many.py::test_many',))
    assert judge(check, outcomes, outcomes).verdict == Verdict.FAIL
    assert '4194304' in messages['test_many.py::test_many']
    assert 'reduce' in messages['test_many.py::test_many']


def test_report_line_above_four_mib_stays_bounded(tmp_path, monkeypatch):
    script = tmp_path / 'flood.py'
    script.write_text('print(\'{"event":"start","schema":1}\')\n'
                      'print("x" * (4 * 1024 * 1024 + 1))\n')
    monkeypatch.setattr(execution, 'container_command', lambda *args: [sys.executable, str(script)])
    report = tmp_path / 'report.jsonl'
    tests = ('test_many.py::test_many',)
    outcomes = execution.run_tree(tmp_path, IMAGE, tests, report, Progress())
    assert outcomes == dict.fromkeys(tests, Outcome.NO_REPORT)
    assert report.stat().st_size < 4 * 1024 * 1024


def test_plan_budget_counts_escaped_bytes_and_the_unique_union():
    from tl3_harness.limits import validate_plan_size
    first = 'test_many.py::test_' + 'é' * (1024 * 1024)
    validate_plan_size((first, first, first))
    with pytest.raises(ValueError, match='4194304.*reduce'):
        validate_plan_size((first, first + 'second'))
    with pytest.raises(ValueError, match='4194304.*reduce'):
        validate_plan_size(('test_many.py::test_' + '"' * (2 * 1024 * 1024),))
