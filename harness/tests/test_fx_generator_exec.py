"""FLUX-GENERATOR-EXEC: a native Flux generator runs on the SAME owned path as test jobs.

Real code under test: generator.run/main, Supervised, runtime.command/podman_command,
prepare_allocation, begin/bind/validate/cleanup clients, runtime_launcher.py, workspace
scratch and the shared Podman containment flags. Fixture boundaries only: the Rust
backend operations (an in-process journal that enforces ordering) and the Podman
executable (it emulates runtime_evidence.py's gate, then executes the configured
generator argv against the mounted private checkout).

Phase status at authoring (11-09): UNRUN. No RED has been observed. Expected outcomes
before implementation, from source reading only:
- actual 018412: under Flux the generator splices docker-shaped flags into the Podman
  argv and runs it with bare subprocess.run (no allocation/begin/bind/cleanup) or
  refuses on the '--entrypoint=python3' splice, so every completed-path test fails.
- prep 80ddef3e: generator.container_command raises FluxContainerUnsupported, so the
  completed-path tests fail with a typed preparation failure.
GREEN requires the generator to launch through Supervised with the Rust launch intent.

Also a subprocess driver (``python3 test_fx_generator_exec.py ROOT SOURCE MODE``) for the
proposed real-collector Rust e2e in crates/controller/tests/run_backend_cli.rs.
"""
import hashlib
import json
import os
from pathlib import Path
import signal
import stat
import sys
import threading
import time

import pytest


IMAGE = 'sha256:' + 'b' * 64
OUTPUT_BYTES = 32 * 1_048_576
GATED_EXEC = ['-I', '-B', '/harness/runtime_evidence.py', '--exec', '--']
CONTAINMENT = ('--cgroups=disabled', '--cgroupns=host', '--init', '--network=none', '--http-proxy=false',
               '--read-only', '--read-only-tmpfs=false', '--interactive', '--rm', '--pull=never',
               '--cap-drop=ALL', '--security-opt=no-new-privileges', '--pids-limit=128', '--ipc=private',
               '--pid=private', '--workdir=/checkout')
GENERATOR = '''import pathlib, sys, time
mode = sys.argv[1]
out = pathlib.Path('generated')
out.mkdir(exist_ok=True)
if mode == 'fail':
    (out / 'partial.txt').write_text('must never be imported\\n')
    print('generator failing deliberately', flush=True)
    sys.exit(3)
if mode == 'sleep':
    print('generator sleeping', flush=True)
    time.sleep(120)
(out / 'out.txt').write_text('generated from ' + pathlib.Path('data.txt').read_text())
print('generated ok', flush=True)
'''


def write_podman(root, image):
    """Emulates Podman plus /harness/runtime_evidence.py's gate. It refuses unowned launches."""
    podman = Path(root) / 'podman'
    podman.write_text(f'''#!{sys.executable}
import hashlib, json, os, pathlib, sys
root = pathlib.Path({str(root)!r})
args = sys.argv[1:]
with (root / 'runtime-calls.jsonl').open('a') as log:
    log.write(json.dumps(args) + '\\n')
if args[:1] == ['run']:
    if '--name' not in args or '--label' not in args:
        print('fixture podman: unowned launch refused', file=sys.stderr, flush=True)
        sys.exit(125)
    name = args[args.index('--name') + 1]
    labels = dict(args[i + 1].split('=', 1) for i in range(len(args) - 1) if args[i] == '--label')
    mounts = [args[i + 1] for i in range(len(args) - 1) if args[i] == '--mount']
    env = dict(args[i + 1].split('=', 1) for i in range(len(args) - 1) if args[i] == '--env')
    metadata = {{'Id': hashlib.sha256(name.encode()).hexdigest(), 'Name': name, 'Labels': labels,
                 'Pid': os.getpid(), 'ConmonPid': os.getpid()}}
    (root / (name + '.container.json')).write_text(json.dumps(metadata))
    status = pathlib.Path('/proc/self/status').read_text()
    allowed = next(line.split(':', 1)[1].strip() for line in status.splitlines()
                   if line.startswith('Cpus_allowed_list:'))
    print('TL3_RUNTIME_EVIDENCE ' + json.dumps({{'Cpus_allowed_list': allowed, 'cgroup': '0::/fixture\\n'}}),
          file=sys.stderr, flush=True)
    if os.read(0, 1) != b'\\x01':
        sys.exit(125)
    payload = args[args.index({image!r}) + 1:]
    (root / 'payload-started').write_text(json.dumps(payload))
    if payload[:5] != {GATED_EXEC!r} or len(payload) < 6:
        print('fixture podman: payload is not the gated generator exec', file=sys.stderr, flush=True)
        sys.exit(126)
    checkout = [m for m in mounts if m.startswith('type=bind,src=') and m.endswith(',dst=/checkout')]
    if len(checkout) != 1:
        print('fixture podman: generator needs exactly one writable /checkout', file=sys.stderr, flush=True)
        sys.exit(126)
    os.chdir(checkout[0][len('type=bind,src='):-len(',dst=/checkout')])
    argv = payload[5:]
    executable = {{'python3': sys.executable}}.get(argv[0], argv[0])
    os.execve(executable, [executable, *argv[1:]], {{'PATH': '/usr/bin:/bin', **env}})
if args[:2] == ['container', 'inspect']:
    print((root / (args[-1] + '.container.json')).read_text())
    sys.exit(0)
if args[:2] == ['container', 'exists']:
    sys.exit(1)
sys.exit(0)
''')
    podman.chmod(0o700)
    return podman


def runtime_calls(root):
    path = Path(root) / 'runtime-calls.jsonl'
    return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []


def source_tree(root):
    tree = Path(root) / 'checkout'
    (tree / 'tools').mkdir(parents=True)
    (tree / 'tools/gen.py').write_text(GENERATOR)
    (tree / 'data.txt').write_text('immutable input\n')
    return tree


def contract(mode, **overrides):
    value = {'command': f'python3 tools/gen.py {mode}', 'generated_files': ['generated/*'],
             'protected_paths': [], 'files': [], 'timeout': 30, 'cpus': 1, 'mem_gb': 1}
    value.update(overrides)
    return value


class Backend:
    """In-process stand-in for the Rust CLI operations. Enforces the owned launch order."""

    def __init__(self, root, context, *, qualified=True, stopped=True, payload_allowed=True):
        self.root, self.context = Path(root), context
        self.qualified, self.stopped, self.payload_allowed = qualified, stopped, payload_allowed
        self.ops, self.launches, self.grant = [], {}, None

    def __call__(self, request, **kwargs):
        op = request['op']
        self.ops.append(op)
        assert request.get('owner', 'owner') == 'owner'
        if op == 'observe_allocation':
            assert request['observed']['run'] == self.context['run']
            return {'facts': {'observed': request['observed']}}
        if op == 'validate_allocation':
            placement = self.context['placement']
            grant = {'run': self.context['run'], 'request_digest': self.context['request_digest'],
                     **{key: placement[key] for key in ('allocation', 'topology', 'host', 'rank', 'os_pus')}}
            assert self.grant in (None, grant), 'allocation grant is immutable for one run'
            self.grant = grant
            return {'grant': grant}
        if op == 'begin_container_launch':
            assert not any(call[:1] == ['run'] for call in runtime_calls(self.root)), \
                'Podman must stay gated until Rust persists the launch intent'
            launcher = request['launcher']
            assert launcher['container'] is None and launcher['run'] == self.context['run']
            assert launcher['execution'] not in self.launches, 'one durable intent per execution'
            name = 'tl3-' + hashlib.sha256(json.dumps(launcher, sort_keys=True).encode()).hexdigest()
            launch = {'launcher': launcher, 'name': name, 'mode': 'in_job',
                      'labels': {'tl3.backend': 'flux', 'tl3.execution': launcher['execution'],
                                 'tl3.owner': 'owner', 'tl3.correlation': 'correlation'}}
            self.launches[launcher['execution']] = launch
            return {'launch': launch}
        if op == 'bind_container_launch':
            observed = request['observed']
            launch = self.launches[observed['execution']]
            assert observed['container']['name'] == launch['name']
            assert observed['container']['labels'] == launch['labels']
            assert not (self.root / 'payload-started').exists(), 'payload ran before binding'
            return {'binding': {'intent': launch, 'ownership': observed, 'payload_allowed': self.payload_allowed}}
        if op == 'validate_runtime_binding':
            assert 'bind_container_launch' in self.ops
            assert request['facts']['execution'] in self.launches
            if self.qualified:
                return {'qualification': {'qualified': True}}
            return {'qualification': {'qualified': False, 'reason': 'fixture binding unqualified'}}
        if op == 'cleanup_runtime':
            return {'cleanup': {'stopped': self.stopped, 'evidence': []}}
        raise AssertionError(f'unexpected Rust operation {op}')

    def first(self, op):
        return self.ops.index(op)


@pytest.fixture
def flux(tmp_path, monkeypatch):
    if os.geteuid() == 0:
        pytest.skip('rootless Podman contract requires an unprivileged fleet user')
    from tl3_harness import runtime, scratch

    bin_dir = tmp_path / 'bin'
    bin_dir.mkdir()
    for name in ('crun', 'catatonit'):
        (bin_dir / name).write_text('#!/bin/sh\nexit 0\n')
        (bin_dir / name).chmod(0o700)
    xdg = tmp_path / 'xdg'
    xdg.mkdir(mode=0o700)
    monkeypatch.setenv('PATH', f"{bin_dir}{os.pathsep}{os.environ.get('PATH', '/usr/bin:/bin')}")
    monkeypatch.setenv('HOME', str(tmp_path))
    monkeypatch.setenv('XDG_RUNTIME_DIR', str(xdg))
    output = tmp_path / 'output'
    output.mkdir()
    scratch_root = tmp_path / 'scratch'
    scratch_root.mkdir(mode=0o700)
    host = os.uname().nodename
    harness = Path(runtime.__file__).resolve().parent
    context = {
        'run': {'backend': 'flux', 'uri': 'local:///fixture', 'instance': 'run/generation', 'job': 'fJob'},
        'owner': 'owner', 'correlation': 'correlation', 'request_digest': 'f' * 64,
        'output_dir': str(output),
        'progress': {'journal': str(output / 'progress.json'), 'no_progress_seconds': 5},
        'scratch': {'approved_root': str(scratch_root), 'leaf': str(scratch_root / 'fJob')},
        'native_threads': 1,
        'runtime': {'mode': 'in_job', 'image': IMAGE, 'executable': str(write_podman(tmp_path, IMAGE)),
                    'trusted_root': str(harness), 'interpreter': '/usr/bin/python3.12'},
        'runtime_identity': {'cleanup': {'fixture': 'pinned cleanup helper'}},
        'placement': {'kind': 'observed', 'allocation': {'fixture': 1}, 'topology': {'fixture': 1},
                      'host': host, 'rank': 0, 'os_pus': sorted(os.sched_getaffinity(0))},
    }
    monkeypatch.setattr(runtime, 'launch_context', lambda: context)
    uncertain, owned = set(runtime._uncertain_containers), dict(scratch._owned)
    yield {'root': tmp_path, 'context': context, 'output': output, 'leaf': scratch_root / 'fJob'}
    runtime._uncertain_containers.clear()
    runtime._uncertain_containers.update(uncertain)
    scratch._owned.clear()
    scratch._owned.update(owned)
    for path in [scratch_root, *scratch_root.rglob('*')]:
        if path.is_dir() and not path.is_symlink():
            path.chmod(0o700)


def use_backend(monkeypatch, flux, **behaviour):
    from tl3_harness import runtime

    backend = Backend(flux['root'], flux['context'], **behaviour)
    monkeypatch.setattr(runtime, 'backend_request', backend)
    return backend


def run_generator(flux, mode, **overrides):
    from tl3_harness import generator

    tree = source_tree(flux['root'])
    before = generator.manifest(tree)
    result = generator.run(tree, generator.Container(IMAGE), **contract(mode, **overrides))
    return tree, before, result


def launched(flux):
    runs = [call for call in runtime_calls(flux['root']) if call[:1] == ['run']]
    assert len(runs) == 1, runs
    return runs[0]


def mounts(argv):
    return [argv[i + 1] for i in range(len(argv) - 1) if argv[i] == '--mount']


@pytest.mark.parametrize('interpreter', ['python3', '/usr/bin/python3.12'])
def test_flux_generator_launches_through_rust_intent_not_refused_or_bypassed(flux, monkeypatch, interpreter):
    # RED target: prep's FluxContainerUnsupported refusal, and 018412's bare subprocess.run
    # bypass ('python3') or its '--entrypoint=python3' splice failure ('/usr/bin/python3.12').
    flux['context']['runtime']['interpreter'] = interpreter
    backend = use_backend(monkeypatch, flux)
    _, _, result = run_generator(flux, 'ok')
    assert result['status'] == 'completed', result
    assert result['exit_code'] == 0 and result['phase'] == 'completed', result
    assert backend.ops.count('begin_container_launch') == 1, backend.ops
    assert (backend.first('observe_allocation') < backend.first('validate_allocation')
            < backend.first('begin_container_launch') < backend.first('bind_container_launch')
            < backend.first('validate_runtime_binding') < backend.first('cleanup_runtime')), backend.ops
    argv = launched(flux)
    (launch,) = backend.launches.values()
    assert argv[argv.index('--name') + 1] == launch['name']
    labels = dict(argv[i + 1].split('=', 1) for i in range(len(argv) - 1) if argv[i] == '--label')
    assert labels == launch['labels']
    assert f'--entrypoint={interpreter}' in argv


def test_flux_generator_argv_keeps_test_containment_with_only_generator_deltas(flux, monkeypatch):
    use_backend(monkeypatch, flux)
    tree, _, result = run_generator(flux, 'ok')
    assert result['status'] == 'completed', result
    argv = launched(flux)
    for flag in CONTAINMENT:
        assert flag in argv, flag
    assert not any(arg.startswith(('--cidfile', '--userns', '--user=', '--memory', '--cpuset-cpus'))
                   for arg in argv), argv
    assert all(Path(call[0]).name != 'docker' for call in runtime_calls(flux['root']))
    bound = mounts(argv)
    checkout = [m for m in bound if m.endswith(',dst=/checkout')]
    assert len(checkout) == 1, bound
    copy = Path(checkout[0][len('type=bind,src='):-len(',dst=/checkout')])
    # The writable target is a private copy inside the Rust-owned job scratch, never the source.
    assert copy.is_relative_to(flux['leaf']) and copy != tree, copy
    assert f'type=bind,src={copy / ".git"},dst=/checkout/.git,readonly' in bound, bound
    assert any(m.endswith(',dst=/harness,readonly') for m in bound), bound
    assert any(m.endswith(',dst=/tmp') and Path(m[len('type=bind,src='):-len(',dst=/tmp')]).is_relative_to(flux['leaf'])
               for m in bound), bound
    assert len(bound) == 4, bound
    assert argv[argv.index('--ulimit') + 1] == f'fsize={OUTPUT_BYTES}:{OUTPUT_BYTES}'
    env = [argv[i + 1] for i in range(len(argv) - 1) if argv[i] == '--env']
    for pair in ('PYTHONDONTWRITEBYTECODE=1', 'GIT_CONFIG_NOSYSTEM=1', 'GIT_CONFIG_GLOBAL=/dev/null',
                 'GIT_TERMINAL_PROMPT=0', 'GIT_NO_LAZY_FETCH=1', 'GIT_CONFIG_VALUE_0=/checkout', 'HOME=/tmp'):
        assert pair in env, env
    assert not any('flux-out' in item or 'TL3_RUN' in item or 'token' in item.lower() for item in env + bound)
    # The trusted gate precedes the configured argv; the image never supplies an entrypoint.
    assert argv[argv.index(IMAGE) + 1:] == [*GATED_EXEC, 'python3', 'tools/gen.py', 'ok'], argv


def test_flux_generator_reads_outputs_only_after_confirmed_stop_and_artifact_imports(flux, monkeypatch):
    from tl3_harness import generator

    backend = use_backend(monkeypatch, flux)
    original = generator.manifest
    reads = []

    def observed(root):
        reads.append((Path(root), len(backend.ops)))
        return original(root)

    monkeypatch.setattr(generator, 'manifest', observed)
    tree, before, result = run_generator(flux, 'ok')
    assert result['status'] == 'completed', result
    cleanup = backend.first('cleanup_runtime')
    copies = [count for root, count in reads if root != tree]
    assert copies and all(count > cleanup for count in copies), (reads, backend.ops)
    assert [output['path'] for output in result['outputs']] == ['generated/out.txt'], result
    # The maker's import boundary accepts exactly this artifact and nothing forged.
    after = generator.validate_artifact(result, before, contract('ok'))
    assert after['generated/out.txt']['sha256'] == hashlib.sha256(
        b'generated from immutable input\n').hexdigest()
    forged = json.loads(json.dumps(result))
    forged['outputs'][0]['entry']['sha256'] = '0' * 64
    with pytest.raises(ValueError):
        generator.validate_artifact(forged, before, contract('ok'))
    assert not flux['leaf'].exists(), 'own job scratch retained after confirmed cleanup'
    bindings = [json.loads(line) for line in (flux['output'] / 'runtime-bindings.jsonl').read_text().splitlines()]
    assert [row['execution'] for row in bindings] == list(backend.launches), bindings


def test_flux_generator_nonzero_exit_is_typed_failed_with_cleanup_and_no_outputs(flux, monkeypatch):
    backend = use_backend(monkeypatch, flux)
    _, _, result = run_generator(flux, 'fail')
    assert result['status'] == 'failed' and result['exit_code'] == 3, result
    assert 'outputs' not in result and 'output_manifest' not in result, result
    assert result['timings']['container']['outcome'] == 'failed', result
    assert result['timings']['validation']['outcome'] == 'skipped', result
    assert 'cleanup_runtime' in backend.ops and 'begin_container_launch' in backend.ops
    assert 'deliberately' in result.get('log', ''), result
    assert not flux['leaf'].exists(), 'proven cleanup must release own job scratch'


def test_flux_generator_deadline_stops_owned_container_and_reports_timeout(flux, monkeypatch):
    backend = use_backend(monkeypatch, flux)
    started = time.monotonic()
    _, _, result = run_generator(flux, 'sleep', timeout=2)
    assert time.monotonic() - started < 30
    assert result['status'] == 'failed' and result['exit_code'] == 124, result
    assert 'deadline' in result['message'], result
    assert 'outputs' not in result, result
    assert backend.ops[-1] == 'cleanup_runtime', backend.ops
    container = json.loads((flux['root'] / (next(iter(backend.launches.values()))['name'] + '.container.json')).read_text())
    assert not Path(f"/proc/{container['Pid']}").exists(), 'owned generator process survived its deadline'


def test_flux_generator_unqualified_binding_never_starts_configured_argv(flux, monkeypatch):
    backend = use_backend(monkeypatch, flux, qualified=False)
    _, _, result = run_generator(flux, 'ok')
    assert result['status'] == 'failed', result
    assert 'outputs' not in result, result
    assert not (flux['root'] / 'payload-started').exists(), 'generator argv ran before qualification'
    assert 'begin_container_launch' in backend.ops and backend.ops[-1] == 'cleanup_runtime', backend.ops
    assert result['cause']['operation'] == 'container', result


def test_flux_generator_unproven_cleanup_never_completes_and_keeps_scratch_debt(flux, monkeypatch):
    from tl3_harness import runtime

    use_backend(monkeypatch, flux, stopped=False)
    _, _, result = run_generator(flux, 'ok')
    assert result['status'] == 'failed', result
    assert 'outputs' not in result and 'output_manifest' not in result, result
    assert result['cause']['operation'] == 'container', result
    assert flux['leaf'].exists() and any(flux['leaf'].iterdir()), 'unproven cleanup released scratch'
    assert runtime._uncertain_containers, 'unconfirmed container must remain uncertain debt'


def test_flux_generator_term_unwinds_owned_cleanup_and_emits_no_artifact(flux, monkeypatch, capsys):
    from tl3_harness import generator

    backend = use_backend(monkeypatch, flux)
    tree = source_tree(flux['root'])
    (flux['root'] / 'generator.json').write_text(json.dumps(contract('sleep')))
    monkeypatch.chdir(flux['root'])
    monkeypatch.setattr('sys.argv', ['generator', IMAGE])
    sent = []

    def cancel():
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline and not (flux['root'] / 'payload-started').exists():
            time.sleep(0.01)
        handler = signal.getsignal(signal.SIGTERM)
        if (flux['root'] / 'payload-started').exists() and callable(handler):
            sent.append('SIGTERM')
            os.kill(os.getpid(), signal.SIGTERM)
            return
        # Without a cancellation handler SIGTERM would kill the test runner; stop the fixture instead.
        sent.append('no-handler')
        for path in flux['root'].glob('*.container.json'):
            try:
                os.kill(json.loads(path.read_text())['Pid'], signal.SIGKILL)
            except ProcessLookupError:
                pass

    watcher = threading.Thread(target=cancel, daemon=True)
    watcher.start()
    with pytest.raises(SystemExit) as stopped:
        generator.main()
    watcher.join(timeout=30)
    assert sent == ['SIGTERM'], sent
    assert stopped.value.code == 128 + signal.SIGTERM
    assert generator.PREFIX not in capsys.readouterr().out, 'cancelled generator emitted an artifact'
    assert 'cleanup_runtime' in backend.ops, backend.ops
    assert (flux['output'] / 'action-sigterm.json').exists()
    assert not flux['leaf'].exists(), 'confirmed TERM cleanup must release own job scratch'
    assert stat.S_ISDIR(tree.stat().st_mode)


def driver():
    """Real host collector -> real Rust collect_worker driver. Native Flux stays the Rust fixture."""
    root, source, mode = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3]
    settings = json.loads((root / 'action-fixture.json').read_text())
    output = Path(settings['output_dir'])
    os.environ.update(settings['environment'])
    sys.path.insert(0, str(source / 'harness/src'))
    from tl3_harness import generator, runtime

    if mode.startswith('action-'):
        podman = root / 'podman'

        def command(tree, scratch, **kwargs):
            context = runtime.launch_context()
            return runtime.podman_command(tree, context['runtime_identity']['expected_image'], scratch, 1,
                                          mode='in_job', executable=podman, **kwargs)

        runtime.command = command
        snapshot = runtime.Evidence.host_snapshot
        runtime.Evidence.host_snapshot = staticmethod(lambda: {**snapshot(), 'hostname': 'dl380g10',
            'cgroup': '0::/user.slice/user-1000.slice/user@1000.service/flux.slice/flux-jobs.slice/shell-fABC:sys.service\n'})
        (output / 'action-pid.json').write_text(json.dumps({'pid': os.getpid()}))
        os.chdir(root / 'generator-work')
        sys.argv = ['generator', settings['image']]
        generator.main()
        return
    write_podman(root, settings['image'])
    pus = ','.join(map(str, sorted(os.sched_getaffinity(0))))
    (root / 'all.json').write_text(json.dumps({'version': 1, 'execution': {
        'R_lite': [{'rank': '0', 'children': {'core': '37'}}], 'nodelist': ['dl380g10']},
        'scheduling': {'graph': {'nodes': [
            {'metadata': {'type': 'node', 'rank': 0, 'id': 0, 'name': 'dl380g10'}},
            {'metadata': {'type': 'core', 'rank': 0, 'id': 37, 'properties': {'os-pus': pus}}}]}}}))
    work = root / 'generator-work'
    work.mkdir()
    source_tree(work)
    generator_mode = {'success': 'ok', 'failure': 'fail', 'cancel': 'sleep'}[mode]
    (work / 'generator.json').write_text(json.dumps(contract(generator_mode)))
    signals = []

    def cancel():
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if (root / 'payload-started').exists() and (output / 'action-pid.json').exists():
                pid = json.loads((output / 'action-pid.json').read_text())['pid']
                os.kill(pid, signal.SIGTERM)
                signals.append(pid)
                return
            time.sleep(0.01)

    watcher = threading.Thread(target=cancel, daemon=True) if mode == 'cancel' else None
    if watcher is not None:
        watcher.start()
    try:
        code, result = runtime.collect_host_payload(
            [sys.executable, '-I', '-B', __file__, str(root), str(source), 'action-' + mode],
            timeout=90, operation='collect_worker')
        outcome = {'code': code, 'result': result, 'error_type': None}
    except Exception as error:
        outcome = {'error_type': type(error).__name__, 'diagnostic': getattr(error, 'diagnostic', None)}
    if watcher is not None:
        watcher.join(timeout=35)
    outcome.update(signals=signals, scratch_absent=not Path(settings['scratch']).exists(),
                   payload_started=(root / 'payload-started').exists(),
                   runtime_calls=[call[:1] for call in runtime_calls(root)])
    (output / 'generator-exec-driver.json').write_text(json.dumps(outcome))
    print(json.dumps(outcome), flush=True)


if __name__ == '__main__':
    driver()
