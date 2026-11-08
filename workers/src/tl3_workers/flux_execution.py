"""Worker orchestration over Rust-generated execution commands and verified payloads."""
from functools import cache
import hashlib
import json
import os
from pathlib import Path
import secrets
import signal
import subprocess

import jsonschema

from tl3_workers import reply_check
from tl3_harness.supervision import ExecutionFailure, ExecutionOutcome


def control(document):
    return reply_check.run_check(['worker-execution'], document, name='worker execution', timeout=60)['result']


def activation(settings):
    try:
        control({'op': 'activation', 'profile': settings.execution['profile'], 'image': settings.image})
    except reply_check.ReplyCheckInfrastructureError as error:
        raise ExecutionFailure(ExecutionOutcome.UNKNOWN_INCOMPLETE, 'admission', 'runtime_activation_held') from error


def allocate(settings):
    call = secrets.token_hex(32)
    root = control({'op': 'allocate', 'profile': settings.execution['profile'], 'call': call})['root']
    return call, Path(root)


def prepare(settings, call, root, tree, contract, *, generator=False, parallel=0,
            walltime=86400, arguments=()):
    raw = json.dumps(contract, sort_keys=True).encode()
    (root / 'worker-input.json').write_bytes(raw)
    identity = hashlib.sha256(raw).hexdigest()
    plan = control({'op': 'prepare', 'profile': settings.execution['profile'], 'call': call,
                    'served_tree': tree, 'contract': identity, 'generator': generator,
                    'image': settings.image, 'cpus': settings.cpus, 'mem_gb': settings.mem_gb,
                    'parallel': parallel, 'quiet_seconds': settings.quiet_seconds,
                    'walltime_seconds': walltime, 'arguments': list(arguments)})
    return plan


def resume(settings, key, tree, contract):
    identity = hashlib.sha256(json.dumps(contract, sort_keys=True).encode()).hexdigest()
    return control({'op': 'resume', 'profile': settings.execution['profile'], 'key': key,
                    'served_tree': tree, 'contract': identity})


@cache
def _wire(cli):
    result = subprocess.run([cli, 'schema'], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                            check=True, timeout=30)
    schema = json.loads(result.stdout)
    return {kind: jsonschema.Draft202012Validator({
        '$ref': schema['properties'][kind]['$ref'], '$defs': schema['$defs']})
        for kind in ('command', 'frame')}


def _command(plan, command):
    wire = _wire(plan['cli'])
    wire['command'].validate(command)
    result = None
    with subprocess.Popen([plan['cli'], '--config', plan['config']], stdin=subprocess.PIPE,
                          stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                          start_new_session=True) as process:
        try:
            process.stdin.write(json.dumps(command).encode() + b'\n')
            process.stdin.flush()
            sequence = 0
            while True:
                raw = process.stdout.readline(plan['bounds']['cli_envelope_bytes'] + 1)
                if not raw:
                    break
                if len(raw) > plan['bounds']['cli_envelope_bytes'] or not raw.endswith(b'\n'):
                    raise ValueError('Shared execution CLI exceeded its generated envelope bound')
                frame = json.loads(raw)
                wire['frame'].validate(frame)
                if frame['sequence'] != sequence:
                    raise ValueError('Shared execution CLI sequence changed')
                sequence += 1
                if frame['kind'] == 'error':
                    raise ValueError('Shared execution retained an unknown or refused outcome')
                if frame['kind'] == 'result':
                    if result is not None or frame['result']['op'] != command['op']:
                        raise ValueError('Shared execution returned an unrelated result')
                    result = frame['result']
            if process.wait(timeout=30) or result is None:
                raise ValueError('Shared execution did not return a verified result')
            return result
        finally:
            if process.poll() is None:
                # Stop observation only. Never turn a reader failure into native cancellation.
                try:
                    process.stdin.write(json.dumps({'op': 'stop_watch'}).encode() + b'\n')
                    process.stdin.flush()
                    process.wait(timeout=30)
                except (OSError, subprocess.TimeoutExpired):
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    process.wait()


def _blob(reference, bound):
    if reference['bytes'] > bound or Path(reference['path']).is_symlink():
        raise ValueError('Verified worker payload exceeds its generated bound')
    with open(reference['path'], 'rb') as stream:
        raw = stream.read(bound + 1)
    if len(raw) != reference['bytes'] or hashlib.sha256(raw).hexdigest() != reference['sha256']:
        raise ValueError('Verified worker payload bytes changed')
    return json.loads(raw)


def execute(plan, *, resumed=False, emit=lambda event: None):
    # Callers must validate activation(settings) before preparing or resuming this plan.
    # Rust Prepare gates new requests; Resume keeps its original retained binding.
    if not resumed:
        # The caller checkpoints the opaque reference before this boundary.
        _command(plan, plan['submit'])
    emit({'event': 'progress', 'unit': 'external_wait',
          'wait': {'upstream': 'flux', 'key': plan['key']}})
    observation = _command(plan, plan['attach'])['observation']
    if observation['kind'] != 'terminal':
        raise ValueError('Flux acceptance or observation is unknown; retain the original binding and input')
    terminal = observation['terminal']
    payload = _command(plan, {'op': 'worker_payload', **plan['owner'], 'terminal': terminal})['payload']
    receipt = {'backend': 'flux', 'wait_reference': plan['key'], 'run_binding': observation['binding'],
               'receipt_sha256': terminal['receipt']['sha256'], 'ending': terminal['ending']}
    if payload['kind'] == 'tests':
        return {'kind': 'tests', 'summary': _blob(payload['summary'], plan['bounds']['payload_bytes']),
                'symptom_runs': [_blob(item, plan['bounds']['payload_bytes']) for item in payload['symptom_runs']],
                'symptom_receipts': payload['symptom_runs'],
                'receipts': [receipt]}
    artifact = _blob(payload['artifact'], plan['bounds']['payload_bytes'])
    return {'kind': 'generator', 'artifact': artifact,
            'receipt': {**receipt, 'artifact_sha256': payload['artifact']['sha256'],
                        'exit_code': artifact['exit_code'], 'command': artifact.get('command'),
                        'argv': artifact.get('argv'), 'identity': artifact.get('identity'),
                        'budget': artifact.get('budget'), 'phase': artifact.get('phase'),
                        'timings': artifact.get('timings'),
                        'inner': {'status': artifact['status'], 'exit_code': artifact['exit_code']},
                        'outer': {'status': terminal['ending'], 'exit_code': None, 'elapsed_s': None}}}
