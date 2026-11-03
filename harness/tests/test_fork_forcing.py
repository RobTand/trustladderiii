"""Real fork descendants must retain forcing evidence even after SIGKILL (#602)."""
import json
from pathlib import Path
import subprocess
import sys

import pytest

from tl3_harness import execution, runner, test_tool
from tl3_harness.model import Forcing


RUNNER = Path(execution.__file__).with_name('runner.py')
CONTAINER = execution.Container('sha256:' + '1' * 64)
TARGET = 'product.target'
FORCING = Forcing.parse({'symptom': 'The child misses its deadline.',
                         'patches': [{'target': TARGET, 'delay_seconds': 4},
                                     {'target': 'product.other', 'delay_seconds': 4}]})
SOURCE = '''
import os
from pathlib import Path
import signal
import sys
import time

import product

def kill_during_delay():
    reader, writer = os.pipe()
    child = os.fork()
    if child == 0:
        os.close(reader)
        def entering_sleep(frame, event, function):
            if event == 'c_call' and function is time.sleep:
                os.write(writer, b'entered')
                sys.setprofile(None)
        sys.setprofile(entering_sleep)
        product.target(7, extra=2)
        os._exit(0)
    os.close(writer)
    try:
        assert os.read(reader, 7) == b'entered'
    finally:
        os.close(reader)
        os.kill(child, signal.SIGKILL)
        os.waitpid(child, 0)

def test_killed():
    kill_during_delay()
    assert False, 'outer assertion after child target entry'

def test_completed():
    child = os.fork()
    if child == 0:
        # PB's bounded reader closes inherited descriptors before its callback.
        os.closerange(3, 256)
        result = product.target(7, extra=2)
        Path('witness').write_text(str(result))
        os._exit(0)
    _, status = os.waitpid(child, 0)
    assert status == 0
    assert Path('witness').read_text() == '9'

def test_zero():
    pass

def test_spawn():
    import subprocess
    result = subprocess.run([sys.executable, '-I', '-c',
        "import sys; sys.path.insert(0, '.'); import product; assert product.target(7, extra=2) == 9"])
    assert result.returncode == 0

def consume_budget(expected=10):
    Path('waits').write_text('')
    reader, writer = os.pipe()
    children = []
    for index in range(6):
        child = os.fork()
        if child == 0:
            os.close(writer)
            os.read(reader, 1)
            class SkipSleep(Exception):
                pass
            def record_sleep(frame, event, function):
                if event == 'c_call' and function is time.sleep:
                    with open('waits', 'a') as log:
                        log.write(str(frame.f_locals['wait']) + '\\n')
                    raise SkipSleep
            sys.setprofile(record_sleep)
            try:
                (product.target if index % 2 == 0 else product.other)(7, extra=2)
            except SkipSleep:
                pass
            os._exit(0)
        children.append(child)
    os.close(reader)
    os.write(writer, b'x' * len(children))
    os.close(writer)
    for child in children:
        assert os.waitpid(child, 0)[1] == 0
    assert sum(map(float, Path('waits').read_text().splitlines())) == expected

def test_budget():
    consume_budget()

def test_killed_budget():
    kill_during_delay()
    consume_budget(expected=6)
'''


@pytest.fixture
def repo(tmp_path, monkeypatch):
    (tmp_path / 'product.py').write_text(
        'def target(value, *, extra):\n    return value + extra\n\nother = target\n')
    (tmp_path / 'test_product.py').write_text(SOURCE)
    monkeypatch.setattr(test_tool, 'require_admission', lambda: None)
    monkeypatch.setattr(test_tool, 'container_command', lambda tree, container, tests: [
        sys.executable, '-I', '-B', str(RUNNER), '-', str(tree)])
    monkeypatch.setenv('PRISMABUILD_ACTION_PROGRESS_PATH', str(tmp_path / 'progress.json'))
    monkeypatch.setenv('PRISMABUILD_ACTION_PROGRESS_TOKEN', 'fixture')
    return tmp_path


def summary(repo, names, forcing=FORCING):
    return test_tool.run(repo, CONTAINER, tuple('test_product.py::test_' + name for name in names), forcing=forcing)


def test_killed_child_counts_at_entry_and_still_requires_an_outer_assertion(repo):
    result = summary(repo, ['killed', 'zero'])
    assert result['status'] == 'completed', result
    killed, zero = result['tests']
    assert killed['outcome'] == 'failed'
    assert 'outer assertion after child target entry' in killed['message']
    assert killed['target_hits'] == {TARGET: 1, 'product.other': 0}
    assert zero['target_hits'] == {TARGET: 0, 'product.other': 0}


def test_child_closing_inherited_descriptors_keeps_hits_and_original_result(repo):
    forcing = Forcing.parse({'symptom': 'A slow callback.',
                             'patches': [{'target': TARGET, 'delay_seconds': 0.001}]})
    result = summary(repo, ['completed', 'zero'], forcing)
    assert result['status'] == 'completed', result
    assert result['totals']['passed'] == 2
    assert [test['target_hits'][TARGET] for test in result['tests']] == [1, 0]


def test_concurrent_children_share_budget_across_targets_and_keep_exact_hits(repo):
    result = summary(repo, ['budget', 'zero'])
    assert result['status'] == 'completed', result
    budget, zero = result['tests']
    assert budget['outcome'] == 'passed', budget
    assert budget['target_hits'] == {TARGET: 3, 'product.other': 3}
    assert zero['target_hits'] == {TARGET: 0, 'product.other': 0}


def test_spawned_interpreter_without_forcing_supplies_no_hit(repo):
    result = summary(repo, ['spawn'])
    assert result['status'] == 'completed', result
    assert result['totals']['passed'] == 1
    assert result['tests'][0]['target_hits'] == {TARGET: 0, 'product.other': 0}


def test_killed_child_reservation_stays_consumed_and_next_node_gets_fresh_budget(repo):
    result = summary(repo, ['killed_budget', 'budget'])
    assert result['status'] == 'completed', result
    assert result['totals']['passed'] == 2, result
    assert result['tests'][0]['target_hits'] == {TARGET: 4, 'product.other': 3}
    assert result['tests'][1]['target_hits'] == {TARGET: 3, 'product.other': 3}


def test_retired_journal_cannot_feed_the_next_node(tmp_path):
    old = runner.DelayAccounting(tmp_path, [TARGET])
    assert old.enter(TARGET, 4) == 4
    assert old.finish() == {TARGET: 1}
    current = runner.DelayAccounting(tmp_path, [TARGET])
    assert current.path != old.path
    with pytest.raises(FileNotFoundError):
        old.enter(TARGET, 4)
    assert current.finish() == {TARGET: 0}


@pytest.mark.parametrize('data, reason', [
    (b'x', 'incomplete'),
    (runner.DelayAccounting.record.pack(1, 4), 'invalid'),
    (runner.DelayAccounting.record.pack(0, float('nan')), 'invalid'),
    (runner.DelayAccounting.record.pack(0, 6), 'invalid'),
])
def test_malformed_journal_is_unusable_and_removed(tmp_path, data, reason):
    accounting = runner.DelayAccounting(tmp_path, [TARGET])
    Path(accounting.path).write_bytes(data)
    with pytest.raises(runner.ForcingError, match=reason):
        accounting.finish()
    assert not Path(accounting.path).exists()


@pytest.mark.parametrize('parallel', [2, 4])
def test_forcing_cannot_mix_xdist_worker_evidence(repo, parallel):
    with pytest.raises(ValueError, match='without xdist'):
        test_tool.run(repo, CONTAINER, ('test_product.py',), parallel=parallel, forcing=FORCING)


def test_independent_checking_keeps_killed_child_assertion_classification(repo):
    selection = {'tests': ['test_product.py::test_killed'], 'forcing': FORCING.as_json()}
    child = subprocess.run([sys.executable, '-I', '-B', str(RUNNER), json.dumps(selection), str(repo)],
                           capture_output=True, check=False)
    from tl3_harness.report import decode, parse
    from tl3_harness.model import Outcome
    outcomes = parse([decode(line) for line in child.stdout.splitlines()], selection['tests'], set(), child.returncode)
    assert outcomes == {'test_product.py::test_killed': Outcome.ASSERTION}
