"""#807: unit-test recordings reject unrecorded and out-of-order calls."""
from concurrent.futures import ThreadPoolExecutor
import json
import math
import os
from pathlib import Path
import subprocess
import sys
import time

import pytest
from tl3_workers import reply_check
from tl3_workers.reply_check import ReplyCheckInfrastructureError

import checker_stub


def recorded_case():
    return next((case, rows) for case, rows in checker_stub.cases().items() if len(rows) >= 2)


@pytest.mark.parametrize("published", [False, True])
@pytest.mark.parametrize("existing", [False, True])
def test_interrupted_replay_keeps_a_complete_cursor(tmp_path, existing, published):
    case, rows = recorded_case()
    cursor = tmp_path / "cursor"
    if existing:
        cursor.write_text("0")
    prepare = getattr(checker_stub, "prepare_replay", None)
    if prepare is not None:
        prepare(case, cursor)
    source = '''
import os, sys
from pathlib import Path
sys.path.insert(0, sys.argv[1])
import checker_stub
case, destination, published = sys.argv[2:5]
cursor = Path(destination)
write_text, replace = Path.write_text, os.replace
def interrupted_write(path, data, *args, **kwargs):
    if path == cursor:
        if published == 'True':
            write_text(path, data, *args, **kwargs)
        else:
            path.open('w').close()
        os._exit(91)
    return write_text(path, data, *args, **kwargs)
def interrupted_replace(source, destination):
    if published == 'True':
        replace(source, destination)
    os._exit(91)
Path.write_text, os.replace = interrupted_write, interrupted_replace
row = checker_stub.cases()[case][0]
checker_stub.replay(case, cursor, row['command'], row['document'])
'''
    process = subprocess.run([sys.executable, "-c", source, str(Path(checker_stub.__file__).parent),
                              case, str(cursor), str(published)], capture_output=True, text=True)
    assert process.returncode == 91, process.stderr
    expected = "1" if published else "0" if existing else None
    assert (cursor.read_text() if cursor.exists() else None) == expected
    position = int(expected or "0")
    row = rows[position]
    assert checker_stub.replay(case, cursor, row["command"], row["document"]) == row["response"]
    assert cursor.read_text() == str(position + 1)


def test_prepared_replay_needs_no_corpus_in_the_child(tmp_path):
    case, rows = recorded_case()
    cursor = tmp_path / "cursor"
    prepare = getattr(checker_stub, "prepare_replay", None)
    if prepare is not None:
        prepare(case, cursor)
    stub = tmp_path / "checker_stub.py"
    stub.write_bytes(Path(checker_stub.__file__).read_bytes())
    for row in rows:
        process = subprocess.run([sys.executable, str(stub), "--replay", case, str(cursor), *row["command"]],
                                 input=json.dumps(row["document"]), capture_output=True, text=True)
        assert process.returncode == 0, process.stderr
        assert json.loads(process.stdout) == row["response"]
    process = subprocess.run([sys.executable, str(stub), "--replay", case, str(cursor), *rows[-1]["command"]],
                             input=json.dumps(rows[-1]["document"]), capture_output=True, text=True)
    assert process.returncode != 0 and not process.stdout
    assert "unrecorded Rust checker call" in process.stderr


@pytest.mark.parametrize("prepared", [False, True])
@pytest.mark.parametrize("wrong", ["command", "kind"])
def test_out_of_order_calls_do_not_advance_cursor(tmp_path, wrong, prepared):
    case, rows = recorded_case()
    row = rows[0]
    cursor = tmp_path / "cursor"
    prepare = getattr(checker_stub, "prepare_replay", None)
    if prepared and prepare is not None:
        prepare(case, cursor)
    command = ["wrong-command"] if wrong == "command" else row["command"]
    document = {"kind": "wrong-kind"} if wrong == "kind" else row["document"]
    with pytest.raises(RuntimeError, match="out-of-order Rust checker call"):
        checker_stub.replay(case, cursor, command, document)
    process = subprocess.run([sys.executable, checker_stub.__file__, "--replay", case, str(cursor), *command],
                             input=json.dumps(document), capture_output=True, text=True)
    assert process.returncode != 0 and not process.stdout
    assert "out-of-order Rust checker call" in process.stderr
    assert not cursor.exists()


def test_cursor_is_not_published_if_fsync_fails(tmp_path, monkeypatch):
    case, rows = recorded_case()
    cursor = tmp_path / "cursor"
    cursor.write_text("0")

    def failed_sync(descriptor):
        raise OSError("injected cursor sync failure")

    monkeypatch.setattr(os, "fsync", failed_sync)
    row = rows[0]
    with pytest.raises(OSError, match="injected cursor sync failure"):
        checker_stub.replay(case, cursor, row["command"], row["document"])
    assert cursor.read_text() == "0"
    assert list(tmp_path.iterdir()) == [cursor]


def test_replay_latency(checker_state_directory, capsys):
    """Measure actual child calls under four concurrent replay streams, without a timing threshold."""
    case, rows = recorded_case()
    row = rows[0]
    source = '''
import sys, time
started = time.perf_counter()
sys.path.insert(0, sys.argv[1])
import checker_stub
imported = time.perf_counter()
from pathlib import Path
import json
if sys.argv[2] == 'corpus':
    checker_stub.cases()
else:
    del sys.argv[1]
    checker_stub.main()
finished = time.perf_counter()
print('CHECKER_TIMING ' + json.dumps([imported-started, finished-imported]), file=sys.stderr)
'''

    def sample(index):
        cursor = checker_state_directory / str(index)
        prepare = getattr(checker_stub, "prepare_replay", None)
        if prepare is not None:
            prepare(case, cursor)
        measurements = {}
        for mode in ("startup", "corpus", "replay"):
            command = [sys.executable, "-c", "pass"] if mode == "startup" else [
                sys.executable, "-c", source, str(Path(checker_stub.__file__).parent),
                "corpus" if mode == "corpus" else "--replay", case, str(cursor), *row["command"]]
            if mode == "replay":
                command.insert(1, "-IS")
            started = time.perf_counter()
            process = subprocess.run(command, input=json.dumps(row["document"]), capture_output=True, text=True)
            measurements[mode] = time.perf_counter() - started
            assert process.returncode == 0, process.stderr
            if mode != "startup":
                phases = json.loads(process.stderr.split("CHECKER_TIMING ")[1])
                measurements[mode + "_import"], measurements[mode + "_body"] = phases
            if mode == "replay":
                assert json.loads(process.stdout) == row["response"]
        return measurements

    with ThreadPoolExecutor(max_workers=4) as pool:
        samples = list(pool.map(sample, range(48)))
    summary = {}
    for name in samples[0]:
        values = sorted(sample[name] * 1000 for sample in samples)
        summary[name] = {f"p{percentile}": round(values[math.ceil(len(values) * percentile / 100) - 1], 3)
                         for percentile in (50, 95, 99, 100)}
    with capsys.disabled():
        print("CHECKER_LATENCY_MS " + json.dumps({"samples": len(samples), "streams": 4, "timings": summary}, sort_keys=True))


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
