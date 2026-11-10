"""Controlled executable fixtures exercise the real launcher and cleanup boundary."""
import json
import signal
from pathlib import Path
import sys

import pytest

from tl3_harness.supervision import ExecutionFailure, ExecutionOutcome, Supervised
from conftest import flux_context


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
    conmon_path = pathlib.Path({str(tmp_path / 'conmon-pid')!r})
    conmon = int(conmon_path.read_text()) if conmon_path.exists() else pid
    print(json.dumps({{'Id': 'a' * 64, 'Pid': pid, 'ConmonPid': conmon,
                      'Name': launch.get('name'), 'Labels': launch.get('labels')}}))
    sys.exit(0)
if args[:2] == ['container', 'exists']:
    sys.exit(int(pathlib.Path({str(tmp_path / 'exists')!r}).read_text()))
sys.exit(0)
''')
    runtime.chmod(0o700)
    (tmp_path / 'exists').write_text('1')
    monkeypatch.setenv('PATH', str(tmp_path))
    flux_context(monkeypatch, no_progress_seconds=10)
    return runtime, calls


def test_flux_in_job_command_keeps_every_containment_flag(tmp_path):
    from tl3_harness.runtime import podman_command

    scratch = tmp_path / 'private'
    scratch.mkdir(mode=0o700)
    arguments = podman_command(tmp_path / 'checkout', 'sha256:' + '1' * 64, scratch, 1, mode='in_job')[1:]
    for flag in ('--cgroups=disabled', '--cgroupns=host', '--network=none', '--read-only', '--init',
                 '--pull=never', '--cap-drop=ALL', '--security-opt=no-new-privileges', '--ipc=private',
                 '--read-only-tmpfs=false', '--pid=private', '--http-proxy=false'):
        assert flag in arguments
    mounts = [arguments[i + 1] for i, flag in enumerate(arguments) if flag == '--mount']
    assert len(mounts) == 3
    assert len([mount for mount in mounts if mount.endswith(',readonly')]) == 2
    assert f'type=bind,src={scratch},dst=/tmp' in mounts
    assert not any('flux-out' in mount or 'token' in mount for mount in mounts)
    assert '--cpuset-cpus' not in arguments
    assert not any(arg.startswith(('--userns', '--user=', '--memory')) for arg in arguments)
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


def test_backend_selection_is_flux_only_and_fail_closed(monkeypatch):
    """PB-REMOVAL: Flux is the only executor. A missing, retired or unknown backend is
    refused before any launch; nothing falls back to a PrismaBuild action."""
    from tl3_harness import runtime

    monkeypatch.setenv('FLUX_JOB_ID', 'nested-native-job')
    monkeypatch.delenv('TL3_EXECUTION_BACKEND', raising=False)
    for backend in (None, 'prismabuild', 'misspelled', ''):
        if backend is not None:
            monkeypatch.setenv('TL3_EXECUTION_BACKEND', backend)
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


@pytest.fixture
def cleanup_cli(backend_cli):
    reply, calls = backend_cli
    executable = reply.parent / 'backend-cli'
    executable.write_text(f'''#!{sys.executable}
import json, pathlib, sys, time
command = json.load(sys.stdin)
pathlib.Path({str(calls)!r}).write_text(json.dumps(command))
def frame(sequence, kind, **body):
    print(json.dumps(dict(schema='tl3.run_backend.frame.v1', sequence=sequence, kind=kind, **body)), flush=True)
if command['op'] == 'cleanup_limits':
    frame(0, 'result', result=dict(op='cleanup_limits', limits=dict(helper_budget_seconds=1,
          native_timeout_seconds=2, idle_timeout_seconds=3, max_progress_frames=4,
          max_frame_bytes=4096, max_response_bytes=16384)))
elif command['op'] == 'launch_context':
    frame(0, 'result', result=dict(op='launch_context', context=dict(owner='owner', correlation='correlation',
          run=dict(backend='flux', uri='local:///native', instance='run/generation', job='fJob'), request_digest='a'*64)))
else:
    for delay, value in json.loads(pathlib.Path({str(reply)!r}).read_text()):
        time.sleep(delay)
        print(json.dumps(value), flush=True)
''')
    return reply


def cleanup_frames(operation, *, delay=0):
    progress = dict(owner=dict(owner='owner', correlation='correlation'),
                    run=dict(backend='flux', uri='local:///native', instance='run/generation', job='fJob'),
                    request_digest='a' * 64, execution='first', helper_budget_seconds=1, idle_timeout_seconds=3)
    first = dict(schema='tl3.run_backend.frame.v1', sequence=0, kind='cleanup_progress', progress=progress)
    second = {**first, 'sequence': 1, 'progress': {**progress, 'execution': 'second'}}
    final = dict(schema='tl3.run_backend.frame.v1', sequence=2, kind='result', result=dict(op=operation, value='final'))
    return [[0, first], [delay, second], [delay, final]]


@pytest.mark.parametrize('operation', ['cleanup_runtime', 'cancel'])
def test_cleanup_progress_renews_only_the_idle_deadline(cleanup_cli, operation):
    from tl3_harness import runtime

    cleanup_cli.write_text(json.dumps(cleanup_frames(operation, delay=2)))
    result = runtime.backend_request(dict(op=operation, owner='owner', correlation='correlation'))
    assert result == dict(op=operation, value='final')


def test_cleanup_progress_silence_remains_bounded(cleanup_cli):
    from tl3_harness import runtime

    frames = cleanup_frames('cleanup_runtime')
    frames[1][0] = 4
    cleanup_cli.write_text(json.dumps(frames))
    with pytest.raises(RuntimeError, match='timed out'):
        runtime.backend_request(dict(op='cleanup_runtime', owner='owner', correlation='correlation'))


@pytest.mark.parametrize('fault', ['owner', 'run', 'request_digest', 'helper_budget_seconds',
                                'idle_timeout_seconds', 'sequence', 'duplicate_execution',
                                'progress_after_result', 'duplicate_result', 'frame_bytes', 'count'])
def test_cleanup_progress_refuses_wrong_binding_or_order(cleanup_cli, fault):
    from tl3_harness import runtime

    frames = cleanup_frames('cleanup_runtime')
    if fault == 'owner':
        frames[0][1]['progress']['owner']['owner'] = 'other'
    elif fault == 'run':
        frames[0][1]['progress']['run']['job'] = 'other'
    elif fault in ('request_digest', 'helper_budget_seconds', 'idle_timeout_seconds'):
        frames[0][1]['progress'][fault] = 'wrong'
    elif fault == 'sequence':
        frames[1][1]['sequence'] = 3
    elif fault == 'duplicate_execution':
        frames[1][1]['progress']['execution'] = 'first'
    elif fault == 'progress_after_result':
        frames = [frames[2], frames[0]]
        for i, (_, frame) in enumerate(frames):
            frame['sequence'] = i
    elif fault == 'duplicate_result':
        frames.append([0, {**frames[-1][1], 'sequence': 3}])
    elif fault == 'frame_bytes':
        frames[0][1]['progress']['execution'] = 'a' * 4096
    else:
        frames = [[0, {**frames[0][1], 'sequence': i,
                       'progress': {**frames[0][1]['progress'], 'execution': str(i)}}] for i in range(5)]
    cleanup_cli.write_text(json.dumps(frames))
    with pytest.raises(RuntimeError):
        runtime.backend_request(dict(op='cleanup_runtime', owner='owner', correlation='correlation'))


def test_cleanup_main_consumes_rust_budget(monkeypatch, tmp_path, capsys):
    import io
    from tl3_harness import runtime

    monkeypatch.setattr(runtime, 'check_rootless_directory', lambda _: None)
    monkeypatch.setattr(sys, 'argv', ['cleanup', '--podman', '/explicit/podman', '--systemctl', '/explicit/systemctl'])
    monkeypatch.setattr(sys, 'stdin', type('Input', (), {'buffer': io.BytesIO(json.dumps(dict(
        schema='tl3.cleanup_input.v1', budget_seconds=7, ownership={'fixture': True}, launch=None)).encode())})())
    observed = []
    monkeypatch.setattr(runtime, 'cleanup_observations', lambda ownership, **options: observed.append(options) or {'fixture': True})
    runtime.cleanup_main()
    assert observed == [dict(launch=None, podman='/explicit/podman', systemctl='/explicit/systemctl', timeout=7)]
    assert json.loads(capsys.readouterr().out) == {'fixture': True}


def test_own_scope_command_keeps_mapped_grant_and_has_no_in_job_fallback(tmp_path):
    from tl3_harness.runtime import podman_command

    arguments = podman_command(tmp_path / 'checkout', 'sha256:' + '1' * 64, tmp_path / 'private', 1,
                               mode='own_scope', cpus='21-23,30,61-63,70', memory=268435456)[1:]
    assert arguments[:2] == ['--cgroup-manager=systemd', 'run']
    assert '--cgroups=enabled' in arguments
    assert '--init' not in arguments and '--cgroups=disabled' not in arguments
    assert arguments[arguments.index('--cpuset-cpus') + 1] == '21-23,30,61-63,70'
    assert arguments[arguments.index('--memory') + 1] == '268435456'
    assert arguments[arguments.index('--memory-swap') + 1] == '268435456'
    assert '--pull=never' in arguments


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
    context = {'output_dir': str(output), 'run': run, 'owner': 'owner', 'correlation': 'correlation',
               'output_bounds': {'stdout_bytes': 1024, 'stderr_bytes': 1024}}
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
    assert requests[0]['op'] == 'cleanup_runtime'
    assert requests[1:] == [{'op': 'collect_runner', 'owner': 'owner', 'correlation': 'correlation',
                         'facts': {'run': run, 'stdout': {'path': str(output / 'host.stdout'),
                                   'sha256': hashlib.sha256(raw).hexdigest(), 'bytes': len(raw)},
                                   'stderr': {'path': str(output / 'host.stderr'), 'bytes': 10,
                                   'sha256': hashlib.sha256(b'diagnostic').hexdigest()},
                                   'runtime_binding': None, 'runtime_bindings': [], 'cleanup': None}}]


def test_host_collector_never_overwrites_prior_output(tmp_path, monkeypatch):
    from tl3_harness import runtime

    (tmp_path / 'host.stdout').write_text('retained')
    monkeypatch.setattr(runtime, 'launch_context', lambda: {'output_dir': str(tmp_path),
                        'output_bounds': {'stdout_bytes': 1024, 'stderr_bytes': 1024}})
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


@pytest.mark.parametrize(('allowed', 'live_conmon'), [(True, False), (False, False), (True, True)],
                         ids=['allowed', 'refused', 'survivor'])
def test_durable_launch_precedes_podman_and_binding_gates_payload(launcher, monkeypatch, allowed, live_conmon):
    import os
    import subprocess

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
    # Allocation is exercised with the real Rust CLI by fx_action_e2e.py.
    monkeypatch.setattr(runtime, 'prepare_allocation', lambda evidence: None)
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
    survivor = None
    original_record = runtime.Evidence.record
    def record(evidence, event, facts):
        if event == 'container_processes_stopped':
            for handle in process.process_handles:
                os.fstat(handle)  # Evidence must be retained while these identities are still held.
        original_record(evidence, event, facts)
    monkeypatch.setattr(runtime.Evidence, 'record', record)
    try:
        if live_conmon:
            survivor = subprocess.Popen([sys.executable, '-I', '-c',
                'import sys; print("ready", flush=True); sys.stdin.buffer.read()'],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
            assert survivor.stdout.readline() == b'ready\n'
            (executable.parent / 'conmon-pid').write_text(str(survivor.pid))
        with calls.with_suffix('.stdin').open('w+b') as selection:
            selection.write(b'["private-test"]')
            selection.seek(0)
            if allowed and not live_conmon:
                with Supervised([str(executable), 'run', 'fixture'], selection, ()) as process:
                    list(process.records())
                assert process.cleanup_confirmed
            else:
                with pytest.raises(ExecutionFailure) as failed:
                    with Supervised([str(executable), 'run', 'fixture'], selection, ()) as process:
                        list(process.records())
                if live_conmon:
                    assert failed.value.termination_reason == 'container_cleanup_unconfirmed'
                    assert not process.cleanup_confirmed
                    assert survivor.poll() is None
        if allowed:
            assert (executable.parent / 'payload').read_bytes() == b'["private-test"]'
            assert operations.index('bind_container_launch') < operations.index('validate_runtime_binding')
            records = [json.loads(line) for line in process.evidence.path.read_text().splitlines()]
            stopped = next(row for row in records if row['event'] == 'container_processes_stopped')
            assert stopped['confirmed'] is not live_conmon
            assert stopped['captured'] == 2
            assert stopped['finished_monotonic_ns'] >= stopped['started_monotonic_ns']
            for handle in process.process_handles:
                with pytest.raises(OSError):
                    os.fstat(handle)
        else:
            assert not (executable.parent / 'payload').exists()
            assert 'validate_runtime_binding' not in operations
        assert operations[0] == 'begin_container_launch'
        assert operations[-1] == 'cleanup_runtime'
    finally:
        if survivor is not None:
            survivor.kill()
            survivor.wait(timeout=5)
            survivor.stdin.close()
            survivor.stdout.close()


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
    monkeypatch.setattr(runtime, 'scope_identity', lambda *args, **kwargs: owned['container']['scope'])
    result = runtime.cleanup_observations(launcher, launch=launch, podman=podman, systemctl=systemctl,
                                          cgroup_root=root / 'cgroups', timeout=2)
    assert result['ownership'] == owned
    assert result['container']['name_exists_exit'] == 1
    assert result['container']['sweep_exit'] == 0
    assert result['container']['remaining_ids'] == []


def test_cleanup_query_failure_still_stops_exact_retained_launcher(cleanup_fixture):
    import subprocess
    from tl3_harness import runtime

    root, podman, systemctl, owned = cleanup_fixture
    child = subprocess.Popen([sys.executable, '-I', '-c', 'import time;time.sleep(60)'], start_new_session=True)
    observation = runtime._process_observation({'pid': child.pid, 'start_ticks': 0})
    owned = {**owned, 'launcher': {'pid': child.pid, 'start_ticks': observation['observed_start_ticks']},
             'process_group': child.pid}
    (root / 'sweep').write_text('f' * 64)
    try:
        with pytest.raises(RuntimeError, match='sweep'):
            runtime.cleanup_observations(owned, podman=podman, systemctl=systemctl,
                                         cgroup_root=root / 'cgroups', timeout=1)
        assert child.wait(timeout=5) < 0
    finally:
        if child.poll() is None:
            child.kill()
        child.wait(timeout=5)


def test_cleanup_stops_launcher_before_any_podman_query(cleanup_fixture, monkeypatch):
    import os
    import subprocess
    from tl3_harness import runtime

    root, podman, systemctl, owned = cleanup_fixture
    child = subprocess.Popen([sys.executable, '-I', '-c', 'import time;time.sleep(60)'], start_new_session=True)
    observation = runtime._process_observation({'pid': child.pid, 'start_ticks': 0})
    owned = {**owned, 'launcher': {'pid': child.pid, 'start_ticks': observation['observed_start_ticks']},
             'process_group': child.pid}
    handle = os.pidfd_open(child.pid)
    original_run = subprocess.run

    def observe(arguments, **options):
        assert runtime.processes_stopped([handle], 0.01), 'launcher must stop before cleanup I/O'
        child.wait(timeout=1)
        return original_run(arguments, **options)

    monkeypatch.setattr(subprocess, 'run', observe)
    try:
        facts = runtime.cleanup_observations(owned, podman=podman, systemctl=systemctl,
                                             cgroup_root=root / 'cgroups', timeout=1)
        assert facts['container']['remove_exit'] == 0
        assert child.wait(timeout=5) < 0
    finally:
        os.close(handle)
        if child.poll() is None:
            child.kill()
        child.wait(timeout=5)


def test_cleanup_slow_podman_shares_one_budget_and_retains_incomplete_evidence(cleanup_fixture):
    import time
    from tl3_harness import runtime

    root, podman, systemctl, owned = cleanup_fixture
    executable = Path(podman)
    executable.write_text(executable.read_text().replace('import json\n', 'import json\nimport time\ntime.sleep(0.09)\n'))
    started = time.monotonic()
    facts = runtime.cleanup_observations(owned, podman=podman, systemctl=systemctl,
                                         cgroup_root=root / 'cgroups', timeout=0.35)
    assert time.monotonic() - started < 0.65, 'cleanup subprocesses must share the total budget'
    assert facts['schema'] == 'tl3.cleanup_evidence.v1'
    assert facts['launcher'] == {'kind': 'absent'}
    assert facts['process_group_probe_errno'] == 3
    assert facts['container'] is None, 'incomplete cleanup must not invent successful container evidence'


def test_raw_prepared_inventory_includes_explicit_runtime_tools(tmp_path):
    from tl3_harness import runtime

    collector, inventory, trusted = runtime.stage_host_runtime(tmp_path / 'raw', interpreter=sys.executable,
        producer_commit='a' * 40, podman='/explicit/podman', systemctl='/explicit/systemctl')
    assert json.loads((trusted / 'runtime-tools.json').read_bytes()) == {
        'podman': '/explicit/podman', 'systemctl': '/explicit/systemctl'}
    assert str((trusted / 'runtime-tools.json').relative_to(collector.parent)) in {
        entry['path'] for entry in json.loads(inventory.read_bytes())['files']}


def test_prelaunch_cancellation_never_executes_podman_and_reaps_gated_client(launcher, monkeypatch):
    import os
    from tl3_harness import runtime

    executable, calls = launcher
    output = executable.parent / 'output'
    output.mkdir()
    context = {'run': {}, 'owner': 'owner', 'correlation': 'correlation', 'request_digest': 'f' * 64,
               'output_dir': str(output), 'progress': {'no_progress_seconds': 10},
               'placement': {'kind': 'observed', 'host': 'dl380g10', 'rank': 0}}
    monkeypatch.setattr(runtime, 'launch_context', lambda: context)
    monkeypatch.setattr(runtime, 'prepare_allocation', lambda evidence: None)
    monkeypatch.setattr(runtime, '_uncertain_containers', set())
    launcher_pids = []
    def backend(request):
        assert request['op'] == 'begin_container_launch'
        launcher_pids.append(request['launcher']['launcher']['pid'])
        raise RuntimeError('launch held after cancellation')
    monkeypatch.setattr(runtime, 'backend_request', backend)
    with calls.with_suffix('.stdin').open('w+b') as selection:
        with pytest.raises(ExecutionFailure) as failed:
            with Supervised([str(executable), 'run', 'fixture'], selection, ()):
                pytest.fail('cancelled begin must never return an active launcher')
        assert failed.value.outcome == ExecutionOutcome.UNKNOWN_INCOMPLETE
        assert 'cancellation' in failed.value.stderr_tail
    assert not calls.exists()
    assert len(launcher_pids) == 1
    with pytest.raises(ChildProcessError):
        os.waitpid(launcher_pids[0], os.WNOHANG)


@pytest.mark.parametrize('fault', ['lost-response', 'missing-name', 'missing-labels', 'record-failed'])
def test_begin_response_failure_retains_unknown_and_owned_scratch(launcher, monkeypatch, fault):
    from tl3_harness import runtime, scratch

    executable, calls = launcher
    output = executable.parent / 'output'
    output.mkdir()
    root = executable.parent / 'scratch'
    root.mkdir(mode=0o700)
    context = {'run': {}, 'owner': 'owner', 'correlation': 'correlation', 'request_digest': 'f' * 64,
               'scratch': {'leaf': str(root)}, 'output_dir': str(output),
               'progress': {'no_progress_seconds': 10},
               'placement': {'kind': 'observed', 'host': 'dl380g10', 'rank': 0}}
    monkeypatch.setattr(runtime, 'launch_context', lambda: context)
    monkeypatch.setattr(runtime, 'prepare_allocation', lambda evidence: None)
    monkeypatch.setattr(runtime, '_uncertain_containers', set())
    persisted = output / 'persisted-intent.json'
    launch = {'name': 'tl3-durable-name', 'labels': {'tl3.execution': 'token'}}

    def backend(request):
        assert request['op'] == 'begin_container_launch'
        persisted.write_text(json.dumps(request))
        if fault == 'lost-response':
            raise RuntimeError('response lost after durable begin')
        return {'launch': {key: value for key, value in launch.items()
                           if key != {'missing-name': 'name', 'missing-labels': 'labels'}.get(fault)}}

    monkeypatch.setattr(runtime, 'backend_request', backend)
    original_record = runtime.Evidence.record

    def record(evidence, event, facts):
        if fault == 'record-failed' and event == 'launch_intent':
            raise OSError('evidence volume full after durable begin')
        return original_record(evidence, event, facts)

    monkeypatch.setattr(runtime.Evidence, 'record', record)
    with runtime.workspace(prefix='tl3-checks-') as workspace:
        temporary = scratch.private_directory(root)
        command = [str(executable), 'run', '--mount', f'type=bind,src={temporary},dst=/tmp', 'fixture']
        with calls.with_suffix('.stdin').open('w+b') as selection:
            process = Supervised(command, selection, ())
            with pytest.raises(ExecutionFailure) as failed:
                process.__enter__()
        assert failed.value.outcome == ExecutionOutcome.UNKNOWN_INCOMPLETE
        assert failed.value.stderr_tail
        assert not process.cleanup_confirmed
        assert process.evidence.container in runtime._uncertain_containers
        assert persisted.exists() and temporary.is_dir()
        assert process.child.returncode is not None
    assert Path(workspace).is_dir(), 'ambiguous begin must retain action-owned workspace'
    assert temporary.is_dir()
    assert not calls.exists(), 'gated launcher must receive no Podman command'


@pytest.mark.parametrize('fault', ['container-stopped', 'evidence-record'])
def test_refusal_secondary_failure_keeps_typed_cause(launcher, monkeypatch, fault):
    from tl3_harness import runtime

    executable, calls = launcher
    output = executable.parent / 'output'
    output.mkdir()
    context = {'run': {}, 'output_dir': str(output), 'progress': {'no_progress_seconds': 10}}
    monkeypatch.setattr(runtime, 'launch_context', lambda: context)
    monkeypatch.setattr(runtime, '_uncertain_containers', set())

    def refuse(evidence):
        raise RuntimeError('original placement refusal')

    monkeypatch.setattr(runtime, 'prepare_allocation', refuse)
    if fault == 'container-stopped':
        def stopped(*args):
            raise RuntimeError('secondary scratch identity changed')
        monkeypatch.setattr(runtime, 'container_stopped', stopped)
    else:
        original_record = runtime.Evidence.record
        def record(evidence, event, facts):
            if event == 'prelaunch_refusal':
                raise OSError('secondary evidence volume full')
            return original_record(evidence, event, facts)
        monkeypatch.setattr(runtime.Evidence, 'record', record)
    with calls.with_suffix('.stdin').open('w+b') as selection:
        with pytest.raises(ExecutionFailure) as failed:
            Supervised([str(executable), 'run', 'fixture'], selection, ()).__enter__()
    assert failed.value.outcome == ExecutionOutcome.SETUP_FAILURE
    assert 'original placement refusal' in failed.value.stderr_tail
    assert 'secondary' in failed.value.stderr_tail
    assert not calls.exists()


def test_begin_local_observation_failure_is_no_effect_and_removes_job_root(launcher, monkeypatch):
    from tl3_harness import runtime, scratch

    executable, calls = launcher
    output = executable.parent / 'output'
    output.mkdir()
    root = executable.parent / 'scratch'
    root.mkdir(mode=0o700)
    context = {'run': {}, 'owner': 'owner', 'correlation': 'correlation', 'request_digest': 'f' * 64,
               'scratch': {'leaf': str(root)}, 'output_dir': str(output),
               'progress': {'no_progress_seconds': 10},
               'placement': {'kind': 'observed', 'host': 'dl380g10', 'rank': 0}}
    monkeypatch.setattr(runtime, 'launch_context', lambda: context)
    monkeypatch.setattr(runtime, 'prepare_allocation', lambda evidence: None)
    monkeypatch.setattr(runtime, '_uncertain_containers', set())

    def observe(*args):
        raise OSError('local launcher observation unavailable')

    monkeypatch.setattr(runtime, '_process_observation', observe)
    monkeypatch.setattr(runtime, 'backend_request', lambda *args, **kwargs: pytest.fail('begin must not be dispatched'))
    with runtime.workspace(prefix='tl3-checks-'):
        temporary = scratch.private_directory(root)
        command = [str(executable), 'run', '--mount', f'type=bind,src={temporary},dst=/tmp', 'fixture']
        with calls.with_suffix('.stdin').open('w+b') as selection:
            process = Supervised(command, selection, ())
            with pytest.raises(ExecutionFailure) as failed:
                process.__enter__()
        assert failed.value.outcome == ExecutionOutcome.SETUP_FAILURE
        assert 'local launcher observation unavailable' in failed.value.stderr_tail
        assert process.cleanup_confirmed and not process.evidence.launch_requested
        assert not runtime._uncertain_containers and not temporary.exists()
        assert process.child.returncode is not None
    assert not root.exists()
    assert not calls.exists()


@pytest.fixture
def session_cli(tmp_path, monkeypatch):
    from tl3_harness import runtime
    executable = tmp_path / 'session-cli'
    control = tmp_path / 'session-mode'
    config = tmp_path / 'session-config'
    config.write_text('{}')
    control.write_text('ok')
    executable.write_text(f'''#!{sys.executable}
import json, os, pathlib, sys, time
for line in sys.stdin:
    request = json.loads(line)
    command = request['command']
    mode = pathlib.Path({str(control)!r}).read_text()
    if mode == 'silent':
        time.sleep(30)
    if mode == 'pipe-holder':
        child = os.fork()
        if child == 0:
            time.sleep(30)
            os._exit(0)
        pathlib.Path({str(control)!r} + '.pid').write_text(str(child))
        os._exit(7)
    if mode == 'closed-output':
        os.close(1)
        os.close(2)
        time.sleep(30)
    if mode == 'eof':
        sys.exit(7)
    frame = {{'schema':'tl3.run_backend.frame.v1', 'sequence':0,
              'request_id':request['request_id'], 'kind':'result',
              'result':{{'op':command['op'], 'pid':os.getpid()}}}}
    if mode == 'wrong-id': frame['request_id'] -= 1
    if mode == 'wrong-sequence': frame['sequence'] = 1
    if mode == 'oversize': frame['result']['data'] = 'x' * 10000
    if mode == 'error':
        frame = {{**frame, 'kind':'error', 'code':'invalid_request','message':'retained refusal'}}
        del frame['result']
        print('CLI stderr diagnostic', file=sys.stderr, flush=True)
    print(json.dumps(frame), flush=True)
    if mode == 'error': sys.exit(7)
''')
    executable.chmod(0o700)
    monkeypatch.setenv('TL3_RUN_BACKEND_CLI', str(executable))
    monkeypatch.setenv('TL3_RUN_BACKEND_CONFIG', str(config))
    monkeypatch.setenv('TL3_RUN_BACKEND_SESSION', '1')
    monkeypatch.setattr(runtime, 'BACKEND_IDLE_SECONDS', 0.5)
    runtime.close_backend_session()
    try:
        yield control, config
    finally:
        runtime.close_backend_session()


def test_session_owns_one_action_config_and_reaps_on_close(session_cli):
    import os
    from tl3_harness import runtime

    _, config = session_cli
    command = {'op': 'launch_context', 'owner': 'first', 'correlation': 'one'}
    first = runtime.backend_request(command)['pid']
    assert runtime.backend_request(command)['pid'] == first
    config.write_text('{"generation":"changed"}')
    second = runtime.backend_request(command)['pid']
    assert second != first
    with pytest.raises(ChildProcessError):
        os.waitpid(first, os.WNOHANG)
    third = runtime.backend_request({**command, 'owner': 'other'})['pid']
    assert third != second
    runtime.close_backend_session()
    with pytest.raises(ChildProcessError):
        os.waitpid(third, os.WNOHANG)


def test_session_fork_closes_copies_without_stopping_parent(session_cli):
    import os
    from tl3_harness import runtime

    command = {'op': 'launch_context', 'owner': 'first', 'correlation': 'one'}
    original = runtime.backend_request(command)['pid']
    child = os.fork()
    if child == 0:
        try:
            assert runtime._backend_session is None
            assert runtime.backend_request(command)['pid'] != original
            runtime.close_backend_session()
        except BaseException:
            os._exit(1)
        os._exit(0)
    assert os.waitpid(child, 0)[1] == 0
    assert runtime.backend_request(command)['pid'] == original


@pytest.mark.parametrize('mode', ['wrong-id', 'wrong-sequence', 'oversize', 'silent', 'eof', 'error', 'pipe-holder', 'closed-output'])
def test_session_refuses_bad_frames_eof_and_timeout_and_reaps(session_cli, mode):
    import os
    import time
    from tl3_harness import runtime

    control, _ = session_cli
    command = {'op': 'launch_context', 'owner': 'first', 'correlation': 'one'}
    pid = runtime.backend_request(command)['pid']
    control.write_text(mode)
    started = time.monotonic()
    with pytest.raises(runtime.BackendCliError) as failed:
        runtime.backend_request(command, bounds={'cli_result_bytes': 2048})
    assert time.monotonic() - started < 2
    assert runtime._backend_session is None
    with pytest.raises(ChildProcessError):
        os.waitpid(pid, os.WNOHANG)
    if mode == 'pipe-holder':
        assert not Path('/proc', control.with_suffix('.pid').read_text()).exists()
    if mode == 'error':
        diagnostic = failed.value.diagnostic
        assert diagnostic['status']['code'] == 7
        assert 'CLI stderr diagnostic' in diagnostic['stderr']['text']
        assert diagnostic['cause']['message'] == 'retained refusal'


def test_host_collector_reaps_pipe_holder_when_action_exits(tmp_path, monkeypatch):
    import time
    from tl3_harness import runtime

    monkeypatch.setattr(runtime, 'launch_context', lambda: {'output_dir': str(tmp_path), 'run': {},
        'owner': 'owner', 'correlation': 'correlation', 'output_bounds': {'stdout_bytes': 1024, 'stderr_bytes': 1024}})
    monkeypatch.setattr(runtime, 'backend_request', lambda command: {})
    child_file = tmp_path / 'child-pid'
    script = ('import os,subprocess,sys; child=subprocess.Popen([sys.executable,"-c","import time;time.sleep(30)"]);'
              f'open({str(child_file)!r},"w").write(str(child.pid)); os._exit(7)')
    started = time.monotonic()
    code, _ = runtime.collect_host_payload([sys.executable, '-I', '-c', script], timeout=20)
    assert code == 7 and time.monotonic() - started < 2
    pid = int(child_file.read_text())
    status = Path(f'/proc/{pid}/stat')
    assert not status.exists()


def test_pending_injob_absent_name_and_empty_labels_retains_typed_evidence(cleanup_fixture):
    from tl3_harness import runtime

    root, podman, systemctl, owned = cleanup_fixture
    (root / 'absent').touch()
    launcher = {**owned, 'container': None}
    launch = {'schema': 'tl3.container_launch.v1', 'launcher': launcher, 'name': 'tl3-pending',
              'mode': 'in_job', 'labels': owned['container']['labels']}
    result = runtime.cleanup_observations(launcher, launch=launch, podman=podman, systemctl=systemctl, timeout=2)
    assert result['ownership'] == launcher
    pending = result['pending_launch']
    assert 0 < pending['started_monotonic_ns'] <= pending['finished_monotonic_ns']
    assert pending == {'intent': launch, 'name_exists_exit': 1, 'sweep_exit': 0, 'remaining_ids': [],
                       'started_monotonic_ns': pending['started_monotonic_ns'],
                       'finished_monotonic_ns': pending['finished_monotonic_ns']}
    assert result['process_group_probe_errno'] == 3
    assert result['container'] is None
    assert not any(args[0] == 'rm' for args in map(json.loads, (root / 'cleanup-calls').read_text().splitlines()))


@pytest.mark.parametrize('mode,exitcode', [('structured', 0), ('structured', 7), ('nonzero', 7), ('malformed', 7)])
def test_backend_cli_failure_retains_scrubbed_status_and_both_streams(backend_cli, tmp_path, monkeypatch, mode, exitcode):
    from tl3_harness import runtime

    reply, _ = backend_cli
    output = tmp_path / 'correlation'
    output.mkdir()
    monkeypatch.setattr(runtime, 'BACKEND_OUTPUT_ROOT', tmp_path)
    # Trusted bootstrap output authority is installed by the Rust submitter.
    monkeypatch.setenv('TL3_RUN_DIAGNOSTIC_DIR', str(output))
    monkeypatch.setenv('OUT', str(output))
    executable = reply.parent / 'backend-cli'
    payload = (json.dumps(dict(schema='tl3.run_backend.frame.v1', sequence=0, kind='error',
                              code='invalid_request', message='native job does not match retained dispatch')) + '\n'
               if mode == 'structured' else 'native output marker\n' if mode == 'malformed' else
               json.dumps(dict(schema='tl3.run_backend.frame.v1', sequence=0, kind='result',
                               result=dict(op='launch_context', context='native output marker'))) + '\n')
    executable.write_text(f'''#!{sys.executable}
import sys
sys.stdin.read()
sys.stdout.write({payload!r})
sys.stdout.flush()
sys.stderr.write('native stderr marker password=actualSecretValue123!\\n')
sys.exit({exitcode})
''')
    with pytest.raises(RuntimeError) as caught:
        runtime.launch_context()
    diagnostic = caught.value.diagnostic
    assert diagnostic['status'] == {'code': exitcode, 'signal': None}
    assert 'native stderr marker' in diagnostic['stderr']['text']
    assert 'actualSecretValue123' not in str(caught.value)
    assert '[REDACTED]' in diagnostic['stderr']['text']
    assert diagnostic['stdout']['text'] == payload
    if mode == 'structured':
        assert diagnostic['cause']['code'] == 'invalid_request'
        assert 'native job does not match' in diagnostic['cause']['message']
    records = list(output.glob('backend-cli-*.json'))
    assert len(records) == 1
    assert json.loads(records[0].read_text()) == diagnostic
    assert 'actualSecretValue123' not in records[0].read_text()
    assert 'native stderr marker' in str(caught.value)


@pytest.mark.parametrize('mode', ['truncated', 'oversize', 'timeout', 'signal', 'spawn'])
def test_backend_cli_transport_failures_keep_durable_diagnostics(backend_cli, tmp_path, monkeypatch, mode):
    from tl3_harness import runtime

    reply, _ = backend_cli
    output = tmp_path / 'correlation'
    output.mkdir()
    monkeypatch.setattr(runtime, 'BACKEND_OUTPUT_ROOT', tmp_path)
    monkeypatch.setenv('TL3_RUN_DIAGNOSTIC_DIR', str(output))
    monkeypatch.setenv('OUT', str(output))
    executable = reply.parent / 'backend-cli'
    if mode == 'spawn':
        executable.unlink()
    else:
        action = {
            'truncated': "sys.stdout.write('unfinished frame'); sys.stdout.flush()",
            'oversize': "sys.stdout.write('stdout marker\\n' + '界' * 100000); sys.stdout.flush()",
            'timeout': 'time.sleep(10)',
            'signal': 'os.kill(os.getpid(), signal.SIGTERM)',
        }[mode]
        executable.write_text(f'''#!{sys.executable}
import os, signal, sys, time
sys.stdin.read()
sys.stderr.write('stderr marker password=actualSecretValue123!\\n' + '界\\n' * 40000)
sys.stderr.flush()
{action}
''')
    monkeypatch.setattr(runtime, 'BACKEND_IDLE_SECONDS', 0.25 if mode == 'timeout' else 30)
    with pytest.raises(RuntimeError) as caught:
        runtime.backend_request(dict(op='launch_context'), bounds={'cli_result_bytes': 1024})
    diagnostic = caught.value.diagnostic
    assert json.loads(next(output.glob('backend-cli-*.json')).read_text()) == diagnostic
    assert 'actualSecretValue123' not in str(caught.value)
    for stream in ('stdout', 'stderr'):
        assert len(diagnostic[stream]['text'].encode()) <= runtime.DIAGNOSTIC_TEXT_BYTES
        assert '\ufffd' not in diagnostic[stream]['text']
    if mode == 'spawn':
        assert diagnostic['status'] == {'code': None, 'signal': None}
        assert diagnostic['cause']['kind'] == 'FileNotFoundError'
    else:
        assert 'stderr marker' in diagnostic['stderr']['text']
        assert diagnostic['stderr']['capture_omitted_bytes'] > 0
        assert 'omitted' in diagnostic['stderr']['text']
        if mode == 'signal':
            assert diagnostic['status']['signal'] == signal.SIGTERM
        elif mode == 'timeout':
            assert diagnostic['status']['signal'] == signal.SIGKILL
            assert 'timed out' in diagnostic['cause']['message']
        elif mode == 'oversize':
            assert 'exceeds transport bound' in diagnostic['cause']['message']
            assert 'stdout marker' in diagnostic['stdout']['text']
        else:
            assert diagnostic['status']['code'] == 0
            assert 'truncated' in diagnostic['cause']['message']


def test_backend_cli_diagnostic_budget_does_not_clip_legal_results_or_stderr(backend_cli):
    from tl3_harness import runtime

    reply, _ = backend_cli
    payload = dict(op='launch_context', context='界' * 30000)
    reply.write_text(json.dumps(dict(schema='tl3.run_backend.frame.v1', sequence=0, kind='result', result=payload)) + '\n')
    executable = reply.parent / 'backend-cli'
    executable.write_text(executable.read_text() + "\nsys.stderr.write('diagnostic line\\n' * 5000)\n")
    assert runtime.backend_request(dict(op='launch_context'), bounds={'cli_result_bytes': 300000}) == payload


def test_backend_cli_success_drains_stderr_beyond_diagnostic_capture(backend_cli):
    from tl3_harness import runtime

    reply, _ = backend_cli
    payload = dict(op='launch_context', context='complete')
    reply.write_text(json.dumps(dict(schema='tl3.run_backend.frame.v1', sequence=0, kind='result', result=payload)) + '\n')
    executable = reply.parent / 'backend-cli'
    executable.write_text(executable.read_text() + "\nsys.stderr.write('diagnostic line\\n' * 300000)\n")
    assert runtime.backend_request(dict(op='launch_context')) == payload


def test_backend_cli_diagnostic_retains_unicode_and_surrogate_in_ascii_locale(backend_cli, tmp_path, monkeypatch):
    import os
    import subprocess

    reply, _ = backend_cli
    output = tmp_path / 'correlation'
    output.mkdir()
    monkeypatch.setenv('TL3_RUN_DIAGNOSTIC_DIR', str(output))
    monkeypatch.setenv('OUT', str(output))
    reply.write_text(json.dumps(dict(schema='tl3.run_backend.frame.v1', sequence=0, kind='error',
                                    code='invalid_request', message='cause \u754c\udcff')) + '\n')
    script = '''import json, sys
from pathlib import Path
from tl3_harness import runtime
runtime.BACKEND_OUTPUT_ROOT = Path(sys.argv[1])
try:
    runtime.launch_context()
except runtime.BackendCliError as error:
    print(json.dumps(error.diagnostic))
else:
    raise AssertionError('backend failure required')
'''
    result = subprocess.run([sys.executable, '-c', script, str(tmp_path)], capture_output=True,
                            env={**os.environ, 'LC_ALL': 'C', 'PYTHONUTF8': '0', 'PYTHONCOERCECLOCALE': '0'})
    assert result.returncode == 0, result.stderr
    diagnostic = json.loads(result.stdout)
    assert 'retention_error' not in diagnostic
    assert diagnostic['cause']['message'] == 'cause \u754c\udcff'
    assert json.loads(next(output.glob('backend-cli-*.json')).read_bytes()) == diagnostic


def test_backend_cli_retention_value_error_keeps_typed_failure(backend_cli, monkeypatch):
    from tl3_harness import runtime

    reply, _ = backend_cli
    reply.write_text('invalid frame\n')

    def unavailable(_diagnostic):
        raise ValueError('diagnostic encoding unavailable')

    monkeypatch.setattr(runtime, '_retain_backend_diagnostic', unavailable)
    with pytest.raises(runtime.BackendCliError) as caught:
        runtime.launch_context()
    assert caught.value.diagnostic['retention_error'] == 'diagnostic encoding unavailable'


@pytest.mark.parametrize('fault', ['caller_path', 'symlink'])
def test_backend_cli_diagnostic_refuses_unapproved_output(backend_cli, tmp_path, monkeypatch, fault):
    from tl3_harness import runtime

    reply, _ = backend_cli
    reply.write_text('bad frame\n')
    output = tmp_path / 'correlation'
    target = tmp_path / 'target'
    target.mkdir()
    if fault == 'symlink':
        output.symlink_to(target, target_is_directory=True)
    else:
        output = target
    monkeypatch.setattr(runtime, 'BACKEND_OUTPUT_ROOT', tmp_path)
    monkeypatch.setenv('TL3_RUN_DIAGNOSTIC_DIR', str(output))
    monkeypatch.setenv('OUT', str(output))
    with pytest.raises(runtime.BackendCliError) as caught:
        runtime.launch_context()
    assert 'retention_error' in caught.value.diagnostic
    assert list(target.iterdir()) == []


def test_backend_cli_scrubs_structured_cause_and_each_stream_before_retention(backend_cli, tmp_path, monkeypatch):
    from tl3_harness import runtime

    reply, _ = backend_cli
    output = tmp_path / 'correlation'
    output.mkdir()
    monkeypatch.setattr(runtime, 'BACKEND_OUTPUT_ROOT', tmp_path)
    monkeypatch.setenv('TL3_RUN_DIAGNOSTIC_DIR', str(output))
    monkeypatch.setenv('OUT', str(output))
    frame = dict(schema='tl3.run_backend.frame.v1', sequence=0, kind='error', code='invalid_request',
                 message='refused password=stdoutSecretValue123!')
    reply.write_text(json.dumps(frame) + '\n')
    executable = reply.parent / 'backend-cli'
    executable.write_text(executable.read_text() + "\nsys.stderr.write('stderr cause password=stderrSecretValue123!\\n')\nsys.exit(3)\n")
    with pytest.raises(runtime.BackendCliError) as caught:
        runtime.launch_context()
    diagnostic = caught.value.diagnostic
    assert 'stdoutSecretValue123' not in str(caught.value)
    assert 'stderrSecretValue123' not in str(caught.value)
    assert '[REDACTED]' in diagnostic['cause']['message']
    assert '[REDACTED]' in diagnostic['stdout']['text']
    assert '[REDACTED]' in diagnostic['stderr']['text']
    assert diagnostic['status'] == {'code': 3, 'signal': None}
    retained = next(output.glob('backend-cli-*.json'))
    assert json.loads(retained.read_text()) == diagnostic
    assert retained.stat().st_mode & 0o777 == 0o600


@pytest.mark.parametrize('opt_in', [False, True])
@pytest.mark.parametrize('generator', [False, True])
def test_x7_native_target_and_descendants_get_only_opted_in_flux_uri(tmp_path, monkeypatch, opt_in, generator):
    import os
    from tl3_harness import runtime

    uri = 'local:///job-owned/run/local-1'
    for name in ('FLUX_URI', 'FLUX_JOB_ID', 'FLUX_KVS_NAMESPACE', 'FLUX_JOB_TMPDIR'):
        monkeypatch.setenv(name, uri if name == 'FLUX_URI' else 'injected')
    flux_context(monkeypatch, pass_flux_uri=opt_in)
    code = ("import json,os,subprocess,sys; "
            "print(json.dumps({k:v for k,v in os.environ.items() if k.startswith('FLUX_')})); "
            "subprocess.run([sys.executable,'-c',\"import os,json; print(json.dumps({k:v for k,v in os.environ.items() if k.startswith('FLUX_')}))\"],check=True)")
    with (tmp_path / 'input').open('w+b') as selection:
        with Supervised([sys.executable, '-B', '-c', code], selection, (),
                        deadline_seconds=5 if generator else None) as process:
            output = process.child.communicate(timeout=5)[0]
    expected = {'FLUX_URI': uri} if opt_in else {}
    assert [json.loads(line) for line in output.splitlines()] == [expected, expected]
    assert os.environ['FLUX_JOB_ID'] == 'injected', 'supervisor keeps job metadata'


@pytest.mark.parametrize('opt_in', [False, True])
def test_x7_container_uses_job_uri_and_directory_mount(tmp_path, monkeypatch, opt_in):
    from tl3_harness import runtime

    monkeypatch.setenv('FLUX_URI', 'local:///job-owned/run/local-1')
    monkeypatch.setattr(runtime, 'check_rootless_directory', lambda env: None)
    import shutil
    monkeypatch.setattr(shutil, 'which', lambda name: '/usr/bin/' + name)
    context = flux_context(monkeypatch, pass_flux_uri=opt_in, native_threads=1,
                           coordinator_uri='local:///controller-submit-time',
                           runtime={'image': 'sha256:' + '1' * 64, 'mode': 'in_job',
                                    'executable': 'podman', 'trusted_root': '/harness', 'interpreter': 'python3'})
    argv = runtime.command(tmp_path / 'checkout', tmp_path / 'scratch')
    env = [argv[i + 1] for i, value in enumerate(argv) if value == '--env' and argv[i + 1].startswith('FLUX_')]
    mounts = [argv[i + 1] for i, value in enumerate(argv) if value == '--mount']
    assert env == (['FLUX_URI=local:///job-owned/run/local-1'] if opt_in else [])
    assert ('type=bind,src=/job-owned/run,dst=/job-owned/run' in mounts) == opt_in
    assert not any('/controller-submit-time' in value for value in argv)


@pytest.mark.parametrize('uri', ['local:///safe,ro=false', 'local://relative', 'ssh:///socket',
                                  'local:///safe=bad', 'local:///white space', 'local:///bad\npath'])
def test_x7_invalid_flux_uri_is_refused_with_typed_reason(tmp_path, uri):
    from tl3_harness import runtime

    with pytest.raises(ValueError) as error:
        runtime.podman_command(tmp_path, 'sha256:' + '1' * 64, tmp_path, 1,
                               mode='in_job', coordinator_uri=uri)
    assert getattr(error.value, 'reason', None) == 'invalid_flux_uri'
    assert uri not in str(error.value)


@pytest.mark.parametrize('field', ['tree', 'scratch', 'harness'])
def test_x7_podman_mount_values_refuse_option_delimiters(tmp_path, field):
    from tl3_harness import runtime

    values = {'tree': tmp_path, 'scratch': tmp_path, 'harness': tmp_path}
    values[field] = str(tmp_path) + ',ro=false'
    with pytest.raises(ValueError):
        runtime.podman_command(values['tree'], 'sha256:' + '1' * 64, values['scratch'], 1,
                               mode='in_job', harness=values['harness'])
