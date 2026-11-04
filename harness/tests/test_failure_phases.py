"""#627: keep phase evidence and notify fixtures about runner deselection."""
import copy
import json
from pathlib import Path
import subprocess
import sys

import pytest

from tl3_harness import execution
from tl3_harness.model import Check, Proof, FailureNotes, Kind, Outcome, Verdict, judge
from tl3_harness.report import InvalidReport, decode, parse
from tl3_harness.trees import git

RUNNER = Path(execution.__file__).with_name('runner.py')
NODE = 'test_phases.py::test_probe'
IMAGE = execution.Container('sha256:' + '1' * 64)
SOURCE = '''import product
import pytest
@pytest.fixture
def resource():
    if product.SETUP:
        raise LookupError('setup blocked')
    yield
    if product.TEARDOWN:
        raise RuntimeError('teardown blocked')
def test_probe(resource):
    assert not product.CALL, "call invariant"
'''


def observe(tree, tests):
    request = {'tests': tests, 'red_message': 'call invariant'}
    child = subprocess.run([sys.executable, '-I', '-B', str(RUNNER), json.dumps(request), str(tree)],
                           capture_output=True, check=False)
    records = [decode(line) for line in child.stdout.splitlines()]
    notes = FailureNotes('call invariant')
    outcomes = parse(records, tests, set(), child.returncode, messages=notes)
    return outcomes, notes, records, child.returncode


def save(repo):
    git(repo, 'add', '.')
    git(repo, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'fixture')
    return git(repo, 'rev-parse', 'HEAD').decode().strip()


@pytest.fixture
def phase_repo(tmp_path):
    (tmp_path / 'test_phases.py').write_text(SOURCE)
    return tmp_path


def configure(repo, setup=False, call=False, teardown=False):
    (repo / 'product.py').write_text(f'SETUP = {setup}\nCALL = {call}\nTEARDOWN = {teardown}\n')


def result_case(before, notes):
    result = judge(Check('phase evidence', Kind.DEMONSTRATING, (NODE,), proof=Proof((), (), 'call invariant')), before,
                   {NODE: Outcome.PASSED}, base_notes=notes)
    return result, json.loads(execution.result_json((result,)))[0]['tests'][0]


def test_call_assertion_and_teardown_error_keep_both_facts_and_refusal(phase_repo):
    configure(phase_repo, call=True, teardown=True)
    before, notes, _, _ = observe(phase_repo, (NODE,))
    result, case = result_case(before, notes)
    assert result.verdict == Verdict.FAIL and before[NODE] == Outcome.ERROR
    assert 'phases' in case, 'the trusted result lost setup/call/teardown evidence'
    phases = case['phases']['base']
    assert phases['test'] == NODE
    assert phases['setup'] == dict(outcome='passed', exception='', assertion=False)
    assert phases['call'] == dict(outcome='failed', exception='builtins.AssertionError', assertion=True)
    assert phases['teardown'] == dict(outcome='failed', exception='builtins.RuntimeError', assertion=False)
    assert 'assertion' in case['message'].lower() and 'during the call' in case['message']
    assert 'RuntimeError' in case['message'] and 'teardown' in case['message']
    assert 'neither an assertion' not in case['message']
    assert len(case['message']) <= 1000


def test_refusal_does_not_misclassify_a_call_assertion(phase_repo):
    configure(phase_repo, call=True, teardown=True)
    before, notes, _, _ = observe(phase_repo, (NODE,))
    result, case = result_case(before, notes)
    assert result.verdict == Verdict.FAIL
    assert 'neither an assertion' not in case['message'], case['message']
    assert 'AssertionError' in case['message'] and 'RuntimeError' in case['message']
    assert 'during the call' in case['message'] and 'teardown' in case['message']


@pytest.mark.parametrize('flags,outcome,failed_phase,exception', [
    ({'call': True}, Outcome.ASSERTION, 'call', 'builtins.AssertionError'),
    ({'setup': True}, Outcome.ERROR, 'setup', 'builtins.LookupError'),
    ({'teardown': True}, Outcome.ERROR, 'teardown', 'builtins.RuntimeError'),
])
def test_single_phase_controls_preserve_red_rules(phase_repo, flags, outcome, failed_phase, exception):
    configure(phase_repo, **flags)
    before, notes, _, _ = observe(phase_repo, (NODE,))
    result, case = result_case(before, notes)
    assert before[NODE] == outcome
    assert (result.verdict == Verdict.PASS) == (outcome == Outcome.ASSERTION)
    assert 'phases' in case, 'single-phase evidence must survive judging too'
    phases = case['phases']['base']
    assert phases[failed_phase]['exception'] == exception
    assert set(phases) == ({'test', 'setup', 'teardown'} if failed_phase == 'setup'
                           else {'test', 'setup', 'call', 'teardown'})
    if outcome == Outcome.ERROR:
        assert failed_phase in case['message'] and exception.rsplit('.', 1)[1] in case['message']


@pytest.mark.parametrize('damage', ['duplicate', 'order', 'unknown', 'exception_bound', 'exception_type'])
def test_phase_reports_still_decode_strictly(phase_repo, damage):
    configure(phase_repo, call=True, teardown=True)
    _, _, records, exit_code = observe(phase_repo, (NODE,))
    records = copy.deepcopy(records)
    phase = next(record for record in records if record.get('when') == 'call')
    if damage == 'duplicate':
        records.insert(-1, dict(phase))
    elif damage == 'order':
        phase['when'] = 'setup'
    elif damage == 'unknown':
        phase['locals'] = {'request': 'must not enter evidence'}
    elif damage == 'exception_bound':
        phase['exception_type'] = 'x' * 201
    else:
        phase['exception_type'] = []
    with pytest.raises(InvalidReport):
        parse(records, (NODE,), set(), exit_code, messages=FailureNotes('call invariant'))


def test_phase_evidence_survives_real_checking_staging(phase_repo, monkeypatch):
    repo = phase_repo
    git(repo, 'init', '-q')
    (repo / 'pyproject.toml').write_text(
        '[tool.tl3.harness]\ncommand=["python", "-m", "pytest"]\ntest-files=["test_*.py"]\n')
    configure(repo, call=True, teardown=True)
    base = save(repo)
    configure(repo)
    head = save(repo)
    monkeypatch.setattr(execution, 'require_admission', lambda: None)
    monkeypatch.setattr(execution, 'container_command', lambda tree, container, tests: [
        sys.executable, '-I', '-B', str(RUNNER), '-', str(tree)])
    monkeypatch.setenv('PRISMABUILD_CONTAINER_OWNER', str(repo))
    monkeypatch.setenv('PRISMABUILD_ACTION_PROGRESS_PATH', str(repo / 'progress.json'))
    monkeypatch.setenv('PRISMABUILD_ACTION_PROGRESS_TOKEN', 'fixture')
    result, = execution.run_checks(repo, base, head, (Check('phases', Kind.DEMONSTRATING, (NODE,)),),
                                   container=IMAGE)
    assert result.verdict == Verdict.FAIL
    case = json.loads(execution.result_json((result,)))[0]['tests'][0]
    assert 'phases' in case, 'Checking must serialize phase evidence from both immutable trees'
    assert case['phases']['base']['call']['assertion'] is True
    assert case['phases']['base']['teardown']['exception'] == 'builtins.RuntimeError'
    assert all(case['phases']['head'][phase]['outcome'] == 'passed'
               for phase in ('setup', 'call', 'teardown'))
    assert case['base'] == 'error' and case['head'] == 'passed'


GUARD = '''import json
from pathlib import Path
import pytest
omitted = []
used = set()
def pytest_deselected(items):
    omitted.extend(item.nodeid for item in items)
    Path('omitted.json').write_text(json.dumps(omitted))
@pytest.fixture(scope='module', autouse=True)
def all_rows():
    yield
    if not any(node.split('::')[0] == 'test_rows.py' for node in omitted):
        assert used == {'first', 'second', 'last'}, 'whole module left golden rows unused'
@pytest.fixture
def row():
    return used.add
'''
ROWS = '''import pytest
@pytest.mark.parametrize('value', ['first', 'second'])
def test_rows(row, value):
    row(value)
def test_last(row):
    row('last')
'''
ALL_NODES = ['test_rows.py::test_rows[first]', 'test_rows.py::test_rows[second]', 'test_rows.py::test_last']


@pytest.mark.parametrize('indices', [(2,), (1, 2), (2, 0)])
def test_runner_deselection_reaches_module_fixture_before_last_selected_teardown(tmp_path, indices):
    (tmp_path / 'conftest.py').write_text(GUARD)
    (tmp_path / 'test_rows.py').write_text(ROWS)
    tests = tuple(ALL_NODES[index] for index in indices)
    outcomes, _, records, _ = observe(tmp_path, tests)
    assert outcomes == dict.fromkeys(tests, Outcome.PASSED), 'last selected node inherited the whole-module guard error'
    omitted = json.loads((tmp_path / 'omitted.json').read_text())
    assert omitted == [node for node in ALL_NODES if node not in tests]
    collection = next(record['tests'] for record in records if record['event'] == 'collection')
    assert collection == [node for node in ALL_NODES if node in tests]
    assert all(record['outcome'] == 'passed' for record in records if record.get('when') == 'teardown')


@pytest.mark.parametrize('unused', [False, True])
def test_full_module_guard_still_detects_unused_golden_rows(tmp_path, unused):
    (tmp_path / 'conftest.py').write_text(GUARD.replace("'second', 'last'", "'second', 'last', 'unused'") if unused else GUARD)
    (tmp_path / 'test_rows.py').write_text(ROWS)
    outcomes, _, records, _ = observe(tmp_path, ('test_rows.py',))
    assert outcomes['test_rows.py'] == (Outcome.ERROR if unused else Outcome.PASSED)
    assert not (tmp_path / 'omitted.json').exists(), 'full modules must not be marked partially selected'
    teardown = [record for record in records if record.get('when') == 'teardown']
    assert teardown[-1]['test'] == ALL_NODES[-1]
    assert teardown[-1]['outcome'] == ('failed' if unused else 'passed')
