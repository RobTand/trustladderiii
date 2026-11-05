"""#807: unit-test recordings reject unrecorded and out-of-order calls."""
from copy import deepcopy
import os
import subprocess

import pytest
from tl3_workers import reply_check
from tl3_workers.reply_check import ReplyCheckInfrastructureError

import checker_stub


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


@pytest.mark.parametrize("pending", [False, True])
def test_handoff_replay_binds_current_git_samples_without_redeciding(tmp_path, monkeypatch, pending):
    row = next(row for rows in checker_stub.cases().values() for row in rows
               if row["command"] == ["check-reply", "maker"]
               and row["document"].get("operation") == "handoff"
               and bool(row["response"]["handoff"]) == pending)
    original = deepcopy(row)
    monkeypatch.setattr(checker_stub, "cases", lambda: {"sampled-handoff": [row]})
    request = deepcopy(row["document"])
    request.update(head="new-head", tree="new-tree")
    expected = deepcopy(row["response"])
    if pending:
        request["inputs"]["check_base_sha"] = "new-base"
        expected["handoff"].update(base_sha="new-base", head_sha="new-head", head_tree="new-tree")
    assert checker_stub.replay("sampled-handoff", tmp_path / "cursor", row["command"], request) == expected
    assert row == original
