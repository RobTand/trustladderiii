"""#807: unit-test recordings reject unrecorded and out-of-order calls."""
import os
import subprocess

import pytest
from tl3_workers import reply_check
from tl3_workers.reply_check import ReplyCheckInfrastructureError

import checker_stub


@pytest.mark.parametrize('kind', ['summary', 'identity'])
def test_harness_replay_requires_full_document_equality(tmp_path, monkeypatch, kind):
    command = ['check-harness', kind]
    document = {'kind': kind, 'limit': 16_384, 'facts': {'message': 'original'}}
    row = {'command': command, 'document': document, 'response': {'result': 'recorded'}}
    monkeypatch.setattr(checker_stub, 'cases', lambda: {'local harness case': [row]})
    changed = {**document, 'facts': {'message': 'changed'}}
    with pytest.raises(RuntimeError, match='document'):
        checker_stub.replay('local harness case', tmp_path / 'cursor', command, changed)
    with pytest.raises(RuntimeError, match='document'):
        checker_stub.replay('local harness case', tmp_path / 'cursor', command, {**document, 'limit': 16_384.0})
    assert not (tmp_path / 'cursor').exists()
    assert checker_stub.replay('local harness case', tmp_path / 'cursor', command, document) == row['response']


def test_unrecorded_calls_fail_closed_in_process_and_in_child(tmp_path):
    request = {"kind": "no_such_parity_vector"}
    with pytest.raises(RuntimeError, match="unrecorded Rust checker call"):
        checker_stub.replay("unknown case", tmp_path / "cursor", ["check-budget"], request)
    process = subprocess.Popen([os.environ["TL3_BINARY"], "check-budget"], text=True,
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    stdout, stderr = process.communicate('{"kind":"no_such_parity_vector"}', timeout=5)
    assert process.returncode != 0 and not stdout
    assert "unrecorded Rust checker call" in stderr
    with pytest.raises(ReplyCheckInfrastructureError, match="process failed"):
        reply_check.run_check(["check-budget"], request)


def test_staged_replay_never_loads_the_global_corpus(tmp_path, monkeypatch):
    command, document = ['check-harness', 'identity'], {'kind': 'identity', 'selectors': []}
    row = {'command': command, 'document': document, 'response': {'result': True}}
    monkeypatch.setattr(checker_stub, 'cases', lambda: pytest.fail('global corpus read'))
    assert checker_stub.replay('staged', tmp_path / 'cursor', command, document,
                               script=[row]) == row['response']


def test_concurrent_checker_processes_consume_each_step_once(tmp_path, monkeypatch):
    # #823: child workers share the cursor, beyond the parent's threading lock.
    import multiprocessing
    import time
    from pathlib import Path

    context = multiprocessing.get_context('fork')
    truncated = context.Event()
    cursor = tmp_path / 'concurrent-cursor'
    command, document = ['check-harness', 'identity'], {'kind': 'identity', 'selectors': []}
    row = {'command': command, 'document': document, 'response': {'result': True}}
    monkeypatch.setattr(checker_stub, 'cases', lambda: {'concurrent': [row, row]})
    original = Path.write_text

    def delayed_write(path, value, *args, **kwargs):
        if path != cursor:
            return original(path, value, *args, **kwargs)
        with path.open('w') as stream:
            truncated.set()
            time.sleep(0.2)
            return stream.write(value)

    monkeypatch.setattr(Path, 'write_text', delayed_write)
    processes = [context.Process(target=checker_stub.replay,
                                args=('concurrent', cursor, command, document)) for _ in range(2)]
    try:
        processes[0].start()
        assert truncated.wait(5), 'first checker never reached its cursor write'
        processes[1].start()
        for process in processes:
            process.join(5)
        assert [process.exitcode for process in processes] == [0, 0]
        assert cursor.read_text() == '2'
    finally:
        for process in processes:
            if process.pid is not None and process.is_alive():
                process.terminate()
                process.join(5)
