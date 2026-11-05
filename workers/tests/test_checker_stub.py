"""#807: unit-test recordings reject unrecorded and out-of-order calls."""
import os
import base64
import json
import subprocess

import pytest
from tl3_workers import reply_check
from tl3_workers.reply_check import ReplyCheckInfrastructureError

import checker_stub


def test_copied_fixture_facts_follow_paths_and_encoded_logs():
    before = {'result': {'stdout': {'path': '/recording/tools/action.log', 'bytes': 123}}}
    after = {'result': {'stdout': {'path': '/current-test/tools/action.log', 'bytes': 124}}}
    response = {'return': before['result']['stdout']}
    assert checker_stub.rebound(response, before, after) == {'return': after['result']['stdout']}
    def encoded(value):
        return base64.b64encode(json.dumps(value).encode()).decode()
    assert checker_stub.rebound({'return': [0, before]}, {'raw': encoded(before)},
                                {'raw': encoded(after)}) == {'return': [0, after]}
    # Refusals remain recorded, and ambiguous copies cannot select a new value.
    assert checker_stub.rebound({'return': False}, before, after) == {'return': False}
    repeated = {'a': before, 'b': before}
    assert checker_stub.rebound(response, repeated, {'a': before, 'b': after}) == response


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
