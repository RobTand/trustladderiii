#!/usr/bin/env python3
"""Exact-source native deployment admission, with private replayable evidence."""
import argparse
from contextlib import contextmanager
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import uuid

SOURCE = Path(__file__).resolve().parents[2]
PREFIX = Path('/home/rob/flux-vanilla/0.91.0-0.56.0')
FLUX = PREFIX / 'install/bin/flux'
URI = f'local://{PREFIX}/runs/fv1-model/run/local-0'
GENERATION = 'fv1-model/f107a7b075094ef6839e3a545b5dd711'
# Owner's observed native starttime binds this generation, not newest-run discovery.
STARTTIME = '1794024934.96'
STORE = Path('/mnt/shared/flux-vanilla/deploy')
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
            'cpus': cpus, 'memory_gib': memory, 'walltime_s': 7200, 'environment': environment}


def jobspec(intent, request_digest):
    out = Path(intent['out'])
    command = ['/usr/bin/python3', '-I', str(SOURCE / 'deploy/dl380g10/flux-deploy.py'),
               'payload', str(out / 'intent.json'), request_digest]
    args = ['submit', '--dry-run', '--env=-*']
    args += [f'--env={key}={value}' for key, value in sorted(intent['environment'].items())]
    args += ['-n1', f"-c{intent['cpus']}", '-t7200s', '--requires=host:dl380g10',
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
            or system['duration'] != 7200 or spec['tasks'][0]['command'] != command):
        raise RuntimeError('native environment/command/cwd/walltime mismatch')
    spec['attributes']['user'] = {'tl3_deploy_intent': request_digest, 'generation': GENERATION,
                                   'source_head': intent['head'], 'source_tree': intent['tree']}
    return spec


def validate_spec(intent, request_digest, spec):
    resources = spec['resources']
    system = spec['attributes']['system']
    tasks = spec['tasks']
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


def collect(intent, request_digest, spec, identifier):
    out = Path(intent['out'])
    if not original_matches(identifier, spec):
        raise RuntimeError('original native intent mismatch')
    wait_rc, wait, wait_error = native('job', 'wait-event', '--format=json', identifier, 'clean', timeout=7500)
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
                rc = child.wait(timeout=7150)
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
        request_digest = digest(encoded(intent))
        if (closed['schema'] != 'tl3.deploy.closed.v1'
                or closed['intent'] != request_digest or bound['intent'] != request_digest
                or closed['generation'] != GENERATION or bound['generation'] != GENERATION
                or closed['job_id'] != job_id(bound['job_id'].encode())
                or closed['owned_scope_empty'] is not True
                or closed['owned_scratch_removed'] is not True
                or set(closed['files']) != set(OBSERVED_FILES)
                or bound['jobspec_sha256'] != digest(encoded(load(out / 'jobspec.json')))):
            raise RuntimeError('inconsistent closed-job evidence')
        for name, reference in closed['files'].items():
            path = out / name
            private(path)
            with path.open('rb') as stream:
                if (hashlib.file_digest(stream, 'sha256').hexdigest() != reference['sha256']
                        or path.stat().st_size != reference['bytes']):
                    raise RuntimeError('closed-job evidence changed')
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise RuntimeError('unresolved prior dispatch; reattach its original ID') from error


@contextmanager
def serialized_admission(reattach):
    descriptor = os.open(STORE / 'deploy.lock', os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, 'a') as lock:
        private(STORE / 'deploy.lock')
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        for sibling in STORE.iterdir():
            if sibling.name == 'deploy.lock':
                continue
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
    STORE.mkdir(mode=0o700, exist_ok=True)
    private(STORE, True)
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
