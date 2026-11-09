#!/usr/bin/env python3
"""Exact-source native deployment admission, with private replayable evidence."""
import argparse
import fcntl
import hashlib
import json
import math
import os
import pwd
import re
import shutil
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import uuid
from contextlib import contextmanager
from pathlib import Path

SOURCE = Path(__file__).resolve().parents[2]
PREFIX = Path('/home/rob/flux-vanilla/0.91.0-0.56.0')
FLUX = PREFIX / 'install/bin/flux'
URI = f'local://{PREFIX}/runs/fv1-model/run/local-0'
GENERATION = 'fv1-model/f107a7b075094ef6839e3a545b5dd711'
# Owner's observed native starttime binds this generation, not newest-run discovery.
STARTTIME = '1794024934.96'
LOCK_ROOT = Path('/mnt/shared/flux-vanilla/deploy')
STORE = Path('/home/rob/.local/share/tl3/deploy-admission')
WALLTIME_S = 5400
SOURCE_STORE = Path('/home/rob/.local/share/tl3-deploy-sources')
UPSTREAM = 'https://github.com/RobTand/trustladderiii.git'
SCRATCH = Path('/var/tmp/flux-vanilla')
CGROUP_ROOT = Path('/sys/fs/cgroup')
HOME = '/home/rob'
PATH = '/home/rob/.cargo/bin:/home/rob/.local/bin:/usr/local/bin:/usr/bin:/bin'
THREADS = {key: '1' for key in ('OMP_NUM_THREADS', 'MKL_NUM_THREADS',
                              'OPENBLAS_NUM_THREADS', 'RAYON_NUM_THREADS')}
OBSERVED_FILES = ('intent.json', 'jobspec.json', 'submit.stdout', 'submit.stderr', 'binding.json',
                  'wait.json', 'wait.stderr', 'status.json', 'status.stderr', 'terminal-query.json',
                  'eventlog.jsonl', 'eventlog.stderr', 'stdout', 'stderr', 'result.json')
SUPERVISOR = {'PATH': f'{PREFIX}/install/bin:{PREFIX}/deps/bin:/usr/bin:/bin',
              'HOME': HOME, 'XDG_RUNTIME_DIR': '/run/user/1000', 'FLUX_URI': URI,
              'LD_LIBRARY_PATH': f'{PREFIX}/install/lib:{PREFIX}/deps/lib', **THREADS}


def encoded(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':')).encode()


def digest(data):
    return hashlib.sha256(data).hexdigest()


def private(path, directory=False):
    info = path.lstat()
    kind = stat.S_ISDIR if directory else stat.S_ISREG
    if not kind(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise RuntimeError('private owned evidence path required')
    for parent in path.parents:
        if parent.is_symlink():
            raise RuntimeError('symlink evidence ancestry refused')


def save(path, value):
    data = value if isinstance(value, bytes) else encoded(value)
    fd, name = tempfile.mkstemp(prefix='.write-', dir=path.parent)
    try:
        with os.fdopen(fd, 'wb') as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(name, path)
        descriptor = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
    finally:
        if os.path.lexists(name):
            os.unlink(name)


def load(path):
    private(path)
    if path.stat().st_size > 4 * 1024 * 1024:
        raise RuntimeError('oversized evidence refused')
    return json.loads(path.read_bytes())


def run(command, *, environment=None, cwd=None, timeout=40):
    # All children are bounded. Kill the owned process group on timeout or interruption.
    with subprocess.Popen([str(part) for part in command], env=environment or SUPERVISOR,
                          cwd=cwd, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, start_new_session=True) as child:
        try:
            stdout, stderr = child.communicate(timeout=timeout)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGKILL)
            stdout, stderr = child.communicate()
            return 124, stdout, stderr + b'\nbounded command timeout\n'
        except BaseException:
            os.killpg(child.pid, signal.SIGKILL)
            child.communicate()
            raise
    return child.returncode, stdout, stderr


def checked(command, **kwargs):
    rc, stdout, _ = run(command, **kwargs)
    if rc:
        raise RuntimeError('bounded command refused')
    return stdout


def source_identity(head):
    if not re.fullmatch('[0-9a-f]{40}', head):
        raise RuntimeError('full exact SHA required')
    if SOURCE != SOURCE_STORE / head:
        raise RuntimeError('deploy-owned SHA-named source outside project/tmp required')
    for path in (SOURCE, SOURCE / '.git'):
        info = path.lstat()
        if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o022:
            raise RuntimeError('independent owned clone required')
    for path in (SOURCE, *SOURCE.parents):
        if path.is_symlink():
            raise RuntimeError('symlink source refused')
    env = SUPERVISOR | {'GIT_CONFIG_NOSYSTEM': '1', 'GIT_CONFIG_GLOBAL': '/dev/null',
                        'GIT_TERMINAL_PROMPT': '0'}
    def git(*args):
        return checked(['git', *args], environment=env, cwd=SOURCE, timeout=120).strip().decode()
    if (git('rev-parse', '--show-toplevel') != str(SOURCE)
            or git('rev-parse', 'HEAD') != head
            or git('status', '--porcelain=v1', '--untracked-files=all', '--ignored')
            or git('rev-parse', '--is-shallow-repository') != 'false'
            or git('config', '--get', 'remote.origin.url') != UPSTREAM):
        raise RuntimeError('exact source identity/cleanliness/history refused')
    git('rev-list', '--objects', '--missing=error', head)
    return git('rev-parse', 'HEAD^{tree}')


def native(*args, timeout=40):
    return run([FLUX, *args], timeout=timeout)


def binding():
    for attr, expected in (('broker.starttime', STARTTIME), ('size', '3')):
        rc, value, _ = native('getattr', attr)
        if rc or value.decode().strip() != expected:
            raise RuntimeError('pinned generation/size drift or unknown')
    rc, raw, _ = native('config', 'get')
    config = json.loads(raw) if rc == 0 else {}
    execution = config.get('exec', {})
    properties = execution.get('sdexec-properties', {})
    if (execution.get('service') != 'sdexec'
            or execution.get('sdexec-constrain-resources') is not False
            or properties.get('MemoryMax') != '75%'
            or properties.get('MemorySwapMax') != '0'
            or properties.get('Slice') != 'flux-jobs.slice'):
        raise RuntimeError('sdexec containment contract drift or unknown')


def plan(head, tree, stage, lane, release, manifest, backup, out):
    cpus, memory = (8, 8) if stage == 'build' else (1, 2)
    environment = {'PATH': PATH, 'HOME': HOME, 'XDG_RUNTIME_DIR': '/run/user/1000',
                   'FLUX_SCRATCH': str(SCRATCH), **THREADS}
    if stage == 'build':
        environment.update(CARGO_BUILD_JOBS='2', CARGO_TARGET_DIR=f'/home/rob/tmp/tl3-build/deploy-cargo-target-{head}')
    else:
        environment['TL3_DEPLOY_NONCE'] = out.parent.name
    return {'schema': 'tl3.deploy.intent.v1', 'generation': GENERATION, 'uri': URI,
            'head': head, 'tree': tree, 'source': str(SOURCE), 'stage': stage, 'lane': lane,
            'release': str(release), 'manifest': str(manifest) if stage != 'build' else None,
            'backup': str(backup) if stage != 'build' else None,
            'out': str(out), 'name': f'tl3-deploy-{out.parent.name}-{stage}',
            'cpus': cpus, 'memory_gib': memory, 'walltime_s': WALLTIME_S, 'environment': environment}


def jobspec(intent, request_digest):
    out = Path(intent['out'])
    command = ['/usr/bin/python3', '-I', str(SOURCE / 'deploy/dl380g10/flux-deploy.py'),
               'payload', str(out / 'intent.json'), request_digest]
    args = ['submit', '--dry-run', '--env=-*', '--rlimit=-*']
    args += [f'--env={key}={value}' for key, value in sorted(intent['environment'].items())]
    args += ['-n1', f"-c{intent['cpus']}", f'-t{WALLTIME_S}s', '--requires=host:dl380g10',
             f"--job-name={intent['name']}", f'--cwd={SOURCE}', '--input=/dev/null',
             f'--output={out}/stdout', f'--error={out}/stderr', *command]
    rc, raw, _ = native(*args)
    if rc:
        raise RuntimeError('native jobspec generation failed')
    spec = json.loads(raw)
    # No node-count request or fixed topology. One slot containing actual memory leaves.
    resources = spec['resources']
    if (len(resources) != 1 or resources[0]['type'] != 'slot'
            or resources[0]['count'] != 1
            or resources[0]['with'] != [{'type': 'core', 'count': intent['cpus']} ]):
        raise RuntimeError('unknown native resource schema')
    resources[0]['with'].append({'type': 'memory', 'count': intent['memory_gib']})
    system = spec['attributes']['system']
    if (system['environment'] != intent['environment'] or system['cwd'] != str(SOURCE)
            or system['duration'] != WALLTIME_S or spec['tasks'][0]['command'] != command):
        raise RuntimeError('native environment/command/cwd/walltime mismatch')
    spec['attributes']['user'] = {'tl3_deploy_intent': request_digest, 'generation': GENERATION,
                                   'source_head': intent['head'], 'source_tree': intent['tree']}
    validate_spec(intent, request_digest, spec)
    return spec


def validate_spec(intent, request_digest, spec):
    resources = spec['resources']
    system = spec['attributes']['system']
    tasks = spec['tasks']
    # Check actual native shape, independently of the saved intent's numbers.
    for value, maximum in ((intent['cpus'], 8), (intent['memory_gib'], 32),
                           (intent['walltime_s'], WALLTIME_S),
                           (system['duration'], WALLTIME_S)):
        if type(value) not in (int, float) or not math.isfinite(value) or not 0 < value <= maximum:
            raise RuntimeError('owner resource limit refused')
    if (type(intent['cpus']) is not int or type(intent['memory_gib']) is not int
            or system.get('exclusive', False) is not False
            or system.get('constraints') != {'hostlist': ['dl380g10']}
            or type(resources) is not list or len(resources) != 1
            or set(resources[0]) != {'type', 'count', 'label', 'with'}
            or type(resources[0]['label']) is not str or not resources[0]['label']
            or type(resources[0]['count']) is not int
            or resources[0]['count'] != 1
            or type(resources[0]['with']) is not list
            or len(resources[0]['with']) != 2):
        raise RuntimeError('owner host/resource shape refused')
    for leaf, kind, maximum in zip(resources[0]['with'], ('core', 'memory'), (8, 32)):
        if (set(leaf) != {'type', 'count'} or leaf['type'] != kind
                or type(leaf['count']) is not int or not 0 < leaf['count'] <= maximum):
            raise RuntimeError('owner resource leaf refused')

    command = ['/usr/bin/python3', '-I', str(SOURCE / 'deploy/dl380g10/flux-deploy.py'),
               'payload', str(Path(intent['out']) / 'intent.json'), request_digest]
    if (len(resources) != 1 or resources[0]['type'] != 'slot' or resources[0]['count'] != 1
            or resources[0]['with'] != [{'type': 'core', 'count': intent['cpus']},
                                       {'type': 'memory', 'count': intent['memory_gib']}]
            or len(tasks) != 1 or tasks[0]['command'] != command
            or tasks[0]['count'] != {'per_slot': 1} or tasks[0]['slot'] != resources[0]['label']
            or system['environment'] != intent['environment'] or system['cwd'] != str(SOURCE)
            or system['duration'] != intent['walltime_s']
            or spec['attributes']['user'] != {'tl3_deploy_intent': request_digest, 'generation': GENERATION,
                                             'source_head': intent['head'], 'source_tree': intent['tree']}):
        raise RuntimeError('persisted jobspec admission/intent mismatch')


def job_id(raw):
    value = raw.decode().strip()
    if not re.fullmatch(r'(?:ƒ|f)[1-9A-HJ-NP-Za-km-z]+', value):
        raise RuntimeError('missing/malformed native ID; acceptance UNKNOWN')
    return value


def original_matches(identifier, spec):
    rc, raw, _ = native('job', 'info', '--original', identifier, 'jobspec')
    return rc == 0 and json.loads(raw) == spec


def recover(intent, spec):
    rc, raw, _ = native('jobs', '-a', '--json', f"--name={intent['name']}")
    rows = json.loads(raw)['jobs'] if rc == 0 else []
    # Zero, multiple and contradictory rows remain UNKNOWN, even if one matches.
    if len(rows) != 1:
        raise RuntimeError('native acceptance UNKNOWN; never resubmit')
    identifier = job_id(rows[0]['jobid'].encode())
    if rows[0]['name'] != intent['name'] or not original_matches(identifier, spec):
        raise RuntimeError('contradictory native acceptance UNKNOWN')
    return identifier


FAILURE_FILES = (*OBSERVED_FILES, 'failure-identity.json', 'failure-cleanup.json')


def failed_terminal(out, identifier):
    """Parse only provable FAILED states. Missing payload output is never a verdict."""
    query = load(out / 'terminal-query.json')
    identity = load(out / 'failure-identity.json')
    status_value = load(out / 'status.json')
    wait = load(out / 'wait.json')
    private(out / 'eventlog.jsonl')
    history = [json.loads(line) for line in (out / 'eventlog.jsonl').read_bytes().splitlines()]
    if not all(isinstance(value, dict) for value in (query, identity, status_value, wait)):
        raise RuntimeError('malformed FAILED terminal records')
    if (any(type(query.get(key)) is not int for key in ('wait_rc', 'event_rc', 'status_rc'))
            or query['wait_rc'] != 0 or query['event_rc'] != 0 or wait.get('name') != 'clean'
            or identity['job_id'] != identifier or identity['id_rc'] != 0
            or identity['jobid_path_rc'] != 0 or identity['jobid_path'] != '/sys'
            or type(identity['numeric_id']) is not int or identity['numeric_id'] <= 0
            or type(status_value.get('id')) is not int
            or status_value['id'] != identity['numeric_id']
            or type(status_value.get('exception_occurred')) is not bool
            or type(status_value.get('result')) is not int
            or not history):
        raise RuntimeError('FAILED terminal identity/query UNKNOWN')
    for event in history:
        if (not isinstance(event, dict) or type(event.get('name')) is not str
                or type(event.get('timestamp')) not in (int, float)
                or not math.isfinite(event['timestamp']) or event['timestamp'] < 0):
            raise RuntimeError('malformed FAILED terminal event')
    names = [event['name'] for event in history]
    times = [event['timestamp'] for event in history]
    if (names.count('submit') != 1 or names[0] != 'submit'
            or names.count('clean') != 1 or names[-1] != 'clean'
            or times != sorted(times)
            or not isinstance(history[0].get('context'), dict)
            or type(history[0]['context'].get('userid')) is not int
            or history[0]['context']['userid'] != os.getuid()
            or any(names.count(name) > 1 for name in ('alloc', 'start', 'finish', 'exception'))):
        raise RuntimeError('contradictory FAILED terminal history')
    exceptions = [event['context'] for event in history if event['name'] == 'exception']
    exception = exceptions[0] if exceptions else None
    if exception is not None:
        if (not isinstance(exception, dict) or type(exception.get('type')) is not str or not exception['type']
                or type(exception.get('severity')) is not int or exception['severity'] != 0
                or status_value['exception_occurred'] is not True
                or status_value.get('exception_type') != exception['type']
                or type(status_value.get('exception_severity')) is not int
                or status_value['exception_severity'] != 0):
            raise RuntimeError('FAILED exception mismatch')
    elif status_value['exception_occurred'] or any(
            key in status_value for key in ('exception_type', 'exception_severity')):
        raise RuntimeError('FAILED exception missing')
    expected_result = {'cancel': 4, 'timeout': 8}.get(exception['type'], 2) if exception else 2
    if status_value['result'] != expected_result:
        raise RuntimeError('FAILED result mismatch')
    if 'start' not in names:
        if ('alloc' in names or 'finish' in names or exception is None
                or type(status_value.get('waitstatus', -1)) is not int
                or status_value.get('waitstatus', -1) != -1):
            raise RuntimeError('pre-start FAILED state UNKNOWN')
        phase, waitstatus, exit_code = 'NEVER_STARTED', None, 125
    else:
        if ('alloc' not in names or 'finish' not in names
                or not names.index('alloc') < names.index('start') < names.index('finish') < names.index('clean')):
            raise RuntimeError('started FAILED state UNKNOWN')
        waitstatus = status_value.get('waitstatus')
        finish = history[names.index('finish')]['context']['status']
        if (type(waitstatus) is not int or not 0 < waitstatus <= 65535
                or type(finish) is not int or finish != waitstatus):
            raise RuntimeError('FAILED waitstatus mismatch')
        exit_code = os.waitstatus_to_exitcode(waitstatus)
        if exit_code < 0:
            exit_code = 128 - exit_code
        if exit_code == 0:
            raise RuntimeError('missing payload result cannot prove FAILED')
        if exception:
            exit_code = max(exit_code, 125)
        if exception and not names.index('start') < names.index('exception') < names.index('finish'):
            raise RuntimeError('FAILED exception ordering mismatch')
        phase = 'STARTED'
    if type(query['status_rc']) is not int or query['status_rc'] != exit_code:
        raise RuntimeError('FAILED status command mismatch')
    return {'phase': phase, 'waitstatus': waitstatus, 'status_rc': exit_code,
            'exception': exception}


def failed_cleanup(identifier):
    """Prove absence independently at exact sdexec names, never by native clean."""
    owner = pwd.getpwnam('rob').pw_uid
    if os.getuid() != owner or socket.gethostname().split('.')[0] != 'dl380g10':
        raise RuntimeError('rob-owned dl380g10 FAILED observer required')
    relative = Path(f'user.slice/user-{owner}.slice/user@{owner}.service/flux-jobs.slice')
    slice_path = CGROUP_ROOT / relative
    roots = {}
    for root in (CGROUP_ROOT, SCRATCH):
        owned_ancestry(root)
        info = root.lstat()
        if not stat.S_ISDIR(info.st_mode) or info.st_uid not in (0, owner):
            raise RuntimeError('FAILED cleanup root UNKNOWN')
        roots[str(root)] = {'device': info.st_dev, 'inode': info.st_ino, 'uid': info.st_uid}
    path = CGROUP_ROOT
    for part in relative.parts:
        path = path / part
        info = path.lstat()
        if not stat.S_ISDIR(info.st_mode) or info.st_uid not in (0, owner):
            raise RuntimeError('FAILED cgroup ancestry UNKNOWN')
    before = slice_path.lstat()
    if before.st_uid != owner:
        raise RuntimeError('FAILED slice ownership UNKNOWN')
    # Flux 0.91 bulk-exec labels use f58plain. sdexec adds :sys or :rank-sys.
    # The pinned generation has three brokers. Check every possible rank spelling.
    plain = 'f' + identifier[1:]
    scopes = [slice_path / f'shell-{plain}:{suffix}.service'
              for suffix in ('sys', '0-sys', '1-sys', '2-sys')]
    scratch_paths = [SCRATCH / ('ƒ' + identifier[1:]), SCRATCH / plain]
    for path in (*scopes, *scratch_paths):
        try:
            path.lstat()
        except FileNotFoundError:
            pass
        else:
            # Without a payload inode, an existing empty scope has unknown origin.
            raise RuntimeError('FAILED cgroup/scratch absence unproven')
    for root in (CGROUP_ROOT, SCRATCH):
        info = root.lstat()
        if roots[str(root)] != {'device': info.st_dev, 'inode': info.st_ino, 'uid': info.st_uid}:
            raise RuntimeError('FAILED cleanup root identity changed')
    after = slice_path.lstat()
    if (before.st_dev, before.st_ino, before.st_uid) != (after.st_dev, after.st_ino, after.st_uid):
        raise RuntimeError('FAILED slice identity changed')
    return {'schema': 'tl3.deploy.failed-cleanup.v1', 'job_id': identifier,
            'host': 'dl380g10', 'roots': roots,
            'slice': str(slice_path), 'slice_device': before.st_dev, 'slice_inode': before.st_ino,
            'scope_paths_absent': [str(path) for path in scopes],
            'scratch_paths_absent': [str(path) for path in scratch_paths]}


def close_failed_without_result(intent, request_digest, spec, identifier, numeric_id, id_rc):
    out = Path(intent['out'])
    if os.path.lexists(out / 'receipt.json'):
        raise RuntimeError('successful receipt cannot become FAILED')
    path_rc, path, _ = native('getattr', 'jobid-path')
    save(out / 'failure-identity.json', {'job_id': identifier, 'id_rc': id_rc,
         'numeric_id': int(numeric_id), 'jobid_path_rc': path_rc,
         'jobid_path': path.decode().strip()})
    terminal = failed_terminal(out, identifier)
    cleanup = failed_cleanup(identifier)
    save(out / 'failure-cleanup.json', cleanup)
    binding()
    files = {}
    for name in FAILURE_FILES:
        path = out / name
        if not os.path.lexists(path):
            if name == 'result.json' or (name in ('stdout', 'stderr') and terminal['phase'] == 'NEVER_STARTED'):
                files[name] = None
                continue
            raise RuntimeError('FAILED retained evidence missing')
        if name in ('stdout', 'stderr'):
            info = path.lstat()
            if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid():
                raise RuntimeError('unknown retained FAILED output')
            path.chmod(0o600)
        private(path)
        data = path.read_bytes()
        files[name] = {'sha256': digest(data), 'bytes': len(data)}
    save(out / 'closed.json', {'schema': 'tl3.deploy.closed.v2', 'outcome': 'FAILED',
         'intent': request_digest, 'generation': GENERATION, 'job_id': identifier,
         'jobspec_sha256': digest(encoded(spec)), 'terminal': terminal, 'files': files,
         'owned_scope_empty': True, 'owned_scratch_removed': True,
         'limitation': 'wrapper observations, no native attestation or PB CAS'})
    raise RuntimeError('native FAILED terminal; closure retained without PASS')


def collect(intent, request_digest, spec, identifier):
    out = Path(intent['out'])
    if not original_matches(identifier, spec):
        raise RuntimeError('original native intent mismatch')
    wait_rc, wait, wait_error = native('job', 'wait-event', '--format=json', identifier, 'clean', timeout=WALLTIME_S + 180)
    save(out / 'wait.json', wait)
    save(out / 'wait.stderr', wait_error)
    if wait_rc == 124:
        cancel_rc, cancel, cancel_error = native('cancel', identifier)
        save(out / 'cancel.json', {'returncode': cancel_rc})
        save(out / 'cancel.stdout', cancel)
        save(out / 'cancel.stderr', cancel_error)
        cleanup_rc, cleanup, cleanup_error = native('job', 'wait-event', '--format=json', identifier, 'clean', timeout=180)
        save(out / 'cancel-clean.json', {'returncode': cleanup_rc})
        save(out / 'cancel-clean.stdout', cleanup)
        save(out / 'cancel-clean.stderr', cleanup_error)
    # Collect every terminal stream even after wait/status failure; never cancel unrelated work.
    status_rc, status, status_error = native('job', 'status', '--json', '--exception-exit-code=125', identifier, timeout=60)
    event_rc, events, event_error = native('job', 'eventlog', '--format=json', identifier)
    save(out / 'status.json', status)
    save(out / 'status.stderr', status_error)
    save(out / 'eventlog.jsonl', events)
    save(out / 'eventlog.stderr', event_error)
    save(out / 'terminal-query.json', {'wait_rc': wait_rc, 'status_rc': status_rc, 'event_rc': event_rc})
    parsed_wait = json.loads(wait) if wait_rc == 0 else {}
    parsed = json.loads(status)
    id_rc, numeric_id, _ = native('job', 'id', '--to=dec', identifier)
    if (not os.path.lexists(out / 'result.json') and isinstance(parsed, dict)
            and parsed.get('result') in (2, 4, 8)):
        close_failed_without_result(intent, request_digest, spec, identifier, numeric_id, id_rc)
    history = [json.loads(line) for line in events.splitlines()]
    names = [event['name'] for event in history]
    finishes = [event['context']['status'] for event in history if event['name'] == 'finish']
    if (wait_rc or event_rc or not isinstance(parsed, dict)
            or parsed_wait.get('name') != 'clean'
            or names.count('clean') != 1 or names.count('start') != 1
            or id_rc or parsed.get('id') != int(numeric_id)
            or type(parsed.get('exception_occurred')) is not bool
            or type(parsed.get('waitstatus')) is not int or parsed['waitstatus'] < 0
            or type(parsed.get('result')) is not int or parsed['result'] not in (1, 2, 4, 8)
            or len(finishes) != 1 or type(finishes[0]) is not int
            or finishes != [parsed['waitstatus']]
            or names.index('start') > names.index('finish') or names.index('finish') > names.index('clean')):
        raise RuntimeError('native terminal failure/unknown; evidence retained')
    result = load(out / 'result.json')
    if (result.get('schema') != 'tl3.deploy.payload.v1' or result.get('intent') != request_digest
            or result.get('job_id') != identifier or type(result.get('returncode')) is not int
            or type(result.get('cgroup_inode')) is not int
            or result['cgroup_inode'] <= 0
            or result.get('source_tree') != intent['tree'] or result.get('scratch_removed') is not True):
        raise RuntimeError('payload receipt failure/mismatch')
    # Native clean is insufficient. Read only this payload's recorded cgroup.
    cgroup = Path(result['cgroup'])
    if (not cgroup.is_absolute() or '..' in cgroup.parts
            or cgroup.parent.name != 'flux-jobs.slice'
            or not re.fullmatch(r'shell-[^/]+:sys\.service', cgroup.name)):
        raise RuntimeError('unknown owned sdexec cgroup')
    scope = CGROUP_ROOT / cgroup.relative_to('/')
    CGROUP_ROOT.stat()
    try:
        scope_info = scope.lstat()
    except FileNotFoundError:
        scope_info = None
    if scope_info is not None:
        if not stat.S_ISDIR(scope_info.st_mode) or scope_info.st_ino != result['cgroup_inode']:
            raise RuntimeError('owned cgroup identity changed')
        if 'populated 0' not in (scope / 'cgroup.events').read_text().splitlines():
            raise RuntimeError('owned scope cleanup UNKNOWN')
    SCRATCH.stat()
    try:
        (SCRATCH / identifier).lstat()
    except FileNotFoundError:
        pass
    else:
        raise RuntimeError('owned scratch remains')
    binding()
    files = {}
    for name in OBSERVED_FILES:
        path = out / name
        if name in ('stdout', 'stderr'):
            info = path.lstat()
            if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid():
                raise RuntimeError('unknown retained output refused')
            # The enclosing private directory protects redirects from their creation.
            path.chmod(0o600)
        private(path)
        with path.open('rb') as stream:
            files[name] = {'sha256': hashlib.file_digest(stream, 'sha256').hexdigest(), 'bytes': path.stat().st_size}
    receipt = {'schema': 'tl3.deploy.observation.v1', 'intent': request_digest,
               'generation': GENERATION, 'job_id': identifier, 'files': files,
               'owned_scope_empty': True, 'owned_scratch_removed': True,
               'limitation': 'wrapper observations, no native attestation or PB CAS'}
    # Closure proves no remaining job, including after a failed command. It grants no PASS.
    save(out / 'closed.json', receipt | {'schema': 'tl3.deploy.closed.v1'})
    if (status_rc or parsed['exception_occurred'] or parsed['waitstatus'] != 0
            or parsed['result'] != 1 or 'exception' in names or result['returncode'] != 0):
        raise RuntimeError('native terminal failure; closed evidence retained')
    save(out / 'receipt.json', receipt)
    return digest(encoded(receipt))


def stage(intent):
    out = Path(intent['out'])
    out.mkdir(mode=0o700, exist_ok=True)
    private(out, True)
    request_digest = digest(encoded(intent))
    saved = out / 'intent.json'
    if saved.exists():
        if load(saved) != intent:
            raise RuntimeError('reattach intent mismatch')
    else:
        save(saved, intent)
    spec_path = out / 'jobspec.json'
    if spec_path.exists():
        spec = load(spec_path)
    else:
        spec = jobspec(intent, request_digest)
        save(spec_path, spec)
    validate_spec(intent, request_digest, spec)
    binding_path = out / 'binding.json'
    if binding_path.exists():
        bound = load(binding_path)
        if (bound['intent'] != request_digest or bound['generation'] != GENERATION
                or bound['jobspec_sha256'] != digest(encoded(spec))):
            raise RuntimeError('saved binding mismatch')
        identifier = job_id(bound['job_id'].encode())
    else:
        dispatched = out / 'dispatched.json'
        if not dispatched.exists():
            # Durable marker BEFORE transport. An interruption can never blindly resubmit.
            save(dispatched, {'intent': request_digest})
            try:
                rc, reply, error = native('job', 'submit', str(spec_path))
            except BaseException:
                save(out / 'submit.stdout', b'')
                save(out / 'submit.stderr', b'bounded transport interrupted; acceptance UNKNOWN\n')
                raise
            save(out / 'submit.stdout', reply)
            save(out / 'submit.stderr', error)
            try:
                identifier = job_id(reply) if rc == 0 else recover(intent, spec)
            except RuntimeError:
                identifier = recover(intent, spec)
        else:
            identifier = recover(intent, spec)
        save(binding_path, {'generation': GENERATION, 'job_id': identifier, 'intent': request_digest,
                            'jobspec_sha256': digest(encoded(spec))})
    try:
        return collect(intent, request_digest, spec, identifier)
    except Exception:
        retained = {}
        for name in ('status.json', 'eventlog.jsonl', 'terminal-query.json', 'result.json'):
            path = out / name
            if path.is_file() and not path.is_symlink():
                retained[name] = digest(path.read_bytes())
        save(out / 'refusal.json', {'schema': 'tl3.deploy.refusal.v1', 'intent': request_digest,
                                  'job_id': identifier, 'observed_files': retained})
        raise


def payload(intent_path, expected):
    intent = load(intent_path)
    if digest(encoded(intent)) != expected or intent['generation'] != GENERATION or intent['uri'] != URI:
        raise RuntimeError('payload intent mismatch')
    if socket.gethostname().split('.')[0] != 'dl380g10':
        raise RuntimeError('local service/artifact host dependency refused')
    tree = source_identity(intent['head'])
    if tree != intent['tree'] or str(SOURCE) != intent['source']:
        raise RuntimeError('payload source mismatch')
    identifier = job_id(checked([FLUX, 'job', 'id', '--to=f58', os.environ['FLUX_JOB_ID']]).strip())
    own = Path('/proc/self/cgroup').read_text().strip()
    if not own.startswith('0::') or '\n' in own:
        raise RuntimeError('unified owned scope required')
    cgroup = Path(own[3:])
    if cgroup.parent.name != 'flux-jobs.slice' or not re.fullmatch(r'shell-[^/]+:sys\.service', cgroup.name):
        raise RuntimeError('sdexec owned scope required')
    inode = (CGROUP_ROOT / cgroup.relative_to('/')).stat().st_ino
    filesystem = checked(['findmnt', '-n', '-o', 'FSTYPE', '-T', SCRATCH]).strip()
    if filesystem != b'btrfs' or SCRATCH.is_symlink():
        raise RuntimeError('approved scratch profile required')
    scratch = SCRATCH / identifier
    scratch.mkdir(mode=0o700)
    private(scratch, True)
    environment = intent['environment'] | {'TMPDIR': str(scratch)}
    if intent['stage'] == 'build':
        command = ['/bin/bash', SOURCE / 'deploy/dl380g10/build-release.sh', intent['release'], intent['head']]
    else:
        command = ['/usr/bin/python3', '-I', SOURCE / 'deploy/dl380g10/deploy.py', 'fresh-init',
                   '--head', intent['head'], '--lane', intent['lane'], '--binaries', intent['release'],
                   '--state-manifest', intent['manifest'], '--backup', intent['backup']]
    rc = 125
    removed = False
    try:
        # Output stays in private native redirects; no inherited environment or nested admission.
        with subprocess.Popen([str(value) for value in command], cwd=SOURCE, env=environment,
                              stdin=subprocess.DEVNULL, start_new_session=True) as child:
            try:
                rc = child.wait(timeout=WALLTIME_S - 50)
            finally:
                # Kill only this child's process group, including leftovers after parent success.
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                child.wait(timeout=10)
        if rc == 0 and source_identity(intent['head']) != tree:
            rc = 125
    finally:
        try:
            shutil.rmtree(scratch)
            removed = not os.path.lexists(scratch)
        finally:
            save(Path(intent['out']) / 'result.json', {
                'schema': 'tl3.deploy.payload.v1', 'intent': expected, 'job_id': identifier,
                'source_tree': tree, 'returncode': rc, 'scratch_removed': removed,
                'cgroup': str(cgroup), 'cgroup_inode': inode})
    if rc or not removed:
        raise RuntimeError('payload or scratch cleanup failed')


def admit_stages(head, tree, lane, release, manifest, backup, dispatch, build_dispatch=None):
    receipts = {}
    reused = dispatch / 'reused-build.json'
    prior = load(reused) if reused.exists() else None
    if prior is not None:
        if prior.get('schema') != 'tl3.deploy.reused-build.v1':
            raise RuntimeError('unknown build reuse record')
        saved_dispatch = Path(prior['source_dispatch'])
        if build_dispatch is not None and build_dispatch != saved_dispatch:
            raise RuntimeError('build reuse identity changed')
        build_dispatch = saved_dispatch
    for name in ('build', 'fresh-init'):
        build_out = dispatch / name
        if name == 'build' and build_dispatch is not None:
            if build_dispatch.parent != STORE:
                raise RuntimeError('prior build outside approved private store')
            private(build_dispatch, True)
            build_out = build_dispatch / 'build'
            private(build_out, True)
            # A prior successful build must already exist. This route cannot submit.
            private(build_out / 'binding.json')
            private(build_out / 'receipt.json')
        intent = plan(head, tree, name, lane, release, manifest, backup, build_out)
        if name == 'build' and release.exists() and not (build_out / 'binding.json').exists():
            raise RuntimeError('existing release requires an explicit verified original build dispatch')
        receipts[name] = stage(intent)
        if name == 'build' and build_dispatch is not None:
            if prior is not None and prior['receipt_sha256'] != receipts[name]:
                raise RuntimeError('reused build observation changed')
            save(reused, {'schema': 'tl3.deploy.reused-build.v1',
                 'source_dispatch': str(build_dispatch), 'receipt_sha256': receipts[name]})
    return receipts


def require_closed(out):
    try:
        closed = load(out / 'closed.json')
        intent = load(out / 'intent.json')
        bound = load(out / 'binding.json')
        identifier = job_id(bound['job_id'].encode())
        request_digest = digest(encoded(intent))
        failed = closed['schema'] == 'tl3.deploy.closed.v2'
        expected_files = FAILURE_FILES if failed else OBSERVED_FILES
        if failed:
            if (closed.get('outcome') != 'FAILED'
                    or closed.get('jobspec_sha256') != bound['jobspec_sha256']
                    or os.path.lexists(out / 'receipt.json')
                    or closed['files'].get('result.json', False) is not None
                    or closed['terminal'] != failed_terminal(out, identifier)
                    or load(out / 'failure-cleanup.json') != failed_cleanup(identifier)):
                raise RuntimeError('inconsistent FAILED closure')
        if (closed['schema'] not in ('tl3.deploy.closed.v1', 'tl3.deploy.closed.v2')
                or closed['intent'] != request_digest or bound['intent'] != request_digest
                or closed['generation'] != GENERATION or bound['generation'] != GENERATION
                or closed['job_id'] != identifier
                or closed['owned_scope_empty'] is not True
                or closed['owned_scratch_removed'] is not True
                or set(closed['files']) != set(expected_files)
                or bound['jobspec_sha256'] != digest(encoded(load(out / 'jobspec.json')))):
            raise RuntimeError('inconsistent closed-job evidence')
        for name, reference in closed['files'].items():
            path = out / name
            if failed and reference is None:
                if (name not in ('result.json', 'stdout', 'stderr')
                        or (name != 'result.json' and closed['terminal']['phase'] != 'NEVER_STARTED')
                        or os.path.lexists(path)):
                    raise RuntimeError('FAILED absent evidence changed')
                continue
            private(path)
            with path.open('rb') as stream:
                if (hashlib.file_digest(stream, 'sha256').hexdigest() != reference['sha256']
                        or path.stat().st_size != reference['bytes']):
                    raise RuntimeError('closed-job evidence changed')
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise RuntimeError('unresolved prior dispatch; reattach its original ID') from error


def owned_ancestry(path):
    owner = pwd.getpwnam('rob').pw_uid
    for parent in path.parents:
        try:
            info = parent.lstat()
        except FileNotFoundError:
            continue
        if (not stat.S_ISDIR(info.st_mode) or info.st_uid not in (0, owner)
                or (info.st_mode & 0o022 and not info.st_mode & stat.S_ISVTX)):
            raise RuntimeError('unsafe admission ancestry')


def lock_layout():
    # Never repair an existing invalid layout or put evidence/temporary files here.
    owner = pwd.getpwnam('rob').pw_uid
    if os.getuid() != owner:
        raise RuntimeError('rob-owned coordinator required')
    owned_ancestry(LOCK_ROOT)
    try:
        LOCK_ROOT.mkdir(mode=0o755)
    except FileExistsError:
        pass
    else:
        # mkdir honors the coordinator's private umask; only a new directory is set.
        LOCK_ROOT.chmod(0o755)
    info = LOCK_ROOT.lstat()
    if (not stat.S_ISDIR(info.st_mode) or info.st_uid != owner
            or stat.S_IMODE(info.st_mode) != 0o755):
        raise RuntimeError('owner lock directory must be rob-owned 0755')
    if any(path.name != 'deploy.lock' for path in LOCK_ROOT.iterdir()):
        raise RuntimeError('foreign owner lock entry refused')


@contextmanager
def serialized_admission(reattach):
    lock_layout()
    path = LOCK_ROOT / 'deploy.lock'
    descriptor = os.open(path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW | os.O_NONBLOCK, 0o600)
    with os.fdopen(descriptor, 'a') as lock:
        private(path)
        info = os.fstat(lock.fileno())
        named = path.lstat()
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                or info.st_mode & 0o077 or info.st_nlink != 1
                or (info.st_dev, info.st_ino) != (named.st_dev, named.st_ino)):
            raise RuntimeError('lock identity refused')
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        lock_layout()
        private(STORE, True)
        for sibling in STORE.iterdir():
            private(sibling, True)
            if sibling == reattach:
                continue
            for name in ('build', 'fresh-init'):
                out = sibling / name
                if out.exists() or out.is_symlink():
                    private(out, True)
                    if any(os.path.lexists(out / marker) for marker in ('dispatched.json', 'binding.json')):
                        require_closed(out)
        yield


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='mode', required=True)
    admitted = sub.add_parser('admit')
    admitted.add_argument('head')
    admitted.add_argument('lane', choices=['dev', 'release'])
    admitted.add_argument('manifest', type=Path)
    admitted.add_argument('backup', type=Path)
    replay = admitted.add_mutually_exclusive_group()
    replay.add_argument('--reattach', type=Path)
    replay.add_argument('--build-dispatch', type=Path)
    job = sub.add_parser('payload')
    job.add_argument('intent', type=Path)
    job.add_argument('digest')
    args = parser.parse_args()
    os.umask(0o077)
    if args.mode == 'payload':
        payload(args.intent, args.digest)
        return
    tree = source_identity(args.head)
    binding()
    # Also guard direct invocation. Dry-run in the canonical shell never bypasses its gate.
    checked(['/usr/bin/python3', '-I', SOURCE / 'deploy/dl380g10/deploy.py', 'qualify-fresh',
             '--head', args.head, '--lane', args.lane], cwd=SOURCE, timeout=610)
    # Reattaching a submitted fresh job must collect its original ID even when
    # that job has already created the backup. It cannot make a new submission.
    if args.reattach is None or not any(os.path.lexists(args.reattach / 'fresh-init' / marker)
                                      for marker in ('dispatched.json', 'binding.json')):
        checked(['/usr/bin/python3', '-I', SOURCE / 'deploy/dl380g10/deploy.py', 'preflight-fresh-budget',
                 '--head', args.head, '--state-manifest', args.manifest, '--backup', args.backup],
                cwd=SOURCE, timeout=610)
    owned_ancestry(STORE)
    STORE.mkdir(mode=0o700, parents=True, exist_ok=True)
    private(STORE, True)
    if stat.S_IMODE(STORE.lstat().st_mode) != 0o700:
        raise RuntimeError('private evidence store must be 0700')
    with serialized_admission(args.reattach):
        dispatch = args.reattach or STORE / uuid.uuid4().hex
        if dispatch.parent != STORE:
            raise RuntimeError('dispatch outside approved private store')
        if args.reattach is None:
            dispatch.mkdir(mode=0o700)
        private(dispatch, True)
        print(f'Deployment evidence/reattach: {dispatch}', flush=True)
        release = Path(f'/home/rob/tmp/tl3-build/release-{args.head}')
        receipts = admit_stages(args.head, tree, args.lane, release, args.manifest,
                                args.backup, dispatch, args.build_dispatch)
        save(dispatch / 'accepted.json', {'schema': 'tl3.deploy.cycle.v1', 'head': args.head,
                                         'tree': tree, 'receipts': receipts})


if __name__ == '__main__':
    try:
        main()
    except Exception:
        # Exception text and native diagnostic content can carry private operational data.
        sys.exit('Flux deployment refused/UNKNOWN; private dispatch retains evidence')
