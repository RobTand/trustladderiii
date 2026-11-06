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
import os
import sys
args = sys.argv[1:]
with pathlib.Path({str(calls)!r}).open('a') as log:
    log.write(json.dumps(args) + '\\n')
if args[0] == 'run':
    pathlib.Path({str(tmp_path / 'run-pid')!r}).write_text(str(os.getpid()))
    evidence = pathlib.Path({str(tmp_path / 'stderr')!r})
    if evidence.exists():
        sys.stderr.write(evidence.read_text())
        sys.stderr.flush()
        if os.read(0, 1) != b'\\x01':
            sys.exit(125)
        pathlib.Path({str(tmp_path / 'payload')!r}).write_bytes(sys.stdin.buffer.read())
    print('{{"event":"start","schema":1}}', flush=True)
    sys.exit(0)
if args[:2] == ['container', 'inspect']:
    pid = int(pathlib.Path({str(tmp_path / 'run-pid')!r}).read_text())
    print(json.dumps({{'Id': 'a' * 64, 'Pid': pid, 'ConmonPid': pid}}))
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


def test_pb_docker_cleanup_keeps_managed_runtime(launcher):
    runtime, calls = launcher
    docker = runtime.with_name('docker')
    runtime.rename(docker)
    with calls.with_suffix('.stdin').open('w+b') as selection:
        with Supervised([str(docker), 'run', 'immutable-fixture'], selection, ()) as process:
            list(process.records())
    commands = [json.loads(line) for line in calls.read_text().splitlines()]
    name = commands[0][commands[0].index('--name') + 1]
    assert commands[1:] == [['rm', '--force', name]]


def test_flux_command_at_launcher_boundary(launcher):
    from tl3_harness.runtime import podman_command

    runtime, calls = launcher
    scratch = runtime.parent / 'private'
    scratch.mkdir(mode=0o700)
    command = podman_command(runtime.parent / 'checkout', 'sha256:' + '1' * 64, scratch, 1)
    with calls.with_suffix('.stdin').open('w+b') as selection:
        selection.write(b'[{"test": "private-selector"}]')
        selection.seek(0)
        with Supervised(command, selection, ()) as process:
            list(process.records())
    arguments = json.loads(calls.read_text().splitlines()[0])
    for flag in ('--cgroups=disabled', '--network=none', '--read-only', '--init', '--userns=keep-id',
                 '--pull=never', '--cap-drop=ALL', '--security-opt=no-new-privileges', '--ipc=private',
                 '--read-only-tmpfs=false', '--pid=private'):
        assert flag in arguments
    mounts = [arguments[i + 1] for i, flag in enumerate(arguments) if flag == '--mount']
    assert len(mounts) == 3
    assert len([mount for mount in mounts if mount.endswith(',readonly')]) == 2
    assert f'type=bind,src={scratch},dst=/tmp' in mounts
    assert not any('flux-out' in mount or 'token' in mount for mount in mounts)
    assert '--cpuset-cpus' not in arguments
    assert arguments[-2:] == ['/harness/runtime_evidence.py', '-']


def test_owned_scratch_cleanup_refuses_replaced_leaf(tmp_path):
    from tl3_harness import scratch

    tmp_path.chmod(0o700)
    leaf = scratch.private_directory(tmp_path)
    original = leaf.with_suffix('.retained')
    leaf.rename(original)
    leaf.mkdir()
    (leaf / 'foreign').write_text('keep')
    with pytest.raises(RuntimeError, match='identity changed'):
        scratch.release_directory(leaf)
    assert (leaf / 'foreign').read_text() == 'keep'


def test_owned_scratch_removes_only_fresh_leaf(tmp_path):
    from tl3_harness import scratch

    tmp_path.chmod(0o700)
    leaf = scratch.private_directory(tmp_path)
    sibling = tmp_path / 'unrelated'
    sibling.mkdir()
    (leaf / 'payload').write_text('temporary')
    scratch.release_directory(leaf)
    assert not leaf.exists()
    assert sibling.is_dir()
    with pytest.raises(RuntimeError, match='unowned'):
        scratch.release_directory(sibling)


@pytest.mark.parametrize('evidence_present', [False, True])
def test_runtime_evidence_is_retained_outside_report(launcher, monkeypatch, evidence_present):
    from tl3_harness import runtime
    from tl3_harness.runtime_evidence import PREFIX

    executable, calls = launcher
    output = executable.parent / 'output'
    output.mkdir()
    facts = {'Cpus_allowed_list': '2-3', 'cgroup': '0::/flux/job-fixture\n'}
    monkeypatch.setattr(runtime.Evidence, 'host_snapshot', staticmethod(lambda: facts))
    monkeypatch.setattr(runtime, 'evidence_sink', lambda name: runtime.Evidence(output, name))
    monkeypatch.setattr(runtime, 'validate_runtime_binding', lambda evidence: None)
    if evidence_present:
        (executable.parent / 'stderr').write_text(PREFIX + json.dumps(facts) + '\n')
    with calls.with_suffix('.stdin').open('w+b') as selection:
        selection.write(b'[\"test_private.py::test_case\"]')
        selection.seek(0)
        if evidence_present:
            with Supervised([str(executable), 'run', 'immutable-fixture'], selection, ()) as process:
                assert list(process.records()) == [b'{"event":"start","schema":1}\n']
                assert process.stderr_diagnostic == ''
            assert (executable.parent / 'payload').read_bytes() == b'[\"test_private.py::test_case\"]'
        else:
            with pytest.raises(ExecutionFailure, match='container_cleanup_unconfirmed'):
                with Supervised([str(executable), 'run', 'immutable-fixture'], selection, ()) as process:
                    list(process.records())
    records = [json.loads(line) for line in next(output.iterdir()).read_text().splitlines()]
    assert records[0] == {'event': 'host', **facts}
    assert records[-1] == {'event': 'cleanup', 'confirmed': evidence_present}
    assert [record['event'] for record in records] == (
        ['host', 'container', 'container_processes', 'cleanup'] if evidence_present
        else ['host', 'execution_failure', 'cleanup'])
    if not evidence_present:
        assert records[1]['termination_reason'] == 'missing_runtime_evidence'


def test_unqualified_binding_never_opens_payload_gate(launcher, monkeypatch):
    from tl3_harness import runtime
    from tl3_harness.runtime_evidence import PREFIX

    executable, calls = launcher
    output = executable.parent / 'output'
    output.mkdir()
    facts = {'Cpus_allowed_list': '2-3', 'cgroup': '0::/unqualified\n'}
    monkeypatch.setattr(runtime.Evidence, 'host_snapshot', staticmethod(lambda: facts))
    monkeypatch.setattr(runtime, 'evidence_sink', lambda name: runtime.Evidence(output, name))

    def reject(evidence):
        assert evidence.container_seen
        raise RuntimeError('Rust reports unsupported mapping')

    monkeypatch.setattr(runtime, 'validate_runtime_binding', reject)
    (executable.parent / 'stderr').write_text(PREFIX + json.dumps(facts) + '\n')
    with calls.with_suffix('.stdin').open('w+b') as selection:
        selection.write(b'["test_private.py::test_case"]')
        selection.seek(0)
        with pytest.raises(ExecutionFailure) as failed:
            with Supervised([str(executable), 'run', 'immutable-fixture'], selection, ()) as process:
                list(process.records())
    assert failed.value.termination_reason == 'runtime_binding_unqualified'
    assert failed.value.outcome == ExecutionOutcome.UNKNOWN_INCOMPLETE
    assert not (executable.parent / 'payload').exists()
    assert not process.report_started
    assert process.cleanup_confirmed


@pytest.mark.parametrize('exists', ['0', '1'])
def test_scratch_is_removed_only_after_confirmed_container_stop(launcher, monkeypatch, exists):
    from tl3_harness import runtime, scratch

    executable, calls = launcher
    executable.parent.chmod(0o700)
    (executable.parent / 'exists').write_text(exists)
    monkeypatch.setattr(runtime, '_uncertain_containers', set())
    monkeypatch.setattr(runtime, 'temporary_root', lambda: executable.parent)
    leaf = scratch.private_directory(executable.parent)

    def run():
        with runtime.workspace('exports-') as workspace:
            retained.append(workspace)
            with calls.with_suffix('.stdin').open('w+b') as selection:
                with Supervised([str(executable), 'run', '--mount', f'type=bind,src={leaf},dst=/tmp',
                                 'immutable-fixture'], selection, ()) as process:
                    list(process.records())

    retained = []
    if exists == '0':
        with pytest.raises(ExecutionFailure, match='container_cleanup_unconfirmed'):
            run()
        assert leaf.is_dir() and retained[0].is_dir()
    else:
        run()
        assert not leaf.exists() and not retained[0].exists()


def test_absent_container_does_not_hide_a_live_owned_process(launcher, monkeypatch):
    import os
    import subprocess

    from tl3_harness import runtime
    from tl3_harness.runtime_evidence import PREFIX

    executable, calls = launcher
    output = executable.parent / 'output'
    output.mkdir()
    facts = {'Cpus_allowed_list': '2-3', 'cgroup': '0::/fixture\n'}
    monkeypatch.setattr(runtime.Evidence, 'host_snapshot', staticmethod(lambda: facts))
    monkeypatch.setattr(runtime, 'evidence_sink', lambda name: runtime.Evidence(output, name))
    monkeypatch.setattr(runtime, 'validate_runtime_binding', lambda evidence: None)
    (executable.parent / 'stderr').write_text(PREFIX + json.dumps(facts) + '\n')
    # This is a test-owned, pipe-bound process in the admitted Bazel action.
    # It models a conmon/payload that survives both attach exit and runtime absence.
    child = subprocess.Popen([sys.executable, '-I', '-c',
                              'import sys; print("ready", flush=True); sys.stdin.buffer.read()'],
                             stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    try:
        assert child.stdout.readline() == b'ready\n'
        monkeypatch.setattr(runtime, 'capture_processes', lambda *args: [os.pidfd_open(child.pid)], raising=False)
        with calls.with_suffix('.stdin').open('w+b') as selection:
            with pytest.raises(ExecutionFailure, match='container_cleanup_unconfirmed'):
                with Supervised([str(executable), 'run', 'immutable-fixture'], selection, ()) as process:
                    list(process.records())
        assert child.poll() is None
        assert not process.cleanup_confirmed
        records = [json.loads(line) for line in next(output.iterdir()).read_text().splitlines()]
        assert records[-1] == {'event': 'cleanup', 'confirmed': False}
    finally:
        child.kill()
        child.wait(timeout=5)
        child.stdin.close()
        child.stdout.close()
