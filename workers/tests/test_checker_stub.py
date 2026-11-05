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
