"""Subprocess fixture driven by the Rust CLI integration test, inside PB only."""
import faulthandler
import json
import os
from pathlib import Path
import runpy
import shutil
import signal
import threading
import subprocess
import sys
import time


def main():
    root, source, mode = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3]
    settings = json.loads((root / 'action-fixture.json').read_text())
    output = Path(settings['output_dir'])
    os.environ.update(settings['environment'])
    sys.path.insert(0, str(source / 'harness/src'))
    from tl3_harness import runtime

    request = runtime.backend_request
    def timed_request(command, **kwargs):
        started = time.monotonic()
        accepted = False
        try:
            if settings['mode'] == 'no-ownership' and command['op'] == 'validate_allocation':
                raise RuntimeError('admitted fixture has no retained prelaunch allocation ownership')
            if command['op'] == 'cleanup_runtime':
                if settings['mode'] == 'cancel-term-unconfirmed':
                    raise RuntimeError('admitted fixture cleanup transport remains unconfirmed')
                state = json.loads((root / 'journal' / os.environ['TL3_RUN_CORRELATION'] / 'state.json').read_text())
                for owned in state['runtime_ownerships']:
                    facts = runtime.cleanup_observations(owned, podman=str(root / 'podman'),
                        systemctl='/unused-fixture', timeout=1)
                    if mode in ('cancel-kill', 'timeout-kill'):
                        # This admitted fixture is the local subreaper. Model native init
                        # reaping only its adopted, exact recorded launcher group.
                        before_reap = facts
                        runtime._reap_owned_group(owned['process_group'], 1)
                        facts = runtime.cleanup_observations(owned, podman=str(root / 'podman'),
                            systemctl='/unused-fixture', timeout=1)
                        (output / ('external-fixture-reap-' + owned['execution'] + '.json')).write_text(
                            json.dumps({'before': before_reap, 'after': facts, 'attribution': 'admitted fixture subreaper'}))
                    (output / ('trusted-stop-' + owned['execution'] + '.json')).write_text(json.dumps(facts))
                for launch in state['container_launches'].values():
                    owned = launch['launcher']
                    if any(item['execution'] == owned['execution'] for item in state['runtime_ownerships']):
                        continue
                    facts = runtime.cleanup_observations(owned, launch=launch, podman=str(root / 'podman'),
                        systemctl='/unused-fixture', timeout=1)
                    (output / ('trusted-stop-' + owned['execution'] + '.json')).write_text(json.dumps(facts))
            result = request(command, **kwargs)
            accepted = True
            if command['op'] == 'begin_container_launch' and settings['mode'] == 'begin-response-lost':
                state = root / 'journal' / os.environ['TL3_RUN_CORRELATION'] / 'state.json'
                persisted = json.loads(state.read_text())
                assert persisted['container_launches'], 'fault must follow a real durable Rust begin'
                (output / 'begin-persisted-state.json').write_text(json.dumps(persisted))
                raise RuntimeError('fixture dropped successful durable begin response')
            if command['op'] == 'observe_allocation' and settings['mode'] == 'changed-allocation':
                native = json.loads((root / 'all.json').read_text())
                native['execution']['changed'] = native['execution'].get('changed', 0) + 1
                (root / 'all.json').write_text(json.dumps(native))
            return result
        finally:
            with (output / f'requests-{mode}.jsonl').open('a') as log:
                log.write(json.dumps({'op': command['op'], 'seconds': time.monotonic() - started,
                                      'finished': time.monotonic(), 'accepted': accepted}) + '\n')
    runtime.backend_request = timed_request

    if mode == 'action':
        (output / 'action-pid.json').write_text(json.dumps({'pid': os.getpid()}))
        # The native CLI and journal are real. Only the host observation and
        # container executable are deterministic fixture boundaries.
        original = runtime.command

        def command(tree, scratch):
            context = runtime.launch_context()
            return runtime.podman_command(tree, context['runtime_identity']['expected_image'], scratch, 1,
                                          mode='in_job', executable=root / 'podman')

        runtime.command = command
        snapshot = runtime.Evidence.host_snapshot
        runtime.Evidence.host_snapshot = staticmethod(lambda: {**snapshot(), 'hostname': '' if settings['mode'] == 'host-absent' else 'dl380g10',
            'cgroup': '0::/user.slice/user-1000.slice/user@1000.service/flux.slice/flux-jobs.slice/shell-fABC:sys.service\n'})
        runpy.run_path(str(root / 'staged/action.py'), run_name='__main__')
        runtime.command = original
        return

    staged = root / 'staged'
    staged.mkdir()
    trusted = staged / 'trusted/tl3_harness'
    shutil.copytree(source / 'harness/src/tl3_harness', trusted)
    shutil.copyfile(source / 'crates/controller/src/adapters/prismabuild/action.py', staged / 'action.py')
    repo = root / 'served'
    repo.mkdir()

    def git(*args):
        return subprocess.check_output(['git', '-C', str(repo), '-c', 'user.name=fixture',
                                       '-c', 'user.email=fixture@example.invalid', *args]).decode().strip()

    git('init', '-q', '--template=')
    (repo / 'pyproject.toml').write_text('[tool.tl3.harness]\ncommand=["python","-m","pytest"]\ntest-files=["test_*.py"]\n')
    (repo / 'test_value.py').write_text(
        'import time\ndef test_value():\n    time.sleep(12)\n    assert True\n'
        if mode in ('cancel-term', 'cancel-term-unconfirmed', 'cancel-kill', 'timeout-kill') else 'def test_value():\n    assert True\n')
    git('add', '.')
    git('commit', '-qm', 'fixture')
    head = git('rev-parse', 'HEAD')
    git('bundle', 'create', str(staged / 'served.bundle'), 'HEAD')
    (staged / 'request.json').write_text(json.dumps({
        'effect_key': 'fixture', 'base': head, 'head': head, 'image': 'sha256:' + 'b' * 64,
        'plan': [{'name': 'regression', 'kind': 'regression', 'tests': ['test_value.py::test_value']}],
    }))
    cpus = sorted(os.sched_getaffinity(0))
    pus = ','.join(map(str, cpus))
    if mode != 'refusal':
        mappings = [{'metadata': {'type': 'node', 'rank': 0, 'id': 0, 'name': 'dl380g10'}},
                    {'metadata': {'type': 'core', 'rank': 0, 'id': 37, 'properties': {'os-pus': pus}}}]
        core = '37'
        if mode == 'affinity-mismatch':
            mappings[1]['metadata']['properties']['os-pus'] = '65535'
        if mode == 'resource-mismatch':
            core = '37-38'
            mappings.append({'metadata': {'type': 'core', 'rank': 0, 'id': 38,
                                          'properties': {'os-pus': '65535'}}})
        (root / 'all.json').write_text(json.dumps({'version': 1, 'execution': {
            'R_lite': [{'rank': '0', 'children': {'core': core}}], 'nodelist': ['dl380g10']},
            'scheduling': {'graph': {'nodes': mappings}}}))
    if mode == 'session':
        context = runtime.launch_context()
        pid = runtime._backend_session.pid
        assert runtime.launch_context() == context and runtime._backend_session.pid == pid
        import fcntl

        with (root / 'journal' / context['correlation'] / 'lock').open('r') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        config_path = Path(os.environ['TL3_RUN_BACKEND_CONFIG'])
        original_config = config_path.read_bytes()
        config = json.loads(original_config)
        config['instance_generation'] = 'changed-generation'
        config_path.write_text(json.dumps(config))
        try:
            runtime.launch_context()
            raise AssertionError('session accepted changed generation')
        except runtime.BackendCliError as error:
            assert 'generation changed' in error.diagnostic['cause']['message']
        with_reaped = False
        try:
            os.waitpid(pid, os.WNOHANG)
        except ChildProcessError:
            with_reaped = True
        assert with_reaped
        config_path.write_bytes(original_config)
        owner = os.environ['TL3_RUN_OWNER']
        os.environ['TL3_RUN_OWNER'] = 'changed-owner'
        try:
            runtime.launch_context()
            raise AssertionError('session accepted changed owner')
        except runtime.BackendCliError as error:
            assert error.diagnostic['cause']['code'] == 'conflict'
        finally:
            os.environ['TL3_RUN_OWNER'] = owner
        host = runtime.Evidence.host_snapshot()
        facts = runtime.backend_request({'op': 'observe_allocation', 'owner': owner,
            'correlation': context['correlation'], 'observed': {'run': context['run'],
            'request_digest': context['request_digest'], 'hostname': 'dl380g10',
            'allowed_cpus': host['Cpus_allowed_list'], 'cgroup': host['cgroup']}})['facts']
        pid = runtime._backend_session.pid
        native = json.loads((root / 'all.json').read_text())
        native['execution']['changed'] = native['execution'].get('changed', 0) + 1
        (root / 'all.json').write_text(json.dumps(native))
        try:
            runtime.backend_request({'op': 'validate_allocation', 'owner': owner,
                'correlation': context['correlation'], 'facts': facts})
            raise AssertionError('session accepted changed allocation')
        except runtime.BackendCliError as error:
            assert 'changed after observation' in error.diagnostic['cause']['message']
        assert runtime._backend_session is None
        (output / 'session-assertions.json').write_text(json.dumps({
            'warm_context_same_pid': True, 'journal_lock_released_between_requests': True, 'changed_generation_refused': True,
            'changed_owner_refused': True, 'changed_live_R_refused': True, 'sessions_reaped': True}))
        return
    (root / 'podman').write_text(f'''#!{sys.executable}
import hashlib, json, os, pathlib, sys
root = pathlib.Path({str(root)!r})
args = sys.argv[1:]
with (root / 'runtime-calls.jsonl').open('a') as log:
    log.write(json.dumps(args) + '\\n')
if args[0] == 'run':
    if {mode!r} == 'absent-interpreter':
        try:
            os.execv('/__tl3_deliberately_absent_interpreter__', ['/__tl3_deliberately_absent_interpreter__'])
        except FileNotFoundError:
            print('catatonit: failed to exec pid1: No such file or directory', file=sys.stderr, flush=True)
            sys.exit(127)
    name = args[args.index('--name') + 1]
    labels = {{args[i+1].split('=',1)[0]: args[i+1].split('=',1)[1] for i in range(len(args)-1) if args[i]=='--label'}}
    metadata = {{'Id':hashlib.sha256(name.encode()).hexdigest(), 'Name':name, 'Labels':labels, 'Pid':os.getpid(), 'ConmonPid':os.getpid()}}
    (root / (name + '.container.json')).write_text(json.dumps(metadata))
    status = pathlib.Path('/proc/self/status').read_text()
    allowed = next(line.split(':',1)[1].strip() for line in status.splitlines() if line.startswith('Cpus_allowed_list:'))
    cgroup = '0::/user.slice/user-1000.slice/user@1000.service/flux.slice/flux-jobs.slice/shell-fABC:sys.service\\n' if '--cgroupns=host' in args else '0::/\\n'
    facts = {{'Cpus_allowed_list':allowed, 'cgroup':cgroup}}
    if {mode!r} == 'binding-mismatch':
        facts['Cpus_allowed_list'] = '65535'
    print('TL3_RUNTIME_EVIDENCE ' + json.dumps(facts), file=sys.stderr, flush=True)
    if os.read(0,1) != b'\\x01':
        sys.exit(125)
    (root / 'payload-started').touch()
    tree = next(arg[len('type=bind,src='):-len(',dst=/checkout,readonly')] for arg in args if arg.endswith(',dst=/checkout,readonly'))
    os.execv(sys.executable, [sys.executable, '-I', '-B', {str(source / 'harness/src/tl3_harness/runner.py')!r}, '-', tree])
if args[:2] == ['container','inspect']:
    print((root / (args[-1] + '.container.json')).read_text())
    sys.exit(0)
if args[:2] == ['container','exists']:
    sys.exit(1)
sys.exit(0)
''')
    (root / 'podman').chmod(0o700)
    original_popen = subprocess.Popen
    def observed_popen(command, *args, **kwargs):
        child = original_popen(command, *args, **kwargs)
        if isinstance(command, list) and command[-1:] == ['action']:
            wait = child.wait
            def observed_wait(*wait_args, **wait_kwargs):
                code = wait(*wait_args, **wait_kwargs)
                (output / 'action-process-status.json').write_text(json.dumps({'pid': child.pid, 'returncode': code}))
                return code
            child.wait = observed_wait
        return child
    subprocess.Popen = observed_popen
    signal_errors = []
    witness_handles = []
    latency = settings.get('injected_latency_seconds', 0)
    if not isinstance(latency, (int, float)) or isinstance(latency, bool) or not 0 <= latency < float('inf'):
        raise ValueError(f'injected_latency_seconds must be non-negative seconds, got {latency!r}')
    def cancel_qualified():
        try:
            # The injected latency runs before the action starts, so it extends this readiness wait.
            deadline = time.monotonic() + 30 + latency
            state_path = root / 'journal' / os.environ['TL3_RUN_CORRELATION'] / 'state.json'
            while time.monotonic() < deadline:
                state = json.loads(state_path.read_text())
                for owned in state['runtime_ownerships']:
                    execution = owned['execution']
                    launch = state['container_launches'][execution]
                    assert owned['run'] == launch['launcher']['run']
                    assert owned['request_digest'] == launch['launcher']['request_digest']
                    assert owned['owner'] == launch['launcher']['owner']
                    assert owned['container']['name'] == launch['name']
                    assert owned['container']['labels'] == launch['labels']
                    for path in output.glob('*.runtime.jsonl'):
                        rows = [json.loads(line) for line in path.read_text().splitlines()]
                        context = next((r['context'] for r in rows if r['event'] == 'launch'), None)
                        if context is None or context['run'] != owned['run']:
                            continue
                        process = next((r for r in rows if r['event'] == 'container_processes'
                                        and r['Id'] == owned['container']['id']), None)
                        bound = next((r for r in rows if r['event'] == 'launch_binding'
                                      and r['payload_allowed']), None)
                        qualified = next((r for r in rows if r['event'] == 'qualification' and r['qualified']), None)
                        bindings = runtime.binding_observations(output)
                        if not (process and bound and qualified and any(
                                r['execution'] == execution and r['container_id'] == process['Id']
                                and r['run'] == owned['run'] and r['request_digest'] == owned['request_digest']
                                for r in bindings)):
                            continue
                        assert process['Pid'] > 1 and process['ConmonPid'] > 1
                        pid = json.loads((output / 'action-pid.json').read_text())['pid']
                        number = signal.SIGKILL if mode in ('cancel-kill', 'timeout-kill') else signal.SIGTERM
                        trigger = {'signal': number, 'pid': pid, 'execution': execution,
                                   'process': process, 'owned': owned, 'bound': bound,
                                   'qualified': qualified, 'bindings': bindings,
                                   'monotonic_ns': time.monotonic_ns(), 'signals_sent': 1}
                        (output / 'signal-trigger.json').write_text(json.dumps(trigger))
                        witness_handles.extend(os.pidfd_open(process[key]) for key in ('Pid', 'ConmonPid'))
                        os.kill(pid, number)
                        return
                time.sleep(0.01)
            raise AssertionError('owned process capture and durable qualification never reached')
        except BaseException as error:
            signal_errors.append(repr(error))
    watcher = None
    if mode in ('cancel-term', 'cancel-term-unconfirmed', 'cancel-kill', 'timeout-kill'):
        watcher = threading.Thread(target=cancel_qualified)
        watcher.start()
    faulthandler.dump_traceback_later(10, repeat=True)
    started = time.monotonic()
    try:
        # Test-only contention model inside the measured window; the payload is unchanged.
        time.sleep(latency)
        code, result = runtime.collect_host_payload(
            [sys.executable, '-I', '-B', __file__, str(root), str(source), 'action'], timeout=80)
        print(json.dumps({'code': code, 'result': result}), flush=True)
    except Exception as error:
        print(json.dumps({'error': str(error), 'type': type(error).__name__}), flush=True)
    finally:
        faulthandler.cancel_dump_traceback_later()
        elapsed = time.monotonic() - started
        (output / 'reproduction-timing.json').write_text(json.dumps({'seconds': elapsed}))
        print(f'collector_seconds={elapsed:.3f}', flush=True)
    if watcher is not None:
        watcher.join(timeout=35 + latency)
        assert not watcher.is_alive() and not signal_errors, signal_errors
        assert (output / 'signal-trigger.json').exists()
        assert runtime.processes_stopped(witness_handles, 2), 'fixture captured processes did not cease'
        for handle in witness_handles:
            os.close(handle)
        absent = not Path(settings['scratch']).exists()
        facts = {'signal': 9 if mode in ('cancel-kill', 'timeout-kill') else 15, 'scratch': settings['scratch'],
                 'scratch_absent': absent, 'handler_record': (output / 'action-sigterm.json').exists(),
                 'finally_record': (output / 'action-scratch-absence.json').exists(),
                 'elapsed': elapsed, 'external_fixture_pidfd_cessation': True,
                 'process_status': json.loads((output / 'action-process-status.json').read_text())}
        (output / 'cancellation-assertions.json').write_text(json.dumps(facts))
        if mode == 'cancel-term-unconfirmed':
            assert facts['process_status']['returncode'] == 143, facts
            assert not absent and facts['handler_record'] and not facts['finally_record'], facts
            requests = [json.loads(line) for line in (output / 'requests-action.jsonl').read_text().splitlines()]
            assert sum(row['op'] == 'begin_container_launch' for row in requests) == 1
            assert 'TL3_CHECKS ' not in (output / 'host.stdout').read_text()
        elif mode == 'cancel-term':
            assert facts['process_status']['returncode'] in (-15, 143), facts
            assert absent, 'actual generated TERM action left own job scratch'
            assert facts['handler_record'] and facts['finally_record'], facts
        else:
            assert facts['process_status']['returncode'] == -9, facts
            assert not absent and not facts['handler_record'] and not facts['finally_record'], facts
        assert runtime._backend_session is None
        return
    raw = (output / 'host.stdout').read_text()
    # Rob 11-03: a performance-budget overrun is a warning, never a failure or a rerun.
    # The contractual bound is collect_host_payload(timeout=80) above.
    if elapsed > 15:
        print(json.dumps({'budget_warning': {'test': f'fx_action_e2e:{mode}', 'measured': elapsed,
                                             'limit': 15, 'metric': 'wall'}}), flush=True)
    if mode in ('success', 'success-retained'):
        assert '"verdict":"pass"' in raw, raw + (output / 'host.stderr').read_text()
        assert (root / 'payload-started').exists()
    else:
        assert not (root / 'payload-started').exists()
        assert 'TL3_EXECUTION_FAILURE ' in raw, (output / 'host.stderr').read_text()
        failure = next(json.loads(line.split(' ', 1)[1]) for line in raw.splitlines()
                       if line.startswith('TL3_EXECUTION_FAILURE '))
        expected = ('post_begin_launch_failure_candidate' if mode == 'absent-interpreter' else
                    'unknown_incomplete' if mode in ('binding-mismatch', 'begin-response-lost') else 'setup_failure')
        assert failure['outcome'] == expected, failure
        assert not failure['test_started'], failure
        assert 'infrastructure-failure' in raw, raw
    if mode in ('begin-response-lost', 'absent-interpreter'):
        assert any(Path(settings['scratch']).iterdir()), 'ambiguous begin must retain scratch'
    if mode == 'begin-response-lost':
        assert (output / 'begin-persisted-state.json').exists()
        assert not (root / 'runtime-calls.jsonl').exists() or all(
            json.loads(line)[0] != 'run' for line in (root / 'runtime-calls.jsonl').read_text().splitlines())
        assert not (root / 'payload-started').exists()
    elif mode != 'absent-interpreter':
        assert not Path(settings['scratch']).exists(), 'owned run scratch root retained'
    requests = [json.loads(line) for line in (output / 'requests-action.jsonl').read_text().splitlines()]
    operations = [item['op'] for item in requests if item['accepted']]
    launches = list(output.glob('*.runtime.jsonl'))
    events = [[json.loads(line) for line in path.read_text().splitlines()] for path in launches]
    if mode == 'absent-interpreter':
        assert 'begin_container_launch' in operations
        assert 'bind_container_launch' not in operations
        report = json.loads((output / 'report.json').read_text())
        assert report['outcome'] == 'post_begin_launch_failure', report
        assert report['launch_failures'] and not report['runtime_bindings']
        for log in events:
            names = [event['event'] for event in log]
            assert 'container' not in names
            assert names.index('launcher_exit') < names.index('no_container_observed')
            assert next(event['exit_code'] for event in log if event['event'] == 'launcher_exit') == 127
    elif mode in ('success', 'success-retained', 'binding-mismatch'):
        for event_log in events:
            names = [event['event'] for event in event_log]
            assert names.index('allocation_grant') < names.index('launch_intent') < names.index('container')
            context = next(event['context'] for event in event_log if event['event'] == 'launch')
            grant = next(event for event in event_log if event['event'] == 'allocation_grant')
            assert grant['run'] == context['run'] and grant['request_digest'] == context['request_digest']
            assert grant['os_pus'] == cpus and grant['rank'] == 0
        assert operations.index('observe_allocation') < operations.index('validate_allocation') < operations.index('begin_container_launch')
        assert 'bind_container_launch' in operations
        if mode in ('success', 'success-retained'):
            assert operations.index('bind_container_launch') < operations.index('validate_runtime_binding')
            assert (output / 'runtime-bindings.jsonl').stat().st_size
    elif mode == 'begin-response-lost':
        assert 'begin_container_launch' in operations
        assert 'bind_container_launch' not in operations and 'payload_allowed' not in operations
        assert not (output / 'runtime-bindings.jsonl').exists()
    else:
        assert 'begin_container_launch' not in operations
        assert not (root / 'runtime-calls.jsonl').exists(), 'runtime effect before allocation gate'
        assert all(not any(event['event'] == 'launch_intent' for event in log) for log in events)
    assert runtime._backend_session is None
    (output / 'gate-assertions.json').write_text(json.dumps({
        'mode': mode, 'seconds': elapsed, 'typed_refusal': mode != 'success',
        'allocation_before_durable_launch': True, 'no_payload_before_gates': True,
        'binding_and_qualification_exercised': mode == 'success',
        'own_scratch_absent': not Path(settings['scratch']).exists(),
        'uncertain_scratch_retained': mode == 'begin-response-lost', 'collector_session_closed': True}))


if __name__ == '__main__':
    main()
