"""#826: launcher stderr is bounded operator evidence, never a test verdict."""
import json
import sys
import tempfile
import tracemalloc

import pytest

from tl3_harness import execution, test_tool
from tl3_harness.supervision import Supervised


IMAGE = execution.Container('sha256:' + '1' * 64)
CAUSE = 'container shim refused: pid limit reached'


def command(source):
    return [sys.executable, '-I', '-B', '-c', source]


def observe(tmp_path, monkeypatch, source):
    monkeypatch.setattr(test_tool, 'require_admission', lambda: None)
    monkeypatch.setattr(test_tool, 'container_command', lambda *args: command(source))
    return test_tool.run_current(tmp_path, IMAGE, ())


@pytest.mark.parametrize('code,outcome', [(125, 'setup_failure'), (1, 'unknown_incomplete')])
def test_early_exit_keeps_cause_outside_test_messages(tmp_path, monkeypatch, code, outcome):
    result = observe(tmp_path, monkeypatch, f'import os; os.write(2, {CAUSE.encode()!r}); os._exit({code})')
    assert result['status'] == 'infrastructure_error'
    assert result['execution']['outcome'] == outcome
    assert result['execution'].get('stderr_tail') == CAUSE, 'the container failure cause was discarded'
    assert CAUSE not in result['message']
    assert result['tests'] == []


@pytest.mark.parametrize('payload', [
    b'old\n' * 8192 + b'last diagnostic\n',
    b'x' * 32768 + b'last diagnostic\n',
    b'\xff\xfe\x00\x1b\r\tbad utf8\nlast diagnostic\n',
    ('\u202e\u0085\u2028\U0001f600' * 8192 + '\nlast diagnostic\n').encode(),
])
def test_total_line_and_encoding_bounds(tmp_path, monkeypatch, payload):
    # The payload is a file so Linux's argv limit cannot hide the real failure.
    path = tmp_path / 'stderr.bin'
    path.write_bytes(payload)
    source = f'import os; os.write(2, open({str(path)!r}, "rb").read()); os._exit(125)'
    result = observe(tmp_path, monkeypatch, source)
    tail = result['execution'].get('stderr_tail', '')
    assert tail.endswith('last diagnostic\n'), 'the newest stderr must survive clipping'
    assert len(tail.encode('utf-8')) <= 16_384
    assert len(tail) <= 2048
    assert all(len(line) <= 512 for line in tail.split('\n'))
    assert all(char == '\n' or char.isprintable() for char in tail)
    assert len(json.dumps(result).encode()) <= test_tool.SUMMARY_BYTES - 2048
    if payload.startswith(b'\xff'):
        assert '\ufffd' in tail, 'invalid UTF-8 must be replaced without losing the cause'


def test_stderr_flood_drains_with_stdout_and_fixed_memory():
    source = '''
import os
for _ in range(128):
    os.write(2, b'x' * 65536)
    os.write(1, b'{"event":"start","schema":1}\\n')
os.write(2, b'\\nlast diagnostic\\n')
os._exit(125)
'''
    tracemalloc.start()
    try:
        with tempfile.TemporaryFile() as selection, Supervised(command(source), selection, ()) as process:
            records = sum(1 for _ in process.records())
            failure = process.incomplete()
            retained = getattr(process, 'stderr_tail', b'')
            _, peak = tracemalloc.get_traced_memory()
        assert records == 128, 'stderr must not block stdout'
        assert len(retained) <= 16_384
        assert failure.as_json().get('stderr_tail', '').endswith('last diagnostic\n')
        assert peak < 1_048_576, f'8 MiB of stderr grew host Python memory to {peak} bytes'
    finally:
        tracemalloc.stop()


def test_success_records_no_stderr_diagnostic(tmp_path, monkeypatch, capsys):
    source = ('import os; os.write(2, b"startup warning"); '
              'os.write(1, b\'{"event":"start","schema":1}\\n{"event":"finish","exit_code":0}\\n\')')
    result = observe(tmp_path, monkeypatch, source)
    assert result['status'] == 'completed'
    assert 'execution' not in result
    assert 'startup warning' not in json.dumps(result)
    assert 'startup warning' not in capsys.readouterr().out


def test_checking_logs_diagnostics_without_maker_feedback(tmp_path, monkeypatch, capsys):
    test = 'test_value.py::test_ok'
    (tmp_path / 'test_value.py').write_text('def test_ok(): pass\n')
    monkeypatch.setattr(execution, 'container_command', lambda *args: command(
        f'import os; os.write(2, {CAUSE.encode()!r}); os._exit(125)'))
    messages = {}
    observed = execution.run_tree(tmp_path, IMAGE, (test,), tmp_path / 'report.jsonl',
                                  execution.Progress(), messages=messages)
    assert observed == {test: execution.Outcome.NO_REPORT}
    lines = [line.removeprefix('TL3_EXECUTION_FAILURE ') for line in capsys.readouterr().out.splitlines()
             if line.startswith('TL3_EXECUTION_FAILURE ')]
    assert lines, 'Checking must publish the failure in the PB action log'
    assert json.loads(lines[-1])['stderr_tail'] == CAUSE
    assert CAUSE not in json.dumps(messages)
