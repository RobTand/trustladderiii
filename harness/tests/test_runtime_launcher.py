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
if args[:1] == ['--cgroup-manager=systemd']:
    args = args[1:]
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
    if any('{{{{json .Id}}}}' in argument for argument in args):
        sys.exit(125)
    pid = int(pathlib.Path({str(tmp_path / 'run-pid')!r}).read_text())
    launch_path = pathlib.Path({str(tmp_path / 'launch.json')!r})
    launch = json.loads(launch_path.read_text()) if launch_path.exists() else {{}}
    print(json.dumps({{'Id': 'a' * 64, 'Pid': pid, 'ConmonPid': pid,
                      'Name': launch.get('name'), 'Labels': launch.get('labels')}}))
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
    command = podman_command(runtime.parent / 'checkout', 'sha256:' + '1' * 64, scratch, 1, mode='in_job')
    with calls.with_suffix('.stdin').open('w+b') as selection:
        selection.write(b'[{"test": "private-selector"}]')
        selection.seek(0)
        with Supervised(command, selection, ()) as process:
            list(process.records())
    arguments = json.loads(calls.read_text().splitlines()[0])
    for flag in ('--cgroups=disabled', '--network=none', '--read-only', '--init', '--userns=keep-id',
                 '--pull=never', '--cap-drop=ALL', '--security-opt=no-new-privileges', '--ipc=private',
                 '--read-only-tmpfs=false', '--pid=private', '--http-proxy=false'):
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


def test_explicit_backend_selection_keeps_pb_default(monkeypatch):
    from tl3_harness import runtime

    monkeypatch.delenv('TL3_EXECUTION_BACKEND', raising=False)
    monkeypatch.setenv('FLUX_JOB_ID', 'nested-native-job')
    assert runtime.launch_context() is None
    monkeypatch.setenv('TL3_EXECUTION_BACKEND', 'prismabuild')
    assert runtime.launch_context() is None
    monkeypatch.setenv('TL3_EXECUTION_BACKEND', 'misspelled')
    with pytest.raises(RuntimeError, match='execution backend'):
        runtime.launch_context()


@pytest.fixture
def backend_cli(tmp_path, monkeypatch):
    executable = tmp_path / 'backend-cli'
    calls = tmp_path / 'backend-calls.json'
    reply = tmp_path / 'reply'
    executable.write_text(f'''#!{sys.executable}
import json
import pathlib
import sys
pathlib.Path({str(calls)!r}).write_text(json.dumps({{'argv': sys.argv[1:], 'request': json.load(sys.stdin)}}))
sys.stdout.write(pathlib.Path({str(reply)!r}).read_text())
''')
    executable.chmod(0o700)
    monkeypatch.setenv('TL3_EXECUTION_BACKEND', 'flux')
    monkeypatch.setenv('TL3_RUN_BACKEND_CLI', str(executable))
    monkeypatch.setenv('TL3_RUN_BACKEND_CONFIG', str(tmp_path / 'options.json'))
    monkeypatch.setenv('TL3_RUN_OWNER', 'owner')
    monkeypatch.setenv('TL3_RUN_CORRELATION', 'correlation')
    monkeypatch.setenv('FLUX_JOB_ID', 'fJob')
    return reply, calls


def test_launch_context_consumes_generated_cli_boundary(backend_cli):
    from tl3_harness import runtime

    reply, calls = backend_cli
    context = {'run': {'backend': 'flux', 'uri': 'local:///native', 'instance': 'run/generation', 'job': 'fJob'}}
    reply.write_text(json.dumps({'schema': 'tl3.run_backend.frame.v1', 'sequence': 0, 'kind': 'result',
                                'result': {'op': 'launch_context', 'context': context}}) + '\n')
    assert runtime.launch_context() == context
    invocation = json.loads(calls.read_text())
    assert invocation['argv'] == ['--config', str(reply.parent / 'options.json')]
    assert invocation['request'] == {'op': 'launch_context', 'owner': 'owner', 'correlation': 'correlation', 'job': 'fJob'}
    assert runtime.cache_owner() == ('flux', 'local:///native', 'run/generation', 'fJob')


@pytest.mark.parametrize('response', [
    '', 'not-json\n',
    '{"schema":"tl3.run_backend.frame.v1","sequence":0,"kind":"error","code":"invalid","message":"refused"}\n',
    '{"schema":"tl3.run_backend.frame.v1","sequence":1,"kind":"result","result":{"op":"launch_context","context":{}}}\n',
    '{"schema":"tl3.run_backend.frame.v1","sequence":0,"kind":"result","result":{"op":"probe","context":{}}}\n',
    '{"schema":"tl3.run_backend.frame.v1","sequence":0,"kind":"result","result":{"op":"launch_context","context":{}}}\n' * 2,
])
def test_backend_cli_refuses_invalid_frames_without_pb_fallback(backend_cli, response):
    from tl3_harness import runtime

    reply, _ = backend_cli
    reply.write_text(response)
    with pytest.raises(RuntimeError):
        runtime.launch_context()


def test_own_scope_command_keeps_mapped_grant_and_has_no_in_job_fallback(launcher):
    from tl3_harness.runtime import podman_command

    executable, calls = launcher
    command = podman_command(executable.parent / 'checkout', 'sha256:' + '1' * 64,
                             executable.parent / 'private', 1, executable=executable,
                             mode='own_scope', cpus='21-23,30,61-63,70', memory=268435456)
    with calls.with_suffix('.stdin').open('w+b') as selection:
        with Supervised(command, selection, ()) as process:
            list(process.records())
    arguments = json.loads(calls.read_text().splitlines()[0])
    assert arguments[:2] == ['--cgroup-manager=systemd', 'run']
    assert '--cgroups=enabled' in arguments
    assert '--init' not in arguments and '--cgroups=disabled' not in arguments
    assert arguments[arguments.index('--cpuset-cpus') + 1] == '21-23,30,61-63,70'
    assert arguments[arguments.index('--memory') + 1] == '268435456'
    assert arguments[arguments.index('--memory-swap') + 1] == '268435456'
    assert '--pull=never' in arguments and process.cleanup_confirmed
    assert len([json.loads(line) for line in calls.read_text().splitlines() if 'run' in json.loads(line)]) == 1


@pytest.mark.parametrize('options', [
    {'mode': 'own_scope'}, {'mode': 'own_scope', 'cpus': '1', 'memory': None},
    {'mode': 'own_scope', 'cpus': None, 'memory': 1024}, {'mode': 'typo'},
    {'mode': 'in_job', 'cpus': '1', 'memory': 1024},
])
def test_container_mode_refuses_missing_or_inapplicable_limits(options, tmp_path):
    from tl3_harness.runtime import podman_command

    with pytest.raises(ValueError):
        podman_command(tmp_path, 'sha256:' + '1' * 64, tmp_path, 1, **options)


def test_host_collector_forwards_raw_stdout_to_exact_rust_operation(tmp_path, monkeypatch):
    import hashlib
    from tl3_harness import runtime

    output = tmp_path / 'out'
    output.mkdir()
    run = {'backend': 'flux', 'uri': 'local:///native', 'instance': 'run/generation', 'job': 'fJob'}
    context = {'output_dir': str(output), 'run': run, 'owner': 'owner', 'correlation': 'correlation'}
    monkeypatch.setattr(runtime, 'launch_context', lambda: context)
    requests = []

    def backend(request):
        requests.append(request)
        return {'op': 'collect_runner', 'report': {'path': 'rust-owned-report'}, 'payload': {'path': 'rust-owned-payload'}}

    monkeypatch.setattr(runtime, 'backend_request', backend)
    raw = b'TL3_CHECKS {"unchanged":"payload"}\nraw tail\n'
    code, result = runtime.collect_host_payload(
        [sys.executable, '-I', '-c', 'import sys;sys.stdout.buffer.write(' + repr(raw) + ');sys.stderr.write("diagnostic")'],
        timeout=10)
    assert code == 0 and result['report']['path'] == 'rust-owned-report'
    assert (output / 'host.stdout').read_bytes() == raw
    assert (output / 'host.stderr').read_text() == 'diagnostic'
    assert requests == [{'op': 'collect_runner', 'owner': 'owner', 'correlation': 'correlation',
                         'facts': {'run': run, 'stdout': {'path': str(output / 'host.stdout'),
                                   'sha256': hashlib.sha256(raw).hexdigest(), 'bytes': len(raw)},
                                   'runtime_binding': None, 'cleanup': None}}]


def test_host_collector_never_overwrites_prior_output(tmp_path, monkeypatch):
    from tl3_harness import runtime

    (tmp_path / 'host.stdout').write_text('retained')
    monkeypatch.setattr(runtime, 'launch_context', lambda: {'output_dir': str(tmp_path)})
    with pytest.raises(FileExistsError):
        runtime.collect_host_payload([sys.executable, '-c', 'raise SystemExit(99)'], timeout=10)
    assert (tmp_path / 'host.stdout').read_text() == 'retained'


def test_host_artifact_staging_produces_executable_and_hashed_readonly_manifest(tmp_path):
    import hashlib
    import subprocess
    from tl3_harness import runtime

    destination = tmp_path / 'artifact'
    executable, manifest, trusted_root = runtime.stage_host_runtime(destination, interpreter=sys.executable, producer_commit='a' * 40,
                                                                    podman='/explicit/podman', systemctl='/explicit/systemctl')
    files = json.loads(manifest.read_text())
    assert files['interpreter'] == sys.executable
    assert trusted_root.joinpath('runner.py').is_file()
    assert trusted_root.joinpath('runtime_evidence.py').is_file()
    for entry in files['files']:
        path = executable.parent / entry['path']
        assert hashlib.sha256(path.read_bytes()).hexdigest() == entry['sha256']
        assert path.stat().st_size == entry['bytes']
        assert path.stat().st_mode & 0o777 == 0o600
    assert manifest.stat().st_mode & 0o777 == 0o600
    assert executable.stat().st_mode & 0o777 == 0o600
    completed = subprocess.run([sys.executable, '-I', str(executable), '--help'], capture_output=True, timeout=10, check=True)
    assert b'--timeout' in completed.stdout
    with pytest.raises(FileExistsError):
        runtime.stage_host_runtime(destination, interpreter=sys.executable, producer_commit='a' * 40,
                                   podman='/explicit/podman', systemctl='/explicit/systemctl')


@pytest.fixture
def cleanup_fixture(tmp_path):
    import subprocess

    root = tmp_path
    container_id = 'c' * 64
    labels = {'tl3.job': 'fJob', 'tl3.instance': 'run/generation', 'tl3.owner': 'owner'}
    cgroup = root / 'cgroups' / 'user.slice' / 'owned.scope'
    cgroup.mkdir(parents=True)
    (cgroup / 'cgroup.events').write_text('populated 1\nfrozen 0\n')
    (cgroup / 'cgroup.procs').write_text('987654\n')
    properties = root / 'properties'
    properties.write_text('LoadState=loaded\nActiveState=active\nInvocationID=' + 'd' * 32 + '\nControlGroup=/user.slice/owned.scope\n')
    podman = root / 'podman'
    metadata = root / 'metadata'
    metadata.write_text(json.dumps({'Id': container_id, 'Name': 'tl3-owned', 'Labels': labels, 'Pid': 12345}))
    podman.write_text(f'''#!{sys.executable}
import json
from pathlib import Path
import sys
root = Path({str(root)!r})
args = sys.argv[1:]
with (root / 'cleanup-calls').open('a') as target:
    target.write(json.dumps(args) + '\\n')
absent = (root / 'absent').exists()
if args[0] == 'ps':
    if not absent:
        print((root / 'sweep').read_text() if (root / 'sweep').exists() else {'c' * 64!r})
elif args[:2] == ['container', 'exists']:
    sys.exit(1 if absent else 0)
elif args[:2] == ['container', 'inspect']:
    if any('{{{{json .Id}}}}' in argument for argument in args):
        sys.exit(125)
    print((root / 'metadata').read_text())
elif args[0] == 'rm':
    (root / 'absent').touch()
    cg = root / 'cgroups' / 'user.slice' / 'owned.scope'
    for name in ('cgroup.events', 'cgroup.procs'):
        (cg / name).unlink(missing_ok=True)
    if cg.exists():
        cg.rmdir()
    (root / 'properties').write_text('LoadState=not-found\\nActiveState=inactive\\nInvocationID=\\nControlGroup=\\n')
else:
    sys.exit(125)
''')
    podman.chmod(0o700)
    systemctl = root / 'systemctl'
    systemctl.write_text(f'#!{sys.executable}\nfrom pathlib import Path\nprint(Path({str(properties)!r}).read_text(), end="")\n')
    systemctl.chmod(0o700)
    child = subprocess.Popen([sys.executable, '-c', 'pass'], start_new_session=True)
    child.wait(timeout=10)
    ownership = {'run': {'backend': 'flux', 'uri': 'local:///native', 'instance': 'run/generation', 'job': 'fJob'},
                 'owner': {'owner': 'owner', 'correlation': 'correlation'}, 'request_digest': 'a' * 64,
                 'host': 'dl380g10', 'rank': 0, 'launcher': {'pid': child.pid, 'start_ticks': 1},
                 'process_group': child.pid, 'container': {'mode': 'own_scope', 'id': container_id, 'name': 'tl3-owned', 'labels': labels,
                     'scope': {'unit': 'owned.scope', 'invocation_id': 'd' * 32, 'cgroup': '/user.slice/owned.scope'}}}
    return root, str(podman), str(systemctl), ownership


def test_independent_cleanup_observes_exact_owned_scope_after_launcher_loss(cleanup_fixture):
    from tl3_harness import runtime

    root, podman, systemctl, owned = cleanup_fixture
    for _ in range(2):
        facts = runtime.cleanup_observations(owned, podman=podman, systemctl=systemctl,
                                             cgroup_root=root / 'cgroups', timeout=2)
        assert facts['schema'] == 'tl3.cleanup_evidence.v1'
        assert facts['ownership'] == owned
        assert facts['launcher'] == {'kind': 'absent'}
        assert facts['process_group_probe_errno'] == 3
        assert facts['container']['id'] == owned['container']['id']
        assert facts['container']['remove_exit'] == 0 and facts['container']['exists_exit'] == 1
        assert facts['container']['scope'] == {'identity': owned['container']['scope'],
            'observed_invocation_id': None, 'cgroup_events': None, 'cgroup_procs': '', 'active_state': 'not-found'}


@pytest.mark.parametrize('defect', ['labels', 'scope_generation', 'sweep'])
def test_independent_cleanup_refuses_ambiguous_or_replaced_ownership(cleanup_fixture, defect):
    from tl3_harness import runtime

    root, podman, systemctl, owned = cleanup_fixture
    if defect == 'labels':
        (root / 'metadata').write_text(json.dumps({'Id': owned['container']['id'], 'Labels': {'tl3.owner': 'foreign'}}))
    elif defect == 'scope_generation':
        (root / 'properties').write_text('LoadState=loaded\nActiveState=active\nInvocationID=' + 'e' * 32 + '\nControlGroup=/user.slice/owned.scope\n')
    else:
        (root / 'sweep').write_text('f' * 64)
    with pytest.raises(RuntimeError):
        runtime.cleanup_observations(owned, podman=podman, systemctl=systemctl,
                                     cgroup_root=root / 'cgroups', timeout=2)
    calls = [json.loads(line) for line in (root / 'cleanup-calls').read_text().splitlines()]
    assert not any(command[0] == 'rm' for command in calls)


def test_in_job_refuses_missing_runtime_prerequisites_before_launch(tmp_path, monkeypatch):
    from tl3_harness import runtime

    monkeypatch.setattr(runtime, 'check_rootless_directory', lambda environment: None)
    monkeypatch.setattr(runtime, 'launch_context', lambda: {'runtime': {'mode': 'in_job'}})
    monkeypatch.setenv('PATH', str(tmp_path))
    with pytest.raises(RuntimeError, match='crun and catatonit'):
        runtime.command(tmp_path / 'checkout', tmp_path / 'private')
    assert list(tmp_path.iterdir()) == []


@pytest.mark.parametrize('entrypoint', ['host-collector', 'host-cleanup'])
def test_published_host_entry_verifies_imported_bytes_before_any_operation(tmp_path, entrypoint):
    import subprocess
    from tl3_harness import runtime

    destination = tmp_path / 'artifact'
    executable, manifest, trusted_root = runtime.stage_host_runtime(destination, interpreter=sys.executable, producer_commit='a' * 40,
                                                                    podman='/explicit/podman', systemctl='/explicit/systemctl')
    target = trusted_root / 'runtime.py'
    target.chmod(0o600)
    target.write_text('raise RuntimeError("untrusted import executed")\n')
    completed = subprocess.run([sys.executable, '-I', str(executable.parent / entrypoint), '--help'], capture_output=True, timeout=10)
    assert completed.returncode != 0
    assert b'trusted runtime' in completed.stderr
    assert b'untrusted import executed' not in completed.stderr
    assert executable.stat().st_mode & 0o777 == 0o600
    assert manifest.stat().st_mode & 0o777 == 0o600
    assert executable.parent.stat().st_mode & 0o777 == 0o700


def test_host_collector_preserves_near_budget_complete_refusal_and_independent_stderr(tmp_path, monkeypatch):
    from tl3_harness import runtime

    report = {'schema': 'tl3.checks.v3', 'effect_key': 'fixture', 'base': 'b' * 40, 'head': 'c' * 40,
              'reviewed': None, 'results': [{'name': 'complete refusal', 'kind': 'demonstrating',
              'verdict': 'infrastructure-failure', 'custody': 'in-process', 'tests': [
                  {'test': f'test_{index}', 'base': 'uncollected', 'head': 'uncollected',
                   'pre_existing': False, 'passed': False, 'message': 'summary',
                   'proof_refusal': 'complete proof refusal ' * 2800} for index in range(129)]}]}
    payload = json.dumps(report, separators=(',', ':')).encode()
    assert 8 * 1024 * 1024 - 128 * 1024 < len(payload) < 8 * 1024 * 1024
    raw = b'TL3_CHECKS ' + payload + b'\n'
    source = tmp_path / 'input'
    source.write_bytes(raw)
    bounds = {'stdout_bytes': 9 * 1024 * 1024 + 12, 'stderr_bytes': 1024 * 1024}
    monkeypatch.setattr(runtime, 'launch_context', lambda: {'output_dir': str(tmp_path), 'run': {},
                        'owner': 'owner', 'correlation': 'correlation', 'output_bounds': bounds})
    requests = []
    monkeypatch.setattr(runtime, 'backend_request', lambda request: requests.append(request) or
                        ({'stopped': False} if request['op'] == 'cleanup_runtime' else {'report': 'rust-owned'}))
    code, _ = runtime.collect_host_payload([sys.executable, '-I', '-c',
        'import pathlib,sys;sys.stdout.buffer.write(pathlib.Path(sys.argv[1]).read_bytes());'
        'sys.stderr.buffer.write(b"x"*1048576)', str(source)], timeout=15)
    assert code == 0
    assert (tmp_path / 'host.stdout').read_bytes() == raw
    assert (tmp_path / 'host.stderr').stat().st_size == bounds['stderr_bytes']
    facts = next(request['facts'] for request in requests if request['op'] == 'collect_runner')
    assert facts['stderr']['bytes'] == bounds['stderr_bytes']
    assert json.loads((tmp_path / 'host.stdout').read_bytes()[len(b'TL3_CHECKS '):]) == report


@pytest.mark.parametrize('operation', ['collect_runner', 'collect_worker'])
def test_collector_uses_rust_selected_stream_bounds_and_operation(tmp_path, monkeypatch, operation):
    from tl3_harness import runtime

    monkeypatch.setattr(runtime, 'launch_context', lambda: {'output_dir': str(tmp_path), 'run': {},
                        'owner': 'owner', 'correlation': 'correlation',
                        'output_bounds': {'stdout_bytes': 13, 'stderr_bytes': 17}})
    requests = []
    monkeypatch.setattr(runtime, 'backend_request', lambda request: requests.append(request) or {'stopped': False})
    code, _ = runtime.collect_host_payload([sys.executable, '-I', '-c',
        'import sys;sys.stdout.buffer.write(b"a"*13);sys.stderr.buffer.write(b"b"*17)'],
        timeout=10, operation=operation)
    assert code == 0
    assert requests[-1]['op'] == operation
    assert requests[-1]['facts']['stdout']['bytes'] == 13
    assert requests[-1]['facts']['stderr']['bytes'] == 17


@pytest.mark.parametrize('stream', ['stdout', 'stderr'])
def test_collector_refuses_independent_stream_overflow(tmp_path, monkeypatch, stream):
    from tl3_harness import runtime

    monkeypatch.setattr(runtime, 'launch_context', lambda: {'output_dir': str(tmp_path), 'run': {},
                        'owner': 'owner', 'correlation': 'correlation',
                        'output_bounds': {'stdout_bytes': 31, 'stderr_bytes': 31}})
    requests = []
    monkeypatch.setattr(runtime, 'backend_request', lambda request: requests.append(request) or {'stopped': False})
    with pytest.raises(RuntimeError, match=stream):
        runtime.collect_host_payload([sys.executable, '-I', '-c',
            f'import sys;sys.{stream}.buffer.write(b"a"*32)'], timeout=10)
    assert not any(request['op'].startswith('collect_') for request in requests)


@pytest.mark.parametrize('allowed', [True, False])
def test_durable_launch_precedes_podman_and_binding_gates_payload(launcher, monkeypatch, allowed):
    from tl3_harness import runtime
    from tl3_harness.runtime_evidence import PREFIX

    executable, calls = launcher
    output = executable.parent / 'output'
    output.mkdir()
    context = {'run': {'backend': 'flux', 'uri': 'local:///fixture', 'instance': 'run/generation', 'job': 'fJob'},
               'owner': 'owner', 'correlation': 'correlation', 'request_digest': 'f' * 64,
               'output_dir': str(output), 'progress': {'no_progress_seconds': 10},
               'placement': {'kind': 'observed', 'host': 'dl380g10', 'rank': 0,
                             'allocation': {}, 'topology': {}, 'os_pus': [1]},
               'runtime': {'mode': 'in_job'}}
    (executable.parent / 'stderr').write_text(PREFIX + json.dumps({'Cpus_allowed_list': '1', 'cgroup': '0::/job'}) + '\n')
    monkeypatch.setattr(runtime, 'launch_context', lambda: context)
    operations = []
    def backend(request):
        op = request['op']
        operations.append(op)
        if op == 'begin_container_launch':
            assert not calls.exists(), 'Podman must remain gated until begin returns'
            launch = {'launcher': request['launcher'], 'name': 'tl3-exact-fixture',
                      'mode': 'in_job', 'labels': {'tl3.execution': request['launcher']['execution']}}
            (executable.parent / 'launch.json').write_text(json.dumps(launch))
            return {'launch': launch}
        if op == 'bind_container_launch':
            assert not (executable.parent / 'payload').exists()
            assert request['observed']['container']['name'] == 'tl3-exact-fixture'
            return {'binding': {'payload_allowed': allowed, 'ownership': request['observed']}}
        if op == 'validate_runtime_binding':
            assert 'bind_container_launch' in operations
            assert request['facts']['container_id'] == 'a' * 64
            assert request['facts']['execution']
            return {'qualification': {'qualified': True}}
        if op == 'cleanup_runtime':
            return {'cleanup': {'stopped': True}}
        raise AssertionError(op)
    monkeypatch.setattr(runtime, 'backend_request', backend)
    with calls.with_suffix('.stdin').open('w+b') as selection:
        selection.write(b'["private-test"]')
        selection.seek(0)
        if allowed:
            with Supervised([str(executable), 'run', 'fixture'], selection, ()) as process:
                list(process.records())
            assert (executable.parent / 'payload').read_bytes() == b'["private-test"]'
            assert operations.index('bind_container_launch') < operations.index('validate_runtime_binding')
        else:
            with pytest.raises(ExecutionFailure):
                with Supervised([str(executable), 'run', 'fixture'], selection, ()) as process:
                    list(process.records())
            assert not (executable.parent / 'payload').exists()
            assert 'validate_runtime_binding' not in operations
    assert operations[0] == 'begin_container_launch'
    assert operations[-1] == 'cleanup_runtime'


def test_pending_launch_without_observed_identity_never_certifies_cleanup(cleanup_fixture):
    from tl3_harness import runtime

    root, podman, systemctl, owned = cleanup_fixture
    launcher = {**owned, 'container': None}
    launch = {'launcher': launcher, 'name': 'tl3-pending', 'mode': 'own_scope', 'labels': owned['container']['labels']}
    (root / 'absent').touch()
    with pytest.raises(RuntimeError, match='unbound'):
        runtime.cleanup_observations(launcher, launch=launch, podman=podman, systemctl=systemctl,
                                     cgroup_root=root / 'cgroups', timeout=2)
    assert not any(json.loads(line)[0] == 'rm' for line in (root / 'cleanup-calls').read_text().splitlines())


def test_pending_launch_recovers_exact_observed_identity_for_rust_reconciliation(cleanup_fixture, monkeypatch):
    from tl3_harness import runtime

    root, podman, systemctl, owned = cleanup_fixture
    launcher = {**owned, 'container': None}
    launch = {'launcher': launcher, 'name': owned['container']['name'], 'mode': 'own_scope',
              'labels': owned['container']['labels']}
    monkeypatch.setattr(runtime, 'scope_facts', lambda *args, **kwargs: (owned['container']['scope'], {}))
    result = runtime.cleanup_observations(launcher, launch=launch, podman=podman, systemctl=systemctl,
                                          cgroup_root=root / 'cgroups', timeout=2)
    assert result['ownership'] == owned
    assert result['container']['name_exists_exit'] == 1
    assert result['container']['sweep_exit'] == 0
    assert result['container']['remaining_ids'] == []
