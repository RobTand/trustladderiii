"""Controlled executable fixtures exercise the real launcher and cleanup boundary."""
import json
from pathlib import Path
import sys

import pytest

from tl3_harness.supervision import ExecutionFailure, ExecutionOutcome, Supervised


@pytest.fixture
def launcher(tmp_path, monkeypatch):
    runtime = tmp_path / 'podman'
    calls = tmp_path / 'calls.jsonl'
    runtime.write_text(f'''#!{sys.executable}
import json
import pathlib
import sys
args = sys.argv[1:]
with pathlib.Path({str(calls)!r}).open('a') as log:
    log.write(json.dumps(args) + '\\n')
if args[0] == 'run':
    print('{{"event":"start","schema":1}}', flush=True)
    sys.exit(0)
if args[:2] == ['container', 'exists']:
    sys.exit(int(pathlib.Path({str(tmp_path / 'exists')!r}).read_text()))
sys.exit(0)
''')
    runtime.chmod(0o700)
    (tmp_path / 'exists').write_text('1')
    monkeypatch.setenv('PATH', str(tmp_path))
    monkeypatch.setenv('PRISMABUILD_ACTION_PROGRESS_ALLOWANCES', '{"tests": 10}')
    return runtime, calls


def test_podman_cleanup_uses_owned_container_identity(launcher):
    runtime, calls = launcher
    # Popen needs a real stdin file; selection is normally a TemporaryFile.
    with Path(calls).with_suffix('.stdin').open('w+b') as selection:
        with Supervised([str(runtime), 'run', 'immutable-fixture'], selection, ()) as process:
            assert list(process.records()) == [b'{"event":"start","schema":1}\n']
    commands = [json.loads(line) for line in calls.read_text().splitlines()]
    name = commands[0][commands[0].index('--name') + 1]
    assert commands[1:] == [['rm', '--force', name], ['container', 'exists', name]]
    assert process.cleanup_confirmed


@pytest.mark.parametrize('exists', ['0', '125'])
def test_launcher_exit_does_not_certify_podman_cleanup(launcher, exists):
    runtime, calls = launcher
    (runtime.parent / 'exists').write_text(exists)
    with calls.with_suffix('.stdin').open('w+b') as selection:
        with pytest.raises(ExecutionFailure) as failed:
            with Supervised([str(runtime), 'run', 'immutable-fixture'], selection, ()) as process:
                list(process.records())
    assert failed.value.outcome == ExecutionOutcome.UNKNOWN_INCOMPLETE
    assert failed.value.termination_reason == 'container_cleanup_unconfirmed'
    assert not process.cleanup_confirmed
