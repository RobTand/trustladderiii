"""Private PrismaBuild channels, exercised with contract-compatible CLI fakes."""
import json
import subprocess
import sys

import pytest

from tl3_harness.supervision import ExecutionFailure, ExecutionOutcome
from tl3_harness.test_tool import Summary
from tl3_workers import pb_tests
import pb_fixtures as shared
from pb_fixtures import IMAGE, KEY, call, configured_generator

submission = shared.submission
generator_repo = shared.generator_repo


# Prefix the existing fake CLIs so their submission/ending bodies still exercise
# receipt and artifact validation. Only the transport changes.
CHANNEL = r'''
import builtins
import fcntl
import json
import os
from pathlib import Path
import signal
import stat
import sys
here = Path(__file__).parent
name = Path(__file__).stem
config_path = here / (name + '-channel.json')
config = json.loads(config_path.read_text()) if config_path.exists() else {}
if sys.argv[1:] == ['--machine-record-capability']:
    with (here / (name + '-probes')).open('a') as probes:
        probes.write('probe\n')
    print(config.get('capability', json.dumps({
        'schema': 'prismabuild.machine_record_capability.v1', 'machine_record_fd': 1})))
    raise SystemExit(config.get('capability_rc', 0))
fd = None
if '--machine-record-fd' in sys.argv:
    index = sys.argv.index('--machine-record-fd')
    fd = int(sys.argv[index + 1])
    del sys.argv[index:index + 2]
    info = os.fstat(fd)
    assert fd > 2 and stat.S_ISREG(info.st_mode) and info.st_size == 0
    assert info.st_uid == os.getuid() and stat.S_IMODE(info.st_mode) == 0o600
    assert fcntl.fcntl(fd, fcntl.F_GETFL) & os.O_ACCMODE != os.O_RDONLY
    assert os.lseek(fd, 0, os.SEEK_CUR) == 0
    assert all((info.st_dev, info.st_ino) != (os.fstat(other).st_dev, os.fstat(other).st_ino)
               for other in (1, 2))
    fcntl.fcntl(fd, fcntl.F_SETFD, fcntl.FD_CLOEXEC)
if name == 'pbrun':
    assert '--detach' in sys.argv
if config.get('refusal'):
    print('private diagnostic must not leak: token=fixture-secret', file=sys.stderr)
    raise SystemExit(2)
original_print = builtins.print

def machine_print(document, *args, **kwargs):
    record = json.loads(document)
    raw = (document + '\n').encode()
    mode = config.get('channel', 'valid')
    if mode == 'empty':
        raw = b''
    elif mode == 'unterminated':
        raw = raw[:-1]
    elif mode == 'malformed':
        raw = b'{token=fixture-secret\n'
    elif mode == 'multiple':
        raw += raw
    elif mode == 'utf8':
        raw = b'\xff\n'
    elif mode == 'oversized':
        raw = b' ' * 2097153 + b'\n'
    if fd is not None:
        os.write(fd, raw)
    if config.get('killed'):
        os.kill(os.getpid(), signal.SIGKILL)
    public = config.get('public', 'record')
    if public == 'noise':
        original_print('public evidence, not a machine record')
    elif public == 'conflict':
        if name == 'pbrun':
            record['action_key'] = 'c' * 64
        else:
            record['endings'][0]['action_key'] = 'c' * 64
        original_print(json.dumps(record))
    else:
        original_print(document)
    raise SystemExit(config.get('rc', 0))

builtins.print = machine_print
'''


@pytest.fixture
def private_backend(submission):
    for name in ('pbrun', 'pbwait'):
        script = submission[1] / (name + '.py')
        script.write_text(CHANNEL + script.read_text())
    return submission


def configure(scripts, name, **config):
    (scripts / (name + '-channel.json')).write_text(json.dumps(config))


@pytest.fixture(params=['tests', 'generator'])
def consumer(request, private_backend, generator_repo, tmp_path):
    root, scripts = private_backend
    if request.param == 'tests':
        return lambda: call(private_backend, Summary().result), scripts
    # The fake pbrun executes the staged trusted generator with its configured
    # interpreter, and publishes the same verified log receipt as fake pbwait.
    script = scripts / 'pbrun.py'
    source = script.read_text().replace(
        'assert (root / "checkout/uncommitted.py").read_bytes() == b"current dirty bytes\\x00\\xff"', '')
    source = source.replace('print(json.dumps({"schema":', '''
sys.path.insert(0, str(root / 'trusted'))
from tl3_harness import generator
generator.require_admission = lambda: None
generator.container_command = lambda tree, container, argv, cidfile: argv
contract = json.loads((root / 'generator.json').read_text())
artifact = generator.run(root / 'checkout', generator.Container(''' + repr(IMAGE) + '''), **contract)
raw = (generator.PREFIX + json.dumps(artifact) + '\\n').encode()
log = here / 'generator.log'
log.write_bytes(raw)
import hashlib
(here / 'override.json').write_text(json.dumps({'result': {
    'verified': True, 'receipt_sha256': 'b' * 64, 'stdout': {
        'path': str(log), 'bytes': len(raw), 'sha256': hashlib.sha256(raw).hexdigest()}}}))
print(json.dumps({"schema":''')
    script.write_text(source)
    (scripts / 'response.json').write_text(json.dumps(Summary().result))

    def generate():
        with configured_generator(generator_repo, tmp_path)[0] as tree:
            from tl3_workers.pb_generators import make_generator_tool
            tree.test_settings = pb_tests.TestSettings(IMAGE, scripts)
            return make_generator_tool(tree)()
    return generate, scripts


def test_private_records_and_existing_receipts(consumer):
    run, scripts = consumer
    for name in ('pbrun', 'pbwait'):
        configure(scripts, name, public='noise')
    result = run()
    assert result['status'] == 'completed', result
    assert result['action_key'] == KEY


@pytest.mark.parametrize('name', ['pbrun', 'pbwait'])
@pytest.mark.parametrize('mode', ['empty', 'unterminated', 'malformed', 'multiple', 'utf8', 'oversized'])
def test_channel_failures_are_typed_and_safe(consumer, name, mode):
    run, scripts = consumer
    configure(scripts, name, channel=mode)
    result = run()
    assert result['status'] in ('infrastructure_error', 'failed'), result
    assert result.get('execution', {}).get('outcome') == 'transport_failure'
    assert result.get('execution', {}).get('phase') == 'prismabuild_channel'
    assert 'fixture-secret' not in json.dumps(result)


@pytest.mark.parametrize('name', ['pbrun', 'pbwait'])
@pytest.mark.parametrize('capability,rc', [
    ('', 2), ('{}', 0),
    ('{"schema":"wrong","machine_record_fd":1}', 0),
    ('{"schema":"prismabuild.machine_record_capability.v1","machine_record_fd":0}', 0),
    ('{"schema":"prismabuild.machine_record_capability.v1","machine_record_fd":true}', 0),
    ('{"schema":"prismabuild.machine_record_capability.v1","machine_record_fd":1.0}', 0),
    ('{"schema":"prismabuild.machine_record_capability.v1","machine_record_fd":1}\nnoise', 0),
    ('{"schema":"prismabuild.machine_record_capability.v1","machine_record_fd":1}', 2),
])
def test_unsupported_runtime_refuses_without_stdout_fallback(consumer, name, capability, rc):
    run, scripts = consumer
    configure(scripts, name, capability=capability, capability_rc=rc)
    result = run()
    assert result['status'] in ('infrastructure_error', 'failed'), result
    assert result.get('execution', {}).get('termination_reason') == 'machine_record_unsupported'
    assert not (scripts / ('submitted.json' if name == 'pbrun' else 'waited.json')).exists()



@pytest.mark.parametrize('name', ['pbrun', 'pbwait'])
def test_runtime_without_capability_flag_refuses_safely(consumer, name):
    run, scripts = consumer
    (scripts / (name + '.py')).write_text(
        'import argparse\n'
        'from pathlib import Path\n'
        'parser = argparse.ArgumentParser(prog="fixture-secret-old-runtime")\n'
        'parser.add_argument("--detach", action="store_true")\n'
        'parser.add_argument("--endings", action="store_true")\n'
        'parser.parse_args()\n'
        'Path(__file__).with_suffix(".executed").touch()\n')
    result = run()
    assert result['status'] in ('infrastructure_error', 'failed'), result
    assert result['execution']['outcome'] == 'transport_failure'
    assert result['execution']['phase'] == 'prismabuild_capability'
    assert result['execution']['termination_reason'] == 'machine_record_unsupported'
    assert 'fixture-secret' not in json.dumps(result)
    assert not (scripts / (name + '.executed')).exists()
    assert not (scripts / ('submitted.json' if name == 'pbrun' else 'waited.json')).exists()


def test_public_stdout_is_not_machine_data(consumer):
    run, scripts = consumer
    for name in ('pbrun', 'pbwait'):
        configure(scripts, name, public='conflict')
    result = run()
    assert result['status'] == 'completed', result
    assert result['action_key'] == KEY


@pytest.mark.parametrize('name', ['pbrun', 'pbwait'])
def test_killed_child_is_channel_failure_even_with_valid_record(consumer, name):
    run, scripts = consumer
    configure(scripts, name, killed=True)
    result = run()
    assert result.get('execution', {}).get('phase') == 'prismabuild_channel', result
    assert result.get('execution', {}).get('termination_reason') == 'machine_record_command_killed'


def test_empty_submission_refusal_keeps_exit_diagnostic(consumer):
    run, scripts = consumer
    configure(scripts, 'pbrun', refusal=True)
    result = run()
    assert result.get('execution', {}).get('termination_reason') == 'submission_refused_exit_2', result
    assert 'fixture-secret' not in json.dumps(result)
    assert not (scripts / 'waited.json').exists()


@pytest.mark.parametrize('supported', [True, False])
def test_capability_is_cached_per_runtime_path(private_backend, supported):
    scripts = private_backend[1]
    configure(scripts, 'pbrun', capability_rc=0 if supported else 2)
    command = [sys.executable, str(scripts / 'pbrun.py'), '--detach']
    for _ in range(2):
        # Invalid action args are intentional; the capability probe still has to
        # happen once, before any attempted submission.
        try:
            pb_tests._json_command(command)
        except (ExecutionFailure, ValueError):
            pass
    assert (scripts / 'pbrun-probes').exists(), 'the runtime capability was never queried'
    assert (scripts / 'pbrun-probes').read_text().splitlines() == ['probe']


@pytest.mark.parametrize('script', ['pbrun.py', 'pbwait.py'])
def test_receipt_fake_cli_capability_branch(submission, script):
    _, scripts = submission
    result = subprocess.run([sys.executable, str(scripts / script), '--machine-record-capability'],
                            capture_output=True, text=True, check=False)
    assert result.returncode == 0, result.stderr
    assert result.stdout == (
        '{"schema":"prismabuild.machine_record_capability.v1","machine_record_fd":1}\n')
    assert pb_tests._machine_record_supported(str((scripts / script).resolve()))
    capability = json.loads(result.stdout)
    assert capability == {
        'schema': 'prismabuild.machine_record_capability.v1', 'machine_record_fd': 1}
    assert type(capability['machine_record_fd']) is int
    assert not (scripts / 'submitted.json').exists()
    assert not (scripts / 'waited.json').exists()


def test_reader_raises_typed_channel_error(private_backend):
    root, scripts = private_backend
    (scripts / 'pbrun.py').write_text(CHANNEL + 'print(json.dumps({"schema": "prismabuild.submission.v1"}))\n')
    configure(scripts, 'pbrun', channel='malformed')
    with pytest.raises(ExecutionFailure) as caught:
        pb_tests._json_command([sys.executable, str(scripts / 'pbrun.py'), '--detach', '--cwd', str(root)])
    assert caught.value.outcome == ExecutionOutcome.TRANSPORT_FAILURE
