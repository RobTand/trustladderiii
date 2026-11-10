const CLIENT_FIXTURE: &str = r######"#!/usr/bin/env python3
"""Run pushed commits on BuildBuddy; credentials stay in HTTP headers.

bbrun.py [test|query|step] --repo URL --commit SHA --branch BRANCH
  --timeout MINUTES [--detach] [--full-history] -- COMMAND...
bbrun.py cancel INVOCATION_ID --expect-commit FULL_SHA --expect-repo EXACT_URL
  [--protect-invocation UUID ...]
bbrun.py wait INVOCATION_ID [--max-wait SECONDS] [--log-file PATH]
bbrun.py artifacts INVOCATION_ID [--fetch NAME --out PATH]
bbrun.py --selftest

TL3 uses cloud actions. PrismaBuild, PrismaQuant and Tessera use our self-hosted
CPU pools, chosen by --arch, with no automatic cloud fallback. --cloud is an
explicit coordinator-approved override. Remote Bazel runners stay in cloud.
Legacy invocations without a mode still run bazel test. Agent test commands
require --jobs=16 after configs. Step commands exec through an environment
allowlist. A repo script may skip its own secret scan only when
BBRUN_FINAL_SCAN=1. This non-secret marker is set only for a command in a
request ending with the key-holding scan; the scanner itself is never marked.
The runner uploads after each API step. The command wrapper moves its output
outside the upload tree before returning; the last step moves it into its own
BUILDBUDDY_ARTIFACTS_DIRECTORY and scans there before the runner uploads it.
Before test/query/step submission, /home/rob/tmp/bazel-wave2/GATE_WINDOW
pauses new runs while its mtime is under 45 minutes old. Recheck every 60
seconds, with a 45-minute total wait cap; warn and ignore stale files.
Wait/artifacts and runs already in flight are unaffected. A critical full-gate
launcher creates the file just before launching, writes owner and reason on
its first line, and removes it when the gate ends. There is no bypass flag.
Only selftests override the gate path and clocks through injected arguments.
Exit 0 means terminal success, except detach and artifact operations;
exit 1 means failure, and exit 2 means still running and safe to resume,
or a local refusal of an oversized action before submission.
Normal client modes never start a local child process. The offline selftest
executes generated scanner steps using fake credentials only.
Run requests whose estimated base64 workflow action exceeds 122880 bytes
are refused locally with exit 2. Pass long target lists with --target_pattern_file.
"""
import argparse
import base64
import gzip
import hashlib
import http.client
import inspect
import json
import os
from pathlib import Path
import re
import shlex
import sys
import time
import urllib.error
import urllib.request
import uuid

API = 'https://app.buildbuddy.io/api/v1/'
IMAGE = ('docker://gcr.io/flame-public/rbe-ubuntu24-04@sha256:'
         '5f0c01e5f6d79e58d7a996330de33aa38ce599bf8431eaf27521e7602daadf50')
TERMINAL = {'COMPLETE_INVOCATION_STATUS', 'DISCONNECTED_INVOCATION_STATUS'}
GATE_WINDOW = Path('/home/rob/tmp/bazel-wave2/GATE_WINDOW')
MAX_SERIALIZED_ACTION_BYTES = 122880


class RemoteError(RuntimeError):
    pass


class ActionTooLarge(RemoteError):
    pass


def check_action_size(data):
    """Estimate base64 YAML size as 4 * ceil((JSON request bytes + 8192) / 3).

    Count the exact outgoing JSON bytes, including every secured step, env,
    platform property and other field. JSON includes string escaping and fields
    outside the action, so it is a conservative proxy for action text. Add 8 KiB
    before base64 expansion for YAML layout and server-added action fields.
    The 120 KiB limit leaves room below Linux's 128 KiB per-argument ceiling
    for --serialized_action= and further serialization overhead.
    """
    estimate = 4 * ((len(data) + 8192 + 2) // 3)
    if estimate > MAX_SERIALIZED_ACTION_BYTES:
        raise ActionTooLarge(
            f'estimated serialized workflow action is {estimate} bytes; '
            f'limit is {MAX_SERIALIZED_ACTION_BYTES} bytes; '
            'pass long target lists with --target_pattern_file')


class BuildBuddy:
    def __init__(self, key):
        self.key = key

    def _error_text(self, text, limit):
        text = str(text)
        if self.key:
            text = text.replace(self.key, '[key]')
        return text[:limit]

    def call(self, method, body, timeout=120, raw=False, strict=False):
        data = json.dumps(body).encode()
        if self.key.encode() in data:
            raise RemoteError('credential found in API request body')
        if method == 'Run':
            check_action_size(data)
        request = urllib.request.Request(
            ('https://app.buildbuddy.io/rpc/BuildBuddyService/' + method
             if method in ('GetExecution', 'CancelExecutions') else API + method), data=data,
            headers={'x-buildbuddy-api-key': self.key, 'Content-Type': 'application/json'})
        try:
            with urllib.request.urlopen(request, timeout=timeout) as response:
                return response.read() if raw else json.load(
                    response, object_pairs_hook=cancel_unique_fields if strict else None)
        except urllib.error.HTTPError as error:
            reason = self._error_text(error.reason, 512)
            try:
                excerpt = error.read(4096).decode('utf-8', errors='replace')
            except Exception:
                excerpt = '<body unavailable>'
            finally:
                try:
                    error.close()
                except Exception:
                    pass
            detail = (f'BuildBuddy {method} failed (HTTP {error.code} {reason}): '
                      + self._error_text(excerpt, 1024))
            raise RemoteError(self._error_text(detail, 2048)) from None
        except urllib.error.URLError as error:
            detail = f'BuildBuddy {method} failed (URLError): ' + self._error_text(error.reason, 512)
            raise RemoteError(self._error_text(detail, 2048)) from None
        except (TimeoutError, ValueError) as error:
            raise RemoteError(f'BuildBuddy {method} failed ({type(error).__name__})') from None


def child_env(environ):
    return {
        name: value for name, value in environ.items()
        if (name in ('PATH', 'HOME', 'TMPDIR', 'LANG', 'VIRTUAL_ENV',
                     'BUILDBUDDY_ARTIFACTS_DIRECTORY', 'BBRUN_FINAL_SCAN')
            or name.startswith(('LC_', 'UV_', 'PYTHON')))
        and not name.endswith(('_API_KEY', '_TOKEN'))
    }


SCANNER_SHA256 = 'decd9ce671299ece1b245811af459349ed9932ee9b81b2c4c7a13cf96b94f609'
LOG_FLAG = re.compile(r'--(?:build_event_(?:json|binary|text)_file|command_log|execution_log_[\w]+)(?==|\s|$)')


def checked_scanner():
    try:
        data = Path(__file__).with_name('check_no_secret_leak.py').read_bytes()
    except OSError:
        raise RemoteError('artifact scanner unavailable; refusing submission') from None
    if hashlib.sha256(data).hexdigest() != SCANNER_SHA256:
        raise RemoteError('artifact scanner hash mismatch; refusing submission')
    return data


def mentions_artifacts(text):
    if re.search(r'\$(?:BUILDBUDDY_ARTIFACTS_DIRECTORY\b|\{BUILDBUDDY_ARTIFACTS_DIRECTORY\})', text):
        return True
    if re.search(r'(?i)(?:^|/)(?:buildbuddy[_-])?artifacts(?:/|$)', text):
        return True
    roots = {os.environ.get('BUILDBUDDY_ARTIFACTS_DIRECTORY', ''),
             '/buildbuddy_artifacts', '/buildbuddy/artifacts'} - {''}
    return any(re.search(re.escape(root.rstrip('/')) + r'(?:/|$|[\s\x27\x22;])', text)
               for root in roots)


def refuse_artifact_logs(command, mode):
    text = ' '.join(command)
    if mode == 'step':
        if mentions_artifacts(text) and (LOG_FLAG.search(text) or (
                re.search(r'\b(?:cp|mv|copy|move)\b', text) and
                re.search(r'(?i)(bep|build[_-]?event|command[_.-]?log|command\.log)', text))):
            raise RemoteError('BEP, command and execution logs are forbidden in the artifacts directory')
        return
    for index, argument in enumerate(command):
        if LOG_FLAG.match(argument):
            value = argument.split('=', 1)[1] if '=' in argument else (
                command[index + 1] if index + 1 < len(command) else '')
            if mentions_artifacts(value):
                raise RemoteError('BEP, command and execution logs are forbidden in the artifacts directory')


SELFHOSTED_REPOS = {'prismaquant', 'tessera', 'prismabuild'}


def repo_name(repo):
    return repo.rstrip('/').rsplit('/', 1)[-1].removesuffix('.git').lower()


def agent_command(command, repo, cloud=False, arch="amd64"):
    """Keep TL3 cloud capacity separate from the other repositories."""
    if repo_name(repo) not in SELFHOSTED_REPOS or cloud or command[:2] != ['bazel', 'test']:
        return command
    if any(arg.startswith(('--remote_header', '--remote_exec_header', '--remote_run_header',
                           '--remote_default_exec_properties', '--remote_executor',
                           '--remote_local_fallback', '--spawn_strategy', '--strategy',
                           '--test_strategy')) for arg in command):
        raise RemoteError('pool routing overrides are forbidden; request coordinator approval for --cloud')
    return command + [
        '--remote_executor=grpcs://remote.buildbuddy.io',
        '--remote_local_fallback=false',
        '--spawn_strategy=remote,local',
        '--remote_exec_header=x-buildbuddy-platform.use-self-hosted-executors=true',
        f'--remote_exec_header=x-buildbuddy-platform.Pool=selfhosted-{arch}',
    ]


def run_and_record(command, mode, result_path):
    """Runner-only command/handoff, or shared support when command is None.

    Support lives here so inspect.getsource callers need only the existing
    child_env/run_and_record/scan_artifacts extraction set, including Kani.
    """
    import json
    import shutil
    import stat
    import subprocess
    from pathlib import Path

    class Support:
        receipt_name = 'bbrun-wrapper-result.v1.json'

        def __init__(self):
            self.state = Path(result_path + '.private')
            self.cleanup_roots = {Path(result_path + '.artifacts')}

        def directory(self, path, *, shared_parent=False):
            # Open every component without following symlinks. No realpath or
            # check-then-chmod path resolution crosses the ownership boundary.
            path = Path(path)
            if not path.is_absolute() or '..' in path.parts or path == Path('/'):
                raise ValueError('unsafe_root')
            fd = os.open('/', os.O_RDONLY | os.O_DIRECTORY)
            try:
                for name in path.parts[1:]:
                    next_fd = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                                      dir_fd=fd)
                    os.close(fd)
                    fd = next_fd
                info = os.fstat(fd)
                if info.st_uid != os.getuid() and not (shared_parent and info.st_uid == 0 and info.st_mode & stat.S_ISVTX):
                    raise ValueError('unowned_root')
                return fd
            except BaseException:
                os.close(fd)
                raise

        def write(self, path, value):
            # Wrapper state never lives inside the command-owned artifact tree.
            fd = self.directory(self.state)
            try:
                name = Path(path).name
                temporary = name + '.new'
                out = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                              0o600, dir_fd=fd)
                with os.fdopen(out, 'w') as stream:
                    json.dump(value, stream, sort_keys=True, separators=(',', ':'))
                os.replace(temporary, name, src_dir_fd=fd, dst_dir_fd=fd)
            finally:
                os.close(fd)

        def read(self):
            fd = self.directory(self.state)
            try:
                child = os.open('command.json', os.O_RDONLY | os.O_NOFOLLOW, dir_fd=fd)
                with os.fdopen(child) as stream:
                    value = json.load(stream)
                code = value['exit_code']
                if code is not None and (type(code) is not int or not -255 <= code <= 255):
                    raise ValueError('invalid_command_state')
                if value['handoff'] not in ('pass', 'fail', 'unknown'):
                    raise ValueError('invalid_command_state')
                cleanup = value['cleanup']
                allowed = {'pass': (True, 'cleanup_complete'),
                           'not_needed': (False, 'no_cleanup_targets')}
                if cleanup['status'] in allowed:
                    present, reason = allowed[cleanup['status']]
                    if cleanup['targets_present'] is not present or cleanup['reason'] != reason:
                        raise ValueError('invalid_cleanup_state')
                elif cleanup['status'] not in ('fail', 'unknown') or cleanup['reason'] not in (
                        'cleanup_permission_denied', 'cleanup_io_error', 'cleanup_not_observed'):
                    raise ValueError('invalid_cleanup_state')
                return value
            finally:
                os.close(fd)

        def remove(self, path):
            path = Path(path)
            if path not in self.cleanup_roots:
                raise ValueError('cleanup_outside_scratch')
            anchor = self.directory(path.parent, shared_parent=True)
            name = path.name
            try:
                info = os.stat(name, dir_fd=anchor, follow_symlinks=False)
                if info.st_uid != os.getuid() or stat.S_ISLNK(info.st_mode):
                    raise ValueError('unsafe_cleanup_root')
                if not stat.S_ISDIR(info.st_mode) or not shutil.rmtree.avoids_symlink_attacks:
                    raise ValueError('unsafe_cleanup_root')

                retried = set()

                def retry(function, failed, error):
                    identity = (function, str(failed))
                    if identity in retried:
                        raise error
                    retried.add(identity)
                    if not isinstance(error, PermissionError):
                        raise error
                    relative = Path(failed)
                    if relative.is_absolute() or '..' in relative.parts or relative.parts[0] != name:
                        raise ValueError('cleanup_escape')
                    # Only directory permissions affect POSIX unlink/rmdir.
                    # Opened fds pin the owned inode; leaf symlinks are never chmodded.
                    parent = os.dup(anchor)
                    try:
                        for component in relative.parts[:-1]:
                            child = os.open(component, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                                            dir_fd=parent)
                            os.close(parent)
                            parent = child
                        if os.fstat(parent).st_uid != os.getuid():
                            raise ValueError('unowned_cleanup_parent')
                        if function in (os.unlink, os.rmdir):
                            os.fchmod(parent, stat.S_IMODE(os.fstat(parent).st_mode) | stat.S_IWUSR | stat.S_IXUSR)
                            function(relative.name, dir_fd=parent)
                        else:
                            # A failed traversal is retried only for that subtree.
                            child = os.open(relative.name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                                            dir_fd=parent)
                            try:
                                if os.fstat(child).st_uid != os.getuid():
                                    raise ValueError('unowned_cleanup_directory')
                                os.fchmod(child, stat.S_IMODE(os.fstat(child).st_mode) | stat.S_IRUSR | stat.S_IXUSR)
                            finally:
                                os.close(child)
                            erase(str(relative))
                    finally:
                        os.close(parent)

                def erase(relative):
                    # Python 3.11 has onerror; 3.12+ has onexc. Both retain the
                    # symlink-safe fd implementation and bounded retry callback.
                    import inspect
                    if 'onexc' in inspect.signature(shutil.rmtree).parameters:
                        shutil.rmtree(relative, dir_fd=anchor, onexc=retry)
                    else:
                        shutil.rmtree(relative, dir_fd=anchor,
                                      onerror=lambda fn, failed, exc: retry(fn, failed, exc[1]))

                erase(name)
                try:
                    os.stat(name, dir_fd=anchor, follow_symlinks=False)
                except FileNotFoundError:
                    return
                raise OSError('cleanup_incomplete')
            finally:
                os.close(anchor)

        def quarantine(self, path):
            # Detach before deletion: even an unrecoverable deletion failure must
            # leave the runner's exact automatic upload directory empty.
            path = Path(path)
            parent = self.directory(path.parent, shared_parent=True)
            try:
                info = os.stat(path.name, dir_fd=parent, follow_symlinks=False)
                if info.st_uid != os.getuid():
                    raise ValueError('unowned_upload')
                if not stat.S_ISLNK(info.st_mode):
                    fd = self.directory(path)
                    try:
                        opened = os.fstat(fd)
                        if (info.st_dev, info.st_ino) != (opened.st_dev, opened.st_ino):
                            raise ValueError('upload_changed')
                        # Cross-parent directory rename updates '..' and needs
                        # owner write/search on the moved root, not only its
                        # parent. Repair only this pinned, uid-checked inode.
                        os.fchmod(fd, stat.S_IMODE(opened.st_mode) | stat.S_IWUSR | stat.S_IXUSR)
                    finally:
                        os.close(fd)
                # A rejected symlink root is moved as a directory entry only;
                # neither validation nor deletion follows its external target.
                name = '.bbrun-rejected-' + os.urandom(16).hex()
                os.mkdir(name, 0o700, dir_fd=parent)
                destination = path.parent / name
                self.cleanup_roots.add(destination)
                os.rename(path.name, name + '/payload', src_dir_fd=parent, dst_dir_fd=parent)
                os.mkdir(path.name, 0o700, dir_fd=parent)
                return destination
            finally:
                os.close(parent)

    support = Support()
    if command is None:
        return support
    environment = child_env(os.environ)
    environment['BBRUN_FINAL_SCAN'] = '1'
    if mode != 'step' and os.environ.get('BUILDBUDDY_API_KEY'):
        environment['BUILDBUDDY_API_KEY'] = os.environ['BUILDBUDDY_API_KEY']
    os.environ.clear()
    os.environ.update(environment)
    code, handoff = None, 'unknown'
    cleanup = {'status': 'not_needed', 'targets_present': False, 'reason': 'no_cleanup_targets'}
    try:
        support.state.mkdir(mode=0o700)
        try:
            code = subprocess.call(command, env=environment)
        except BaseException:
            print('runner command did not complete', file=sys.stderr)
        try:
            upload = Path(environment['BUILDBUDDY_ARTIFACTS_DIRECTORY'])
            fd = support.directory(upload)
            try:
                os.fchmod(fd, stat.S_IMODE(os.fstat(fd).st_mode) | stat.S_IWUSR | stat.S_IXUSR)
            finally:
                os.close(fd)
            if (upload / support.receipt_name).exists() or (upload / support.receipt_name).is_symlink():
                raise ValueError('reserved_receipt_collision')
            shutil.move(str(upload), result_path + '.artifacts')
            upload.mkdir(mode=0o700)
            handoff = 'pass'
        except BaseException:
            handoff = 'fail'
            cleanup = {'status': 'unknown', 'targets_present': None, 'reason': 'cleanup_not_observed'}
            try:
                rejected = support.quarantine(Path(environment['BUILDBUDDY_ARTIFACTS_DIRECTORY']))
                cleanup['targets_present'] = True
                support.remove(rejected)
                cleanup.update(status='pass', reason='cleanup_complete')
            except PermissionError:
                cleanup.update(status='fail', reason='cleanup_permission_denied')
            except BaseException:
                cleanup.update(status='fail', reason='cleanup_io_error')
    except BaseException:
        print('runner private state unavailable', file=sys.stderr, flush=True)
    finally:
        try:
            support.write('command.json', {'exit_code': code, 'handoff': handoff, 'cleanup': cleanup})
        except BaseException:
            print('runner private command state not recorded', file=sys.stderr, flush=True)
        # Keep each attempt independent so a private-state failure does not
        # prevent the legacy status attempt or the mandatory final step.
        try:
            wrapped = (128 - code if code is not None and code < 0 else code)
            Path(result_path).write_text(str(wrapped if handoff == 'pass' and wrapped is not None else 75))
        except BaseException:
            print('runner private status not recorded', file=sys.stderr, flush=True)


def scan_artifacts(scanner_data, scanner_hash, result_path):
    """Runner-only: preserve stage results and finalize even on cleanup failure."""
    import base64
    import contextlib
    import hashlib
    import io
    import json
    from pathlib import Path
    import re
    import shutil
    import tempfile
    from urllib.parse import quote, quote_plus

    support = run_and_record(None, None, result_path)
    secrets = {name: os.environ[name] for name in
               ('BUILDBUDDY_API_KEY', 'MERGIFY_TOKEN', 'GITHUB_TOKEN') if os.environ.get(name)}
    environment = child_env(os.environ)
    environment.pop('BBRUN_FINAL_SCAN', None)
    os.environ.clear()
    os.environ.update(environment)
    os.environ.update(secrets)
    upload_path = os.environ.get('BUILDBUDDY_ARTIFACTS_DIRECTORY')
    upload = Path(upload_path) if upload_path else None
    staged = Path(result_path + '.artifacts')
    code, first_code = None, None
    command_code, handoff = None, 'unknown'
    scanner_status, scanner_reason = 'unknown', 'scanner_not_run'
    cleanup = {'status': 'unknown', 'targets_present': None, 'reason': 'cleanup_not_observed'}
    original, final = None, 75
    scan = None
    diagnostic_records = []
    scan_number = 0

    forms = set(secrets.values())
    redactions = sorted(forms, key=len, reverse=True)

    def redact(message):
        while any(value in message for value in redactions):
            for value in redactions:
                message = message.replace(value, '')
        return message

    def dispose():
        nonlocal cleanup
        targets = []
        try:
            # Establish absence positively; a missing/inaccessible root is not
            # equivalent to an empty upload directory.
            if staged.exists() or staged.is_symlink():
                targets.append(staged)
            if upload is None:
                raise ValueError('missing_upload')
            if upload.is_symlink():
                present = True
            else:
                fd = support.directory(upload)
                try:
                    present = bool(os.listdir(fd))
                finally:
                    os.close(fd)
            if present:
                # Atomic detachment precedes chmod/rmtree and survives a
                # permanent deletion failure without an automatic upload leak.
                targets.append(support.quarantine(upload))
            cleanup = {'status': 'pass' if targets else 'not_needed',
                       'targets_present': bool(targets),
                       'reason': 'cleanup_complete' if targets else 'no_cleanup_targets'}
            for target in targets:
                support.remove(target)
        except PermissionError:
            cleanup = {'status': 'fail', 'targets_present': True, 'reason': 'cleanup_permission_denied'}
        except BaseException:
            cleanup = {'status': 'fail', 'targets_present': True if targets else None,
                       'reason': 'cleanup_io_error'}

    def result():
        return {'schema': 'tl3.bbrun.wrapper-result', 'version': 1,
                'parent_invocation_id': None,
                'command': {'status': 'unknown' if command_code is None else 'pass' if command_code == 0 else 'fail',
                            'exit_code': command_code,
                            'reason': 'command_not_observed' if command_code is None else 'command_succeeded' if command_code == 0 else 'command_failed'},
                'artifact_handoff': {'status': handoff,
                                     'reason': {'pass': 'handoff_succeeded', 'fail': 'handoff_failed', 'unknown': 'handoff_not_observed'}[handoff]},
                'scanner': {'status': scanner_status, 'first_exit_code': first_code,
                            'exit_code': code, 'reason': scanner_reason},
                'cleanup': cleanup,
                'wrapper': {'original_exit_code': original, 'exit_code': final}}

    try:
        # Match the pinned scanner's path redaction, including encoded keys.
        forms = set()
        for value in secrets.values():
            forms.add(value)
            for encoded in (base64.b64encode(value.encode()), base64.urlsafe_b64encode(value.encode())):
                forms.update((encoded.decode(), encoded.rstrip(b'=').decode()))
            for encoded in (quote(value), quote(value, safe=''), quote_plus(value)):
                forms.update((encoded, re.sub(r'%[0-9A-F]{2}', lambda m: m[0].lower(), encoded)))
        redactions = sorted(forms, key=len, reverse=True)

        try:
            state = support.read()
            command_code, handoff, cleanup = state['exit_code'], state['handoff'], state['cleanup']
        except BaseException:
            pass
        command_handoff = handoff
        handoff = 'fail'
        if upload is None:
            raise ValueError('upload_unavailable')
        fd = support.directory(upload)
        os.close(fd)
        if staged.exists() or staged.is_symlink():
            fd = support.directory(staged)
            os.close(fd)
            for path in staged.iterdir():
                target = upload / path.name
                if target.exists() or target.is_symlink():
                    raise ValueError('upload_collision')
                shutil.move(str(path), str(target))
            staged.rmdir()
            if cleanup['status'] in ('pass', 'not_needed'):
                cleanup = {'status': 'pass', 'targets_present': True, 'reason': 'cleanup_complete'}
        if (upload / support.receipt_name).exists() or (upload / support.receipt_name).is_symlink():
            handoff = 'fail'
            raise ValueError('reserved_receipt_collision')
        # Do not let the scanner read a file symlink into original Bazel/CAS
        # input. The cleanup implementation safely unlinks nested symlinks.
        for directory, subdirectories, files in os.walk(upload, followlinks=False):
            if any(Path(directory, name).is_symlink() for name in subdirectories + files):
                handoff = 'fail'
                raise ValueError('artifact_symlink')
        handoff = command_handoff
        if handoff != 'pass':
            raise ValueError('command_handoff_incomplete')
        if not secrets.get('BUILDBUDDY_API_KEY'):
            raise ValueError('required scanner input unavailable')
        source = base64.b64decode(scanner_data, validate=True)
        if hashlib.sha256(source).hexdigest() != scanner_hash:
            raise ValueError('scanner hash mismatch')
        sys.argv = ['check_no_secret_leak.py', '--env', 'BUILDBUDDY_API_KEY',
                    '--require', 'BUILDBUDDY_API_KEY']
        for name in ('MERGIFY_TOKEN', 'GITHUB_TOKEN'):
            if name in secrets:
                sys.argv.extend(['--env', name])
        sys.argv.append(upload_path)
        def rejection_evidence(status, diagnostics, target, phase):
            # Recognize whole reports against the actual file inventory. Never
            # extract a path or a rule from arbitrary scanner output.
            record = {'schema': 'tl3.bbrun.scan-diagnostics', 'version': 1,
                      'phase': phase, 'scanner_exit_code': status,
                      'diagnostics_status': 'partial', 'detections': [],
                      'incomplete_report_count': 0}
            try:
                paths = [target] if target is not None else [
                    Path(directory, filename) for directory, _, filenames in os.walk(upload)
                    for filename in filenames]
                expected = {}
                ambiguous = set()
                multiline = False
                incomplete = ('zstd: not scanned (compression.zstd unavailable)',
                              'compressed nesting limit, not scanned',
                              'gzip member unreadable, not scanned',
                              'zstd member unreadable, not scanned',
                              'unreadable path, not scanned')
                for path in paths:
                    # Newlines can forge other complete reports. Withhold the
                    # entire scan's interpretation when that framing is unsafe.
                    if any(char in str(path) for char in '\r\n\v\f\x1c\x1d\x1e\x85\u2028\u2029'):
                        multiline = True
                    relative = str(path.relative_to(upload)) if target is None else None
                    safe = redact(relative) if relative is not None else None
                    if safe is not None and (len(safe) > 512 or not re.fullmatch(r'[A-Za-z0-9_./ -]+', safe)):
                        safe = None
                    for rule in secrets:
                        report = redact(f'{path}: {rule}: secret detected')
                        item = {'path': safe, 'rule_id': rule, 'match_count': 1}
                        if report in expected:
                            ambiguous.add(report)
                        expected[report] = item
                    for reason in incomplete:
                        report = redact(f'{path}: {reason}')
                        if report in expected:
                            ambiguous.add(report)
                        expected[report] = None
                complete = bool(diagnostics) and not multiline
                seen = set()
                if not multiline:
                    for report in diagnostics.splitlines():
                        if report not in expected or report in ambiguous or report in seen:
                            complete = False
                            continue
                        seen.add(report)
                        item = expected[report]
                        if item is None:
                            record['incomplete_report_count'] += 1
                        elif len(record['detections']) < 32:
                            record['detections'].append(item)
                        else:
                            complete = False
                record['diagnostics_status'] = 'complete' if complete else 'partial'
            except BaseException:
                # Diagnostics are observational; failures cannot alter verdicts
                # or expose exception messages, filenames or scanner bytes.
                record['detections'] = []
            diagnostic_records.append(record)

        def scan(target=None):
            # Raw diagnostics stay private: paths may themselves contain secrets.
            # Execute the pinned scanner in-process without credential-bearing children.
            sys.argv[-1] = str(target or upload)
            nonlocal scanner_status, scanner_reason, scan_number
            scan_number += 1
            diagnostics = io.StringIO()
            status = 75
            with contextlib.redirect_stdout(diagnostics), contextlib.redirect_stderr(diagnostics):
                try:
                    exec(compile(source, '<pinned-artifact-scanner>', 'exec'), {'__name__': '__main__'})
                except SystemExit as outcome:
                    status = outcome.code if type(outcome.code) is int else 75
                except BaseException:
                    scanner_status, scanner_reason = 'error', 'scanner_exception'
                    rejection_evidence(None, '', target, 'receipt' if target else 'initial' if scan_number == 1 else 'remaining')
                    raise
            if status != 0 or diagnostics.getvalue():
                rejection_evidence(status, diagnostics.getvalue(), target,
                                   'receipt' if target else 'initial' if scan_number == 1 else 'remaining')
            return status, diagnostics.getvalue()

        code, diagnostics = scan()
        first_code = code
        scanner_status = 'pass' if code == 0 and not diagnostics else 'fail'
        scanner_reason = 'scan_clean' if scanner_status == 'pass' else 'scan_rejected'
        if code in (0, 75) and diagnostics:
            reasons = ('zstd: not scanned (compression.zstd unavailable)',
                       'compressed nesting limit, not scanned',
                       'gzip member unreadable, not scanned',
                       'zstd member unreadable, not scanned',
                       'unreadable path, not scanned')
            reports = {}
            for directory, _, filenames in os.walk(upload):
                for filename in filenames:
                    path = Path(directory, filename)
                    for reason in reasons:
                        report = redact(f'{path}: {reason}')
                        if report in reports and reports[report] != (path, reason):
                            raise ValueError('ambiguous scanner path')
                        reports[report] = (path, reason)
            dropped = {}
            for report in diagnostics.splitlines():
                # Secret hits (including those masked by exit 75), directory
                # failures and unknown diagnostics are fatal before any deletion.
                if report not in reports:
                    raise ValueError('scanner did not report only skipped files')
                path, reason = reports[report]
                dropped.setdefault(path, set()).add(reason)
            for path, reasons in dropped.items():
                path.unlink()
                if cleanup['status'] in ('pass', 'not_needed'):
                    cleanup = {'status': 'pass', 'targets_present': True, 'reason': 'cleanup_complete'}
                print('artifact scan: dropped skipped file', flush=True)
            # Only a clean second scan can authorize the remaining upload.
            code, diagnostics = scan()
            scanner_status = 'pass' if code == 0 and not diagnostics else 'fail'
            scanner_reason = 'scan_clean' if scanner_status == 'pass' else 'scan_incomplete'
    except BaseException:
        # Preserve the actual observed scanner codes, even if a drop/scan
        # exception follows. No execution means both codes remain null.
        if first_code is not None and scanner_status != 'error':
            scanner_status, scanner_reason = 'fail', 'scan_reason_unknown'
        else:
            scanner_status = 'error'
            scanner_reason = 'scanner_exception' if scan is not None else 'scanner_not_run'
    finally:
        try:
            command_result = 128 - command_code if command_code is not None and command_code < 0 else command_code
            original = (code or 75) if scanner_status != 'pass' else (command_result if handoff == 'pass' and command_result is not None else 75)
            if scanner_status != 'pass' or handoff != 'pass':
                prior_cleanup = cleanup
                dispose()
                if prior_cleanup['status'] in ('fail', 'unknown'):
                    cleanup = prior_cleanup
            elif cleanup['status'] == 'not_needed':
                # No rejection means there are no deletion obligations. The
                # empty staging root was positively removed above.
                cleanup = {'status': 'not_needed', 'targets_present': False, 'reason': 'no_cleanup_targets'}
            final = original if cleanup['status'] in ('pass', 'not_needed') else (original or 75)
            if final is None:
                final = 75
            support.state.mkdir(mode=0o700, exist_ok=True)
            support.write('wrapper.json', result())
            # Only a successful artifact decision can publish a receipt. Scan
            # its exact private bytes with the same pinned required-key core.
            # A rejected upload is never rescanned empty to manufacture PASS.
            if scanner_status == 'pass' and handoff == 'pass' and cleanup['status'] in ('pass', 'not_needed'):
                receipt_path = support.state / 'wrapper.json'
                receipt_code, receipt_diagnostics = scan(receipt_path)
                if receipt_code == 0 and not receipt_diagnostics:
                    destination = upload / support.receipt_name
                    # Exclusive creation prevents command-name collisions.
                    fd, temporary = tempfile.mkstemp(prefix='.bbrun-receipt-', dir=upload)
                    try:
                        with os.fdopen(fd, 'wb') as stream:
                            stream.write(receipt_path.read_bytes())
                        os.link(temporary, destination, follow_symlinks=False)
                    finally:
                        Path(temporary).unlink()
                else:
                    code = receipt_code
                    scanner_status, scanner_reason = 'fail', 'receipt_scan_rejected'
                    original = code or 75
                    dispose()
                    final = original
        except BaseException:
            # Publication failure cannot leave a partial or unscanned receipt.
            if scanner_status != 'pass':
                original = code or 75
            dispose()
            final = original or 75
        finally:
            try:
                support.write('wrapper.json', result())
                Path(result_path).write_text(str(final))
            except BaseException:
                # SIGKILL/unavailable storage cannot promise a durable receipt.
                final = original or 75
                dispose()
            if scanner_status != 'pass':
                if not diagnostic_records:
                    diagnostic_records.append({'schema': 'tl3.bbrun.scan-diagnostics', 'version': 1,
                                               'phase': 'unobserved', 'scanner_exit_code': code,
                                               'diagnostics_status': 'unavailable', 'detections': [],
                                               'incomplete_report_count': 0})
                # Private JSON accompanies retained runner stdout. Never put
                # evidence back into the rejected automatic-upload directory.
                try:
                    safe_records = []
                    for record in diagnostic_records:
                        rendered = json.dumps(record, sort_keys=True, separators=(',', ':'))
                        if any(form in rendered for form in forms):
                            record = {'diagnostics_status': 'withheld'}
                            rendered = json.dumps(record, separators=(',', ':'))
                        if not any(form in 'BBRUN_SCAN_DIAGNOSTICS_V1 ' + rendered for form in forms):
                            safe_records.append(record)
                            print('BBRUN_SCAN_DIAGNOSTICS_V1 ' + rendered, flush=True)
                    support.write('scan-diagnostics.v1.json', safe_records)
                except BaseException:
                    pass
            for name in secrets:
                os.environ.pop(name, None)
            print('artifact scan: PASS (upload directory; required key available)' if scanner_status == 'pass'
                  else 'artifact scan: FAIL; cleanup ' + cleanup['status'], flush=True)
            print('BBRUN_WRAPPER_RESULT_V1 ' + json.dumps(result(), sort_keys=True, separators=(',', ':')), flush=True)
    raise SystemExit(final)


def prepare_full_history(expected_commit):
    """Runner-only: hydrate origin without exposing Git diagnostics or credentials."""
    import json
    from pathlib import Path
    import re
    import subprocess

    environment = child_env(os.environ)
    environment.pop('BBRUN_FINAL_SCAN', None)
    facts = {'fetched': False, 'objects': 0, 'HEAD': None}
    stage = 'validate-commit'

    def git(arguments, *, accepted=(0,), failure='Git command failed'):
        result = subprocess.run(['git', '-c', 'gc.auto=0', '-c', 'maintenance.auto=false',
                                 *arguments], env=environment,
                                capture_output=True, text=True, timeout=300)
        if result.returncode not in accepted:
            raise RuntimeError(failure)
        return result

    try:
        if not re.fullmatch(r'[0-9a-f]{40}', expected_commit):
            raise RuntimeError('a full lowercase commit SHA is required')
        stage = 'git-version'
        version = git(['--version']).stdout
        match = re.match(r'git version (\d+)\.(\d+)', version)
        if not match or tuple(map(int, match.groups())) < (2, 36):
            raise RuntimeError('Git 2.36 or newer is required for fetch --refetch --no-filter')
        stage = 'shallow-check'
        shallow = git(['rev-parse', '--is-shallow-repository']).stdout.strip()
        if shallow not in ('true', 'false'):
            raise RuntimeError('cannot determine whether checkout is shallow')
        fetch = ['fetch', '--refetch', '--no-filter', '--tags']
        if shallow == 'true':
            fetch.append('--unshallow')
        fetch.extend(['origin', '+refs/heads/*:refs/remotes/origin/*'])
        stage = 'fetch'
        git(fetch, failure='full fetch with --refetch --no-filter failed; refusing Bazel')
        facts['fetched'] = True
        stage = 'head-check'
        head = git(['rev-parse', '--verify', 'HEAD']).stdout.strip()
        if re.fullmatch(r'[0-9a-f]{40}', head):
            facts['HEAD'] = head
        if head != expected_commit:
            raise RuntimeError('HEAD differs from requested commit after full fetch')
        stage = 'object-check'
        objects = git(['rev-list', '--objects', '--missing=print', '--all', 'HEAD'],
                      failure='cannot verify complete object graph').stdout.splitlines()
        if not objects or any(line.startswith('?') for line in objects):
            raise RuntimeError('missing objects remain after full fetch')
        if any(not re.fullmatch(r'[0-9a-f]{40}', line.split(' ', 1)[0]) for line in objects):
            raise RuntimeError('cannot verify object identifiers')
        facts['objects'] = len(objects)
        # A successful unfiltered refetch can leave old promisor pack markers.
        # Only discard their metadata after proving the reachable graph complete.
        stage = 'promisor-cleanup'
        pack = Path(git(['rev-parse', '--git-path', 'objects/pack']).stdout.strip())
        for marker in pack.glob('*.promisor'):
            try:
                marker.unlink()
            except FileNotFoundError:
                pass
        stage = 'partial-clone-cleanup'
        for name in ('remote.origin.promisor', 'remote.origin.partialclonefilter',
                     'extensions.partialclone'):
            git(['config', '--local', '--unset-all', name], accepted=(0, 5),
                failure='cannot remove partial-clone metadata')
        stage = 'metadata-check'
        metadata = git(['config', '--get-regexp',
                        r'^(remote\..*\.(promisor|partialclonefilter)|extensions\.partialclone)$'],
                       accepted=(0, 1), failure='cannot verify partial-clone metadata')
        if metadata.returncode != 1 or any(pack.glob('*.promisor')):
            raise RuntimeError('promisor metadata remains after full fetch')
        stage = 'shallow-check'
        if git(['rev-parse', '--is-shallow-repository']).stdout.strip() != 'false':
            raise RuntimeError('shallow history remains after full fetch')
        return True
    except (OSError, subprocess.SubprocessError, RuntimeError) as error:
        print(f'full history preparation failed: {stage} ({type(error).__name__})',
              file=sys.stderr, flush=True)
        return False
    finally:
        print('full history: ' + json.dumps(facts, sort_keys=True), flush=True)


def run_with_full_history(command, mode, result_path, commit):
    """Keep preparation in the command step and finalize a failed preparation."""
    from pathlib import Path

    if prepare_full_history(commit):
        run_and_record(command, mode, result_path)
        return
    support = run_and_record(None, None, result_path)
    cleanup = {'status': 'unknown', 'targets_present': None, 'reason': 'cleanup_not_observed'}
    try:
        upload_path = os.environ.get('BUILDBUDDY_ARTIFACTS_DIRECTORY')
        if upload_path:
            upload = Path(upload_path)
            fd = support.directory(upload)
            try:
                present = bool(os.listdir(fd))
            finally:
                os.close(fd)
            if present:
                target = support.quarantine(upload)
                cleanup = {'status': 'fail', 'targets_present': True, 'reason': 'cleanup_io_error'}
                support.remove(target)
            cleanup = {'status': 'pass' if present else 'not_needed', 'targets_present': present,
                       'reason': 'cleanup_complete' if present else 'no_cleanup_targets'}
    except PermissionError:
        cleanup.update(status='fail', reason='cleanup_permission_denied')
    except BaseException:
        cleanup.update(status='fail', reason='cleanup_io_error')
    finally:
        try:
            support.state.mkdir(mode=0o700, exist_ok=True)
            support.write('command.json', {'exit_code': None, 'handoff': 'fail', 'cleanup': cleanup})
        except BaseException:
            print('runner private command state not recorded', file=sys.stderr, flush=True)
        try:
            Path(result_path).write_text('75')
        except BaseException:
            print('runner private status not recorded', file=sys.stderr, flush=True)


def secured_steps(command, mode, scanner, full_history_commit=None):
    result_path = '/tmp/bbrun-' + uuid.uuid4().hex + '.status'
    common = 'import os, sys\n' + inspect.getsource(child_env) + '\n'
    first = (common + inspect.getsource(run_and_record) + '\n'
             + f'run_and_record({command!r}, {mode!r}, {result_path!r})\n')
    if full_history_commit is not None:
        first = (common + inspect.getsource(prepare_full_history) + '\n'
                 + inspect.getsource(run_and_record) + '\n'
                 + inspect.getsource(run_with_full_history) + '\n'
                 + f'run_with_full_history({command!r}, {mode!r}, {result_path!r}, {full_history_commit!r})\n')
    final = (common + inspect.getsource(run_and_record) + '\n' + inspect.getsource(scan_artifacts) + '\n'
             + f'scan_artifacts({base64.b64encode(scanner).decode()!r}, {SCANNER_SHA256!r}, {result_path!r})\n')
    return [{'run': shlex.join(['python3', '-I', '-S', '-c', source])}
            for source in (first, final)]


def run_request(args):
    command = args.command
    if not command:
        raise RemoteError('a command is required after --')
    mode = getattr(args, 'mode', 'test')
    refuse_artifact_logs(command, mode)
    scanner = checked_scanner()
    if mode != 'step':
        verb = 'query' if mode == 'query' else 'test'
        if command[:2] != ['bazel', verb]:
            raise RemoteError(f'{verb} mode requires bazel {verb}; use step for arbitrary commands')
        if any(arg.startswith('--') and re.search(
                r'(?i)(api.key|token|remote_header|test_env|action_env)', arg) for arg in command):
            raise RemoteError('credential and environment flags are forbidden')
        if verb == 'test':
            if '--jobs=16' not in command or any(
                    arg.startswith(('--jobs', '-j')) and arg != '--jobs=16' for arg in command):
                raise RemoteError('agent runs require --jobs=16')
            if any(arg.startswith('--config') for arg in command[command.index('--jobs=16') + 1:]):
                raise RemoteError('--jobs=16 must follow all configs')
    command = agent_command(command, args.repo, getattr(args, 'cloud', False), getattr(args, 'arch', 'amd64'))
    request = {
        'repo': args.repo, 'commit_sha': args.commit, 'branch': args.branch,
        'steps': secured_steps(command, mode, scanner,
                               args.commit if getattr(args, 'full_history', False) else None),
        'env': {'GIT_REPO_DEFAULT_BRANCH': 'main'},
        'platform_properties': {
            'EstimatedCPU': '4', 'EstimatedMemory': '8GB',
            'EstimatedFreeDiskBytes': '50GB', 'container-image': IMAGE,
            'workload-isolation-type': 'firecracker', 'recycle-runner': 'true',
        },
        'timeout': f'{args.timeout}m',
        'wait_until': 'QUEUED' if getattr(args, 'detach', False) else 'STARTED',
    }

    return request


def status(client, invocation_id, *, timeout=120, artifacts=False, children=False):
    body = {'selector': {'invocation_id': invocation_id}}
    if artifacts:
        body.update(include_artifacts=True, include_build_tool_logs=True)
    if children:
        body['include_child_invocations'] = True
    records = client.call('GetInvocation', body, timeout=timeout).get('invocation', [])
    return records[0] if records else {}


def wait(client, invocation_id, deadline, *, clock=None, sleep=None):
    """Return a terminal record, or None at the caller's resumable deadline."""
    clock = clock or time.monotonic
    sleep = sleep or time.sleep
    errors = 0
    while True:
        remaining = deadline - clock()
        if remaining <= 0:
            return None
        try:
            record = status(client, invocation_id, timeout=min(120, remaining), children=True)
            errors = 0
        except RemoteError:
            errors += 1
            if errors > 3:
                raise
        else:
            if record.get('invocationStatus') in TERMINAL:
                return record
        remaining = deadline - clock()
        if remaining <= 0:
            return None
        sleep(min(10, remaining))


def log(client, invocation_id):
    """Read public GetLog pages from the first chunk, including long logs.

    proto/api/v1/log.proto: selector + page_token -> log.contents +
    next_page_token (JSON nextPageToken). api_server.go maps those tokens to
    eventlog chunk_id/next_chunk_id. Empty selects the tail; 0000 selects head.
    """
    chunks, page, seen = [], '0000', set()
    while True:
        seen.add(page)
        response = client.call('GetLog', {'selector': {'invocation_id': invocation_id}, 'page_token': page})
        chunks.append(response.get('log', {}).get('contents', ''))
        following = response.get('nextPageToken', response.get('next_page_token', ''))
        if not following or following == page:
            return ''.join(chunks)
        if following in seen:
            raise RemoteError('GetLog pagination cycled')
        page = following


def clean(key, data):
    """Refuse to print or save credential-bearing raw or gzip evidence."""
    needle = key.encode()
    if needle in data or (data[:2] == b'\x1f\x8b' and needle in gzip.decompress(data)):
        raise RemoteError('credential found in evidence; refused to print or save it')
    return data


def save(client, path, data):
    path.write_bytes(clean(client.key, data))


def fetch_file(client, uri):
    # GetFile's HTTP handler streams raw bytes, unlike its gRPC response proto.
    return clean(client.key, client.call('GetFile', {'uri': uri}, raw=True))


def artifacts(client, invocation_id, name=None, out=None):
    record = status(client, invocation_id, artifacts=True)
    if not record:
        raise RemoteError('invocation not found')
    files = record.get('artifacts', []) + record.get('buildToolLogs', record.get('build_tool_logs', []))
    for item in files:
        metadata = {field: item[field] for field in ('name', 'uri', 'sizeBytes', 'size_bytes', 'hash')
                    if field in item}
        print(clean(client.key, json.dumps(metadata).encode()).decode(), flush=True)
    if not files:
        print('No artifacts published.', flush=True)
    if name is not None:
        matches = [item for item in files if item.get('name') == name]
        if len(matches) != 1:
            raise RemoteError('artifact name is missing or ambiguous')
        item = matches[0]
        if item.get('uri'):
            data = fetch_file(client, item['uri'])
        elif 'contents' in item:
            data = base64.b64decode(item['contents'], validate=True)
        else:
            raise RemoteError('artifact has neither a URI nor inline contents')
        save(client, out, data)
        print(f'Saved {name} to {out}', flush=True)
    return 0


def succeeded(record):
    """Remote Run uses OK; Bazel children use SUCCESS. Conflicts fail closed."""
    if record.get('invocationStatus') != 'COMPLETE_INVOCATION_STATUS' or record.get('success') is False:
        return False
    code = record.get('bazelExitCode', record.get('bazel_exit_code'))
    if code:
        return code in ('OK', 'SUCCESS')
    return record.get('success') is True


def still_running(invocation_id, detail=''):
    if detail:
        print(detail, flush=True)
    print(f'still running: {invocation_id}; resume with: bbrun.py wait {invocation_id}', flush=True)
    return 2


def verdict(client, record, invocation_id, prefix):
    text = json.dumps({
        'invocationId': invocation_id,
        'invocationStatus': record.get('invocationStatus'),
        'success': succeeded(record),
        'bazelExitCode': record.get('bazelExitCode', record.get('bazel_exit_code'))}, sort_keys=True)
    print(prefix + clean(client.key, text.encode()).decode(), flush=True)


def report(client, invocation_id, record, log_file=None, *, deadline=None):
    verdict(client, record, invocation_id, 'verdict: ')
    passed = succeeded(record)
    pending = []
    # A completed runner is insufficient evidence when it names a failed or
    # unfinished Bazel test invocation. Check every published test child.
    children = record.get('childInvocations', record.get('child_invocations', []))
    for child in children:
        child_id = child.get('invocationId', child.get('invocation_id'))
        if not child_id:
            raise RemoteError('child invocation ID is missing')
        remaining = 120 if deadline is None else deadline - time.monotonic()
        if remaining <= 0:
            pending.append(child_id)
            continue
        child_record = status(client, child_id, timeout=min(120, remaining))
        if not child_record:
            pending.append(child_id)
        elif child_record.get('command') == 'test':
            verdict(client, child_record, child_id, 'child test verdict: ')
            if child_record.get('invocationStatus') not in TERMINAL:
                pending.append(child_id)
            else:
                passed = passed and succeeded(child_record)
    if pending and passed:
        return still_running(invocation_id, 'Waiting for child test results: ' + ', '.join(pending))
    output = clean(client.key, log(client, invocation_id).encode()).decode()
    if log_file:
        save(client, log_file, output.encode())
    if 'no registered executors in pool' in output:
        print('Self-hosted pool unavailable: failing without cloud fallback. '
              'Only a coordinator-approved --cloud run may use cloud actions.', flush=True)
    print(output, end='' if output.endswith('\n') else '\n', flush=True)
    return 0 if passed else 1


# Cancel is deliberately self-contained: the installed operator client must not
# import helpers from a checkout that can disappear after fast-forward.
PROTECTED_INVOCATIONS = frozenset({'631c29e5-de93-4bde-be66-821decdbc83b'})


def cancel_args(argv):
    from urllib.parse import urlsplit

    parser = argparse.ArgumentParser(prog='bbrun.py cancel', allow_abbrev=False)
    parser.add_argument('invocation_id')
    parser.add_argument('--expect-commit', required=True, action='append')
    parser.add_argument('--expect-repo', required=True, action='append')
    parser.add_argument('--protect-invocation', action='append', default=[])
    args = parser.parse_args(argv)
    if len(args.expect_commit) != 1 or len(args.expect_repo) != 1:
        parser.error('exactly one source SHA and repository URL are required')
    args.expect_commit = args.expect_commit[0]
    args.expect_repo = args.expect_repo[0]
    ids = [args.invocation_id, *args.protect_invocation]
    if any(not re.fullmatch(r'[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}', iid)
           or uuid.UUID(iid).int == 0 for iid in ids):
        parser.error('full lowercase nonzero invocation UUIDs are required')
    if args.invocation_id in PROTECTED_INVOCATIONS | set(args.protect_invocation):
        parser.error('protected invocation; cancellation refused')
    if not re.fullmatch(r'[0-9a-f]{40}', args.expect_commit):
        parser.error('a full lowercase source SHA is required')
    try:
        url = urlsplit(args.expect_repo)
        valid = (url.scheme == 'https' and url.hostname and url.netloc == url.hostname
                 and url.path not in ('', '/') and not url.query and not url.fragment
                 and not re.search(r'[\s\x00-\x1f\x7f]', args.expect_repo))
    except ValueError:
        valid = False
    if not valid:
        parser.error('an exact HTTPS repository URL without credentials, query or fragment is required')
    args.mode, args.selftest = 'cancel', False
    return args


def cancel_unique_fields(pairs):
    result = {}
    for name, value in pairs:
        if name in result:
            raise ValueError('duplicate JSON field')
        result[name] = value
    return result


def cancel_field(record, *names):
    if not isinstance(record, dict):
        raise RemoteError('malformed cancellation evidence')
    values = [record[name] for name in names if name in record]
    if not values or any(value != values[0] or type(value) is not type(values[0]) for value in values):
        raise RemoteError('missing or conflicting cancellation evidence')
    return values[0]


def cancel_record(response, field):
    if not isinstance(response, dict) or any(response.get(name) for name in
            ('nextPageToken', 'next_page_token')):
        raise RemoteError('malformed or paginated cancellation evidence')
    records = response.get(field)
    if not isinstance(records, list) or len(records) != 1 or not isinstance(records[0], dict):
        raise RemoteError('ambiguous cancellation evidence; exactly one record required')
    return records[0]


def cancel_observe(client, args):
    invocation = cancel_record(client.call('GetInvocation', {
        'selector': {'invocation_id': args.invocation_id}}, timeout=15, strict=True), 'invocation')
    identity = invocation.get('id', invocation)
    iid = cancel_field(identity, 'invocationId', 'invocation_id')
    # Check all advertised IDs rather than trusting a conflicting outer alias.
    if any(invocation[name] != iid for name in ('invocationId', 'invocation_id') if name in invocation):
        raise RemoteError('conflicting invocation identity')
    source = cancel_field(invocation, 'commitSha', 'commit_sha')
    repo = cancel_field(invocation, 'repoUrl', 'repo_url')
    if (iid, source, repo) != (args.invocation_id, args.expect_commit, args.expect_repo):
        raise RemoteError('invocation source, repository or identity mismatch; cancellation refused')
    state = cancel_field(invocation, 'invocationStatus', 'invocation_status')
    if state not in ('PARTIAL_INVOCATION_STATUS', *TERMINAL):
        raise RemoteError('unknown invocation state; cancellation refused')
    execution = cancel_record(client.call('GetExecution', {
        'execution_lookup': {'invocation_id': args.invocation_id}}, timeout=15, strict=True), 'execution')
    eid = cancel_field(execution, 'executionId', 'execution_id')
    if not isinstance(eid, str) or not eid or len(eid) > 2048 or not eid.isprintable():
        raise RemoteError('unbound execution evidence; cancellation refused')
    if any(execution[name] != iid for name in ('invocationId', 'invocation_id') if name in execution):
        raise RemoteError('execution invocation mismatch; cancellation refused')
    stage = cancel_field(execution, 'stage')
    stages = {0: 'UNKNOWN', 1: 'CACHE_CHECK', 2: 'QUEUED', 3: 'EXECUTING', 4: 'COMPLETED',
              'CACHE_CHECK': 'CACHE_CHECK', 'QUEUED': 'QUEUED',
              'EXECUTING': 'EXECUTING', 'COMPLETED': 'COMPLETED'}
    # REAPI ExecutionStage: UNKNOWN=0, CACHE_CHECK=1, QUEUED=2,
    # EXECUTING=3, COMPLETED=4.
    if type(stage) not in (str, int) or stage not in stages or stages[stage] == 'UNKNOWN':
        raise RemoteError('unknown execution stage; cancellation refused')
    stage = stages[stage]
    outcome = None
    code = exit_code = None
    if stage == 'COMPLETED':
        rpc_status = execution.get('status')
        if not isinstance(rpc_status, dict):
            raise RemoteError('completed execution lacks status; cancellation refused')
        code = rpc_status.get('code', 0)
        exits = [execution[name] for name in ('exitCode', 'exit_code') if name in execution]
        exit_code = exits[0] if exits else 0
        if (type(code) is not int or code < 0 or type(exit_code) is not int
                or any(type(value) is not int or value != exit_code for value in exits)):
            raise RemoteError('malformed terminal execution status')
        outcome = ('cancelled' if code == 1 else 'already_terminal_failure'
                   if code or exit_code else 'already_terminal_success')
    elif state in TERMINAL:
        success = invocation.get('success')
        if state == 'COMPLETE_INVOCATION_STATUS' and type(success) is bool:
            outcome = 'already_terminal_success' if success else 'already_terminal_failure'
        else:
            outcome = 'terminal_unknown'
    # Compare ownership privately; arbitrary service fields never enter output.
    ownership = {name: invocation.get(name) for name in
                 ('user', 'userId', 'user_id', 'groupId', 'group_id', 'host', 'command')}
    return {'invocation_id': iid, 'commit_sha': source, 'repo_url': repo,
            'invocation_status': state, 'execution_id': eid, 'stage': stage,
            'status_code': code, 'exit_code': exit_code}, ownership, outcome


def cancel(client, args):
    attempted = False
    def emit(phase, **fields):
        data = json.dumps({'phase': phase, **fields}, sort_keys=True).encode()
        print(clean(client.key, data).decode(), flush=True)
    try:
        before, owner, outcome = cancel_observe(client, args)
        emit('before', **before)
        if outcome is None:
            current, current_owner, outcome = cancel_observe(client, args)
            if current_owner != owner or current['execution_id'] != before['execution_id']:
                raise RemoteError('ownership or execution changed; cancellation refused')
            if outcome is None:
                attempted = True
                ack = client.call('CancelExecutions', {'invocation_id': args.invocation_id}, timeout=15, strict=True)
                if not isinstance(ack, dict):
                    raise RemoteError('malformed cancellation acknowledgement')
                emit('ack', received=True, completion_confirmed=False)
                after, after_owner, outcome = cancel_observe(client, args)
                if after_owner != owner or after['execution_id'] != before['execution_id']:
                    raise RemoteError('ownership or execution changed after cancellation')
                emit('after', **after)
        emit('terminal', outcome=outcome or 'unconfirmed', cancellation_attempted=attempted)
        return 2 if outcome in (None, 'terminal_unknown') else 0
    except (RemoteError, OSError, ValueError, http.client.HTTPException):
        # The transport sanitizes HTTP errors. Fixed diagnostics also keep
        # remote ownership and acknowledgement strings out of evidence entirely.
        emit('terminal', outcome='unconfirmed' if attempted else 'refused',
             cancellation_attempted=attempted)
        print('cancellation unconfirmed; do not blindly retry' if attempted else
              'cancellation refused; evidence unavailable, ambiguous or mismatched', file=sys.stderr)
        return 2 if attempted else 1


def parse_args(argv):
    argv = list(argv)
    if argv[:1] == ['cancel']:
        return cancel_args(argv[1:])
    modes = {'test', 'query', 'step', 'wait', 'artifacts'}
    mode = argv.pop(0) if argv and argv[0] in modes else 'test'
    parser = argparse.ArgumentParser(prog='bbrun.py ' + mode, description=__doc__)
    parser.set_defaults(mode=mode)
    parser.add_argument('--selftest', action='store_true')
    if mode in ('wait', 'artifacts'):
        parser.add_argument('invocation_id')
    if mode in ('test', 'query', 'step'):
        parser.add_argument('--repo')
        parser.add_argument('--commit')
        parser.add_argument('--branch')
        parser.add_argument('--timeout', type=int, default=10, help='runner timeout in minutes')
        parser.add_argument('--full-history', action='store_true',
                            help='fetch all history and blobs and verify checkout before the command')
        parser.add_argument('--detach', action='store_true')
        parser.add_argument('--cloud', action='store_true', help='coordinator-approved cloud override for non-TL3 repos')
        parser.add_argument('--arch', choices=['amd64', 'arm64'], default='amd64', help='architecture of the self-hosted actions; Bazel client stays cloud')
        parser.add_argument('command', nargs=argparse.REMAINDER)
    if mode != 'artifacts':
        parser.add_argument('--max-wait', type=float, default=600, help='wait seconds, from 0 to 600')
        parser.add_argument('--log-file', type=Path)
    else:
        parser.add_argument('--fetch')
        parser.add_argument('--out', type=Path)
    args = parser.parse_args(argv)
    if args.selftest:
        return args
    if mode in ('test', 'query', 'step'):
        if not all((args.repo, args.commit, args.branch)):
            parser.error('--repo, --commit and --branch are required')
        if args.timeout <= 0:
            parser.error('--timeout must be positive')
        if args.command[:1] == ['--']:
            args.command = args.command[1:]
    if mode != 'artifacts' and not 0 <= args.max_wait <= 600:
        parser.error('--max-wait must be from 0 to 600 seconds')
    if mode == 'artifacts' and bool(args.fetch) != bool(args.out):
        parser.error('--fetch and --out must be used together')
    return args


def wait_for_gate_window(*, path=None, clock=None, wall_clock=None, sleep=None, emit=None):
    """Yield submissions to critical gates; injectable inputs keep selftests offline."""
    path = GATE_WINDOW if path is None else path
    clock = clock or time.monotonic
    wall_clock = wall_clock or time.time
    sleep = sleep or time.sleep
    emit = emit or (lambda message: print(message, flush=True))
    started = clock()
    while True:
        try:
            modified = path.stat().st_mtime
            with path.open(encoding='utf-8', errors='replace') as stream:
                owner = stream.readline().strip() or 'unspecified owner/reason'
        except FileNotFoundError:
            return
        elapsed = clock() - started
        if elapsed >= 45 * 60:
            emit('gate window wait reached 45 min; proceeding')
            return
        since = time.strftime('%Y-%m-%d %H:%M:%S UTC', time.gmtime(modified))
        if wall_clock() - modified >= 45 * 60:
            emit(f'warning: stale gate window since {since} by {owner}; '
                 'at least 45 min old, ignoring')
            return
        emit(f'gate window open since {since} by {owner}; '
             f'waiting ({int(elapsed // 60)}/45 min)')
        sleep(min(60, 45 * 60 - elapsed))


def main(argv=None):
    args = parse_args(sys.argv[1:] if argv is None else argv)
    if args.selftest:
        return selftest()
    try:
        key = (Path.home() / '.config/buildbuddy/api_key').read_text().strip()
        if not key:
            raise RemoteError('BuildBuddy key file is empty')
        client = BuildBuddy(key)
        if args.mode == 'cancel':
            return cancel(client, args)
        if args.mode == 'artifacts':
            return artifacts(client, args.invocation_id, args.fetch, args.out)
        if args.mode == 'wait':
            invocation_id = args.invocation_id
        else:
            request = run_request(args)
            wait_for_gate_window(emit=lambda message: print(
                clean(key, message.encode()).decode(), flush=True))
            response = client.call('Run', request, timeout=600)
            invocation_id = response.get('invocationId', response.get('invocation_id'))
            if not invocation_id:
                raise RemoteError('Run returned no invocation ID; do not blindly retry submission')
            print(f'https://app.buildbuddy.io/invocation/{invocation_id}', flush=True)
            if args.detach:
                print(f'detached: {invocation_id}', flush=True)
                return 0
        deadline = time.monotonic() + args.max_wait
        record = wait(client, invocation_id, deadline)
        if record is None:
            return still_running(invocation_id)
        return report(client, invocation_id, record, args.log_file, deadline=deadline)
    except ActionTooLarge as error:
        print(str(error), file=sys.stderr)
        return 2
    except RemoteError as error:
        print(str(error), file=sys.stderr)
        return 1
    except (OSError, ValueError) as error:
        print(f'BuildBuddy client error ({type(error).__name__})', file=sys.stderr)
        return 1


def environment_selftest():
    from unittest.mock import patch

    key = 'offline-secret-sentinel-2667'
    kept = {'PATH': '/usr/bin', 'HOME': '/tmp', 'TMPDIR': '/tmp/work', 'LANG': 'C.UTF-8',
            'LC_ALL': 'C', 'LC_TIME': 'C', 'UV_CACHE_DIR': '/tmp/uv',
            'PYTHONPATH': '/tmp/modules', 'PYTHONNOUSERSITE': '1', 'VIRTUAL_ENV': '/tmp/venv',
            'BUILDBUDDY_ARTIFACTS_DIRECTORY': '/tmp/fake-buildbuddy-artifacts',
            'BBRUN_FINAL_SCAN': '1'}
    forbidden = {'BUILDBUDDY_API_KEY': key, 'MERGIFY_TOKEN': 'fake-mergify-secret',
                 'GITHUB_TOKEN': 'fake-github-secret', 'OTHER_API_KEY': 'fake-other-key',
                 'OTHER_TOKEN': 'fake-other-token', 'FOO_API_KEY': 'fake-foo-key',
                 'BAR_TOKEN': 'fake-bar-token', 'UV_API_KEY': 'fake-uv-secret',
                 'PYTHON_AUTH_TOKEN': 'fake-python-secret', 'LC_TEST_TOKEN': 'fake-locale-secret'}
    polluted = {**kept, **forbidden, 'SHELL': '/bin/bash', 'LD_PRELOAD': '/tmp/unwanted.so',
                'BUILDBUDDY_UNLISTED_DIRECTORY': '/tmp/not-allowed'}

    def check_environment(environment):
        for name, value in kept.items():
            assert environment.get(name) == value, name
        for name in polluted:
            if name not in kept:
                assert name not in environment, name
        for value in forbidden.values():
            assert value not in json.dumps(environment)

    check_environment(child_env(polluted))
    args = argparse.Namespace(repo='https://github.com/RobTand/prismabuild', commit='a' * 40,
                              branch='prismabuild-2667', timeout=10, mode='bazel',
                              command=['bazel', 'test', '//tests:test_core', '--config=remote', '--jobs=16'])
    bazel_request = run_request(args)
    args.mode = 'step'
    args.command = ['python3', '-m', 'pytest', 'tests/test example.py']
    # Generating a step must not interpolate any local credential or env value.
    with patch.dict(os.environ, polluted, clear=True):
        step_request = run_request(args)
    for request in (bazel_request, step_request):
        text = json.dumps(request)
        argv = shlex.split(request['steps'][0]['run'])
        for name, value in forbidden.items():
            assert name not in request['env']
            assert value not in text
            assert value not in json.dumps(argv)
    argv = shlex.split(step_request['steps'][0]['run'])
    assert argv[:4] == ['python3', '-I', '-S', '-c']
    # Exercise the exact generated wrapper, intercepting process launch rather
    # than starting a child. No test subprocess inherits even the fake credentials.
    with patch.dict(os.environ, polluted, clear=True), patch.object(sys, 'argv', ['-c', *argv[5:]]):
        with patch('subprocess.call', return_value=0) as execute, patch.object(Path, 'write_text'), \
                patch.object(Path, 'exists', return_value=False), patch.object(Path, 'mkdir'):
            exec(compile(argv[4], '<step-wrapper>', 'exec'), {})
        assert execute.call_count == 1
        check_environment(execute.call_args.kwargs['env'])
        for value in forbidden.values():
            assert value not in json.dumps(execute.call_args.args)
    return 0


def security_selftest():
    import subprocess
    import tempfile
    from unittest.mock import patch

    flags = ['--build_event_json_file', '--build_event_binary_file', '--build_event_text_file',
             '--command_log', '--execution_log_json_file', '--execution_log_binary_file',
             '--execution_log_compact_file', '--execution_log_future_file']
    root = '/tmp/fake-buildbuddy-artifacts'
    paths = [root + '/events', '$BUILDBUDDY_ARTIFACTS_DIRECTORY/events',
             '${BUILDBUDDY_ARTIFACTS_DIRECTORY}/events']
    refused = 0
    with patch.dict(os.environ, {'BUILDBUDDY_ARTIFACTS_DIRECTORY': root}):
        for mode in ('test', 'query', 'step'):
            base = (['bazel', mode, '//example:target', '--jobs=16'] if mode != 'step'
                    else ['bash', '-c'])
            for flag in flags:
                for path in paths:
                    for form in ([flag + '=' + path], [flag, path]):
                        command = base + (['bazel test ' + shlex.join(form)] if mode == 'step' else form)
                        args = argparse.Namespace(repo='repo', commit='a' * 40, branch='main',
                                                  timeout=1, mode=mode, command=command)
                        try:
                            run_request(args)
                        except RemoteError as error:
                            assert 'artifacts' in str(error)
                            refused += 1
                        else:
                            raise AssertionError(f'unsafe artifacts flag accepted: {mode} {flag}')
        for operation in ('cp', 'mv'):
            for source in ('/tmp/bep.json', '/tmp/events.bep', '/tmp/command.log',
                           '/tmp/bazel-command.log', '/tmp/build_event_binary_file'):
                for destination in paths:
                    args.command = ['bash', '-c', f'{operation} {source} {destination}']
                    try:
                        run_request(args)
                    except RemoteError:
                        refused += 1
                    else:
                        raise AssertionError('BEP/log copy into artifacts accepted')
    for mode in ('test', 'query', 'step'):
        args.mode = mode
        args.command = ['true'] if mode == 'step' else ['bazel', mode, '//example:target', '--jobs=16']
        body = run_request(args)
        assert len(body['steps']) == 2
        final = shlex.split(body['steps'][-1]['run'])[4]
        assert 'BUILDBUDDY_API_KEY' in final
        assert 'BUILDBUDDY_ARTIFACTS_DIRECTORY' in final
        assert '--env' in final and '--require' in final
        assert "secrets.get('BUILDBUDDY_API_KEY')" in final
        assert 'BBRUN_FINAL_SCAN' in body['steps'][0]['run']
        assert "environment.pop('BBRUN_FINAL_SCAN', None)" in final
    # Exercise the exact generated steps with fake values only. No inherited
    # runner credentials, network, or host-dependent scheduling is involved.
    fake_key = 'offline-final-scanner-sentinel'
    for scenario in ('clean', 'missing-key', 'empty-key', 'leak', 'github-leak', 'mergify-leak',
                     'earlier-failure', 'failure-and-leak', 'incomplete', 'contract'):
        with tempfile.TemporaryDirectory(prefix='bbrun-final-scan-') as directory:
            command_upload = Path(directory, 'command-0')
            command_upload.mkdir()
            upload = Path(directory, 'command-1')
            upload.mkdir()
            planted = scenario in ('leak', 'failure-and-leak', 'github-leak', 'mergify-leak')
            planted_value = {'github-leak': 'offline-github-sentinel',
                             'mergify-leak': 'offline-mergify-sentinel'}.get(scenario, fake_key)
            (command_upload / 'proof.txt').write_text(planted_value if planted else 'harmless artifact')
            if scenario == 'incomplete':
                (command_upload / 'partial.zst').write_bytes(b'\x28\xb5\x2f\xfd')
            # A leak outside the exact upload directory must not affect a clean upload.
            Path(directory, 'staging.txt').write_text(fake_key)
            args.mode = 'step'
            code = 37 if scenario in ('earlier-failure', 'failure-and-leak') else 0
            args.command = [sys.executable, '-I', '-S', '-c',
                            'import os, sys; assert "BUILDBUDDY_API_KEY" not in os.environ; '
                            'assert os.environ.get("BBRUN_FINAL_SCAN") == "1"; '
                            f'sys.exit({code})']
            if scenario == 'contract':
                # Observe the scanner's actual argv and process environment using
                # an instrumented offline core. Production core behavior is tested above.
                observer = b'''import os, sys
assert os.environ['BUILDBUDDY_API_KEY'] == 'offline-final-scanner-sentinel'
assert 'BBRUN_FINAL_SCAN' not in os.environ
assert 'OTHER_API_KEY' not in os.environ
assert sys.argv[:-1] == ['check_no_secret_leak.py', '--env', 'BUILDBUDDY_API_KEY',
                    '--require', 'BUILDBUDDY_API_KEY', '--env', 'MERGIFY_TOKEN', '--env', 'GITHUB_TOKEN']
raise SystemExit(0)
'''
                with patch(__name__ + '.checked_scanner', return_value=observer), patch(
                        __name__ + '.SCANNER_SHA256', hashlib.sha256(observer).hexdigest()):
                    body = run_request(args)
            else:
                body = run_request(args)
            environment = {'PATH': os.defpath, 'BUILDBUDDY_ARTIFACTS_DIRECTORY': str(command_upload),
                           'BBRUN_FINAL_SCAN': '1', 'OTHER_API_KEY': 'offline-other-sentinel',
                           'GITHUB_TOKEN': 'offline-github-sentinel',
                           'MERGIFY_TOKEN': 'offline-mergify-sentinel'}
            if scenario != 'missing-key':
                environment['BUILDBUDDY_API_KEY'] = '' if scenario == 'empty-key' else fake_key
            first = subprocess.run(shlex.split(body['steps'][0]['run']), env=environment,
                                   capture_output=True, timeout=30)
            assert first.returncode == 0, scenario
            assert not list(command_upload.iterdir()), 'intermediate step would upload unscanned files'
            environment['BUILDBUDDY_ARTIFACTS_DIRECTORY'] = str(upload)
            final = subprocess.run(shlex.split(body['steps'][-1]['run']), env=environment,
                                   capture_output=True, timeout=30)
            for secret in (fake_key, 'offline-github-sentinel', 'offline-mergify-sentinel', 'offline-other-sentinel'):
                assert secret.encode() not in first.stdout + first.stderr + final.stdout + final.stderr
            if planted or scenario in ('missing-key', 'empty-key'):
                assert final.returncode != 0, scenario
                if scenario in ('missing-key', 'empty-key'):
                    assert final.returncode == 75, scenario
                assert not list(upload.iterdir()), scenario
            else:
                assert final.returncode == code, scenario
                assert (upload / 'proof.txt').read_text() == 'harmless artifact'
                assert b'artifact scan: PASS' in final.stdout
                if scenario == 'incomplete':
                    assert not (upload / 'partial.zst').exists()
                    assert b'artifact scan: dropped skipped file' in final.stdout
    with tempfile.TemporaryDirectory(prefix='bbrun-handoff-failure-') as directory:
        upload = Path(directory, 'upload')
        upload.mkdir()
        (upload / 'unsafe.txt').write_text(fake_key)
        result_path = str(Path(directory, 'result.status'))
        with patch.dict(os.environ, {'BUILDBUDDY_ARTIFACTS_DIRECTORY': str(upload)}, clear=True), \
                patch('subprocess.call', return_value=0), patch('shutil.move', side_effect=OSError):
            run_and_record(['true'], 'step', result_path)
        assert not list(upload.iterdir()), 'failed handoff would upload unscanned files'
        assert Path(result_path).read_text() == '75'
    scanner = Path(__file__).with_name('check_no_secret_leak.py')
    with patch.object(Path, 'read_bytes', autospec=True, side_effect=lambda path: b'changed' if path == scanner else b''):
        try:
            run_request(args)
        except RemoteError as error:
            assert 'hash' in str(error)
        else:
            raise AssertionError('modified scanner accepted')
    print(f'security selftest: PASS ({refused} refused BEP/log forms; final scan in all modes; '
          'intermediate uploads empty; exact final upload directory; marker paired with unmarked required-key scan; '
          '--require enforced; optional tokens scanned; missing key exits 75; leaks deleted; skipped files dropped; '
          'clean artifacts kept; original failure preserved; scanner hash enforced)')


def artifact_drop_selftest():
    import subprocess
    import tempfile
    from unittest.mock import patch

    fake_key = 'offline-drop-scanner-sentinel'
    scenarios = [('secret', 1, 1), ('mixed-secret', 75, 1),
                 ('drop', 0, 2), ('drop-incomplete', 0, 2),
                 ('redacted-path', 0, 2), ('earlier-failure', 37, 2),
                 ('rescan-secret', 1, 2), ('rescan-error', 42, 2),
                 ('rescan-skipped', 75, 2), ('scanner-error', 42, 1),
                 ('unknown-diagnostic', 75, 1), ('directory-error', 75, 1)]
    for scenario, expected, scans in scenarios:
        with tempfile.TemporaryDirectory(prefix='bbrun-drop-scan-') as directory:
            upload = Path(directory, 'upload')
            (upload / 'logs').mkdir(parents=True)
            relative = ('logs/' + fake_key + '-partial.zst' if scenario == 'redacted-path'
                        else 'logs/partial.zst')
            unscannable = upload / relative
            unscannable.write_bytes(b'opaque-file-bytes')
            clean = upload / 'proof.txt'
            clean.write_text('harmless artifact')
            status = Path(directory, 'result.status')
            status.write_text('37' if scenario == 'earlier-failure' else '0')
            support = run_and_record(None, None, str(status))
            support.state.mkdir(mode=0o700)
            support.write('command.json', {'exit_code': 37 if scenario == 'earlier-failure' else 0,
                          'handoff': 'pass', 'cleanup': {'status': 'not_needed',
                          'targets_present': False, 'reason': 'no_cleanup_targets'}})
            observer = (f'scenario = {scenario!r}\nrelative = {relative!r}\n' + r'''import os, sys
from pathlib import Path
upload = Path(sys.argv[-1])
if upload.is_file():
    raise SystemExit(0)
counter = upload.parent / 'scan-calls'
count = int(counter.read_text()) + 1 if counter.exists() else 1
counter.write_text(str(count))
assert 'BBRUN_FINAL_SCAN' not in os.environ
assert (upload / 'proof.txt').read_text() == 'harmless artifact'
if scenario == 'scanner-error' or (scenario == 'rescan-error' and count == 2):
    raise SystemExit(42)
if scenario == 'unknown-diagnostic':
    print('unexpected scanner error')
    raise SystemExit(75)
if scenario == 'directory-error':
    print(f'{upload / "logs"}: unreadable directory, not scanned')
    raise SystemExit(75)
skipped = scenario != 'secret' and ((upload / relative).exists() or scenario == 'rescan-skipped')
if skipped:
    report = f'{upload / relative}: zstd: not scanned (compression.zstd unavailable)'
    print(report.replace(os.environ['BUILDBUDDY_API_KEY'], ''))
secret = scenario in ('secret', 'mixed-secret') or (scenario == 'rescan-secret' and count == 2)
if secret:
    print(f'{upload / "proof.txt"}: BUILDBUDDY_API_KEY: secret detected')
raise SystemExit(75 if scenario in ('mixed-secret', 'drop-incomplete') and skipped else 1 if secret else 0)
''').encode()
            with patch(__name__ + '.SCANNER_SHA256', hashlib.sha256(observer).hexdigest()):
                step = secured_steps(['true'], 'step', observer)[-1]['run']
            source = shlex.split(step)[4]
            # Exercise the exact final step, using this test's status file.
            source = re.sub(r'/tmp/bbrun-[0-9a-f]{32}\.status', str(status), source)
            result = subprocess.run([sys.executable, '-I', '-S', '-c', source],
                                    env={'PATH': os.defpath, 'BUILDBUDDY_API_KEY': fake_key,
                                         'BBRUN_FINAL_SCAN': '1',
                                         'BUILDBUDDY_ARTIFACTS_DIRECTORY': str(upload)},
                                    capture_output=True, timeout=30)
            assert result.returncode == expected, (scenario, result.returncode, expected)
            assert int(Path(directory, 'scan-calls').read_text()) == scans, scenario
            output = result.stdout + result.stderr
            assert fake_key.encode() not in output, scenario
            assert b'opaque-file-bytes' not in output, 'file content escaped in diagnostic'
            if expected in (0, 37):
                assert not unscannable.exists(), scenario
                assert clean.read_text() == 'harmless artifact', scenario
                assert b'artifact scan: dropped skipped file' in output, scenario
                assert b'artifact scan: PASS' in output, scenario
            else:
                assert not list(upload.iterdir()), scenario
                assert b'artifact scan: FAIL' in output, scenario
    print('artifact drop selftest: PASS (unscannable files deleted with safe metadata; '
          'clean re-scan required; secret hits, scanner errors and directory errors fatal; '
          'redacted paths; prior command failure preserved)')


def gate_window_selftest():
    import contextlib
    import io
    import tempfile
    from unittest.mock import patch

    base = 1793660400.0
    with tempfile.TemporaryDirectory(prefix='bbrun-gate-window-') as directory:
        home = Path(directory)
        gate = home / 'GATE_WINDOW'
        keyfile = home / '.config/buildbuddy/api_key'
        keyfile.parent.mkdir(parents=True)
        keyfile.write_text('offline-gate-window-sentinel')
        common = ['--repo', 'https://github.com/RobTand/prismabuild', '--commit', 'a' * 40,
                  '--branch', 'main', '--detach', '--']
        for mode in ('test', 'query', 'step'):
            gate.write_text('uplift lead: critical full gate\nignored second line\n')
            os.utime(gate, (base, base))
            events = []
            elapsed = [0.0]

            def sleep(seconds):
                assert seconds == 60
                elapsed[0] += seconds
                events.append('sleep')
                gate.unlink()

            def call(client, method, body, **kwargs):
                events.append(method)
                return {'invocationId': 'offline-gated'}

            command = ['true'] if mode == 'step' else ['bazel', mode, '//example:target', '--jobs=16']
            output = io.StringIO()
            with patch(__name__ + '.GATE_WINDOW', gate, create=True), \
                    patch.object(Path, 'home', return_value=home), \
                    patch('time.time', side_effect=lambda: base + elapsed[0]), \
                    patch('time.monotonic', side_effect=lambda: elapsed[0]), \
                    patch('time.sleep', side_effect=sleep), \
                    patch.object(BuildBuddy, 'call', autospec=True, side_effect=call), \
                    contextlib.redirect_stdout(output):
                assert main([mode, *common, *command]) == 0
            assert events == ['sleep', 'Run'], f'{mode} submitted before the gate window closed'
            assert 'uplift lead: critical full gate; waiting (0/45 min)' in output.getvalue()
            assert 'ignored second line' not in output.getvalue()

        def exercise(age=None, remove_after=None, refresh=False):
            elapsed, delays, messages = [0.0], [], []
            if gate.exists():
                gate.unlink()
            if age is not None:
                gate.write_text('coordinator: full main gate\n')
                os.utime(gate, (base - age, base - age))

            def advance(seconds):
                delays.append(seconds)
                elapsed[0] += seconds
                if remove_after is not None and len(delays) == remove_after:
                    gate.unlink()
                elif refresh:
                    os.utime(gate, (base + elapsed[0], base + elapsed[0]))

            wait_for_gate_window(path=gate, clock=lambda: elapsed[0],
                                 wall_clock=lambda: base + elapsed[0], sleep=advance,
                                 emit=messages.append)
            return elapsed[0], delays, messages

        assert exercise() == (0, [], [])
        elapsed, delays, messages = exercise(age=0, remove_after=2)
        assert elapsed == 120 and delays == [60, 60]
        assert len(messages) == 2
        assert messages[0] == ('gate window open since 2026-11-02 23:00:00 UTC by '
                               'coordinator: full main gate; waiting (0/45 min)')
        assert messages[1].endswith('waiting (1/45 min)')
        for refresh in (False, True):
            elapsed, delays, messages = exercise(age=0, refresh=refresh)
            assert elapsed == 2700 and delays == [60] * 45
            assert len(messages) == 46
            assert '45 min' in messages[-1] and 'proceeding' in messages[-1]
        for age in (2700, 2701):
            elapsed, delays, messages = exercise(age=age)
            assert elapsed == 0 and not delays
            assert len(messages) == 1 and 'warning:' in messages[0] and 'stale' in messages[0]
        gate.write_text('coordinator: still active\n')
        os.utime(gate, (base, base))
        for argv in (['wait', 'offline-running'], ['artifacts', 'offline-running']):
            with patch(__name__ + '.GATE_WINDOW', gate), patch.object(Path, 'home', return_value=home), \
                    patch('time.time', return_value=base), \
                    patch('time.sleep', side_effect=AssertionError('non-submission mode blocked')), \
                    patch(__name__ + '.wait', return_value={}), \
                    patch(__name__ + '.report', return_value=0), \
                    patch(__name__ + '.artifacts', return_value=0):
                assert main(argv) == 0
    print('gate window selftest: PASS (test/query/step wait before submission; no file proceeds; '
          '60s rechecks and removal; 45min total cap despite refresh; stale warning; '
          'wait/artifacts never block; fake clocks and temporary path)')


def routing_selftest():
    from argparse import Namespace
    command = ['bazel', 'test', '//tests:test_remote_gate', '--config=remote', '--jobs=16']
    for name in ['trustladderiii', 'prismaquant', 'prismabuild', 'tessera']:
        for arch in ['amd64', 'arm64']:
            args = Namespace(repo=f'https://github.com/RobTand/{name}.git', commit='a'*40,
                             branch='main', timeout=10, mode='test', command=command,
                             cloud=False, arch=arch)
            request = run_request(args)
            props = request['platform_properties']
            routed = agent_command(command, args.repo, arch=arch)
            if name == 'trustladderiii':
                assert 'Pool' not in props
                assert routed == command
                continue
            assert 'Pool' not in props
            assert props['workload-isolation-type'] == 'firecracker'
            assert routed[-1].endswith('Pool=selfhosted-' + arch)
            assert '--remote_executor=grpcs://remote.buildbuddy.io' in routed
            assert '--remote_local_fallback=false' in routed
            # Bazel must be able to select local for no-remote/no-remote-exec
            # actions such as rules_python CopyFile. Remote execution errors
            # still fail because remote_local_fallback remains false above.
            assert '--spawn_strategy=remote,local' in routed, 'mandatory local actions have no available strategy'
            for override in [
                    '--remote_exec_header=x-buildbuddy-platform.Pool=default',
                    '--remote_exec_header=x-buildbuddy-platform.use-self-hosted-executors=false',
                    '--remote_default_exec_properties=Pool=default',
                    '--remote_executor=', '--remote_local_fallback=true',
                    '--spawn_strategy=local', '--spawn_strategy=standalone',
                    '--spawn_strategy=sandboxed', '--strategy=TestRunner=local',
                    '--strategy=CopyFile=standalone', '--test_strategy=local']:
                try:
                    run_request(Namespace(**{**vars(args), 'command': command+[override]}))
                except RemoteError:
                    pass
                else:
                    raise AssertionError('caller bypassed repository pool routing')
            args.cloud = True
            assert 'Pool' not in run_request(args)['platform_properties']
            assert agent_command(command, args.repo, cloud=True, arch=arch) == command
    print('routing selftest: PASS (repo split; explicit architectures; runner and actions; '
          'runner stays cloud; mandatory local actions allowed; no remote error fallback; '
          'caller strategy/pool overrides refused; explicit cloud opt-out)')


def full_history_selftest():
    import contextlib
    import io
    import subprocess
    import tempfile
    from types import SimpleNamespace
    from unittest.mock import patch

    commit = 'a' * 40
    common = ['--repo', 'https://github.com/RobTand/tessera', '--commit', commit,
              '--branch', 'main', '--timeout', '10']
    command = ['bazel', 'test', '//tests:test_core', '--config=remote', '--jobs=16']
    with patch.object(uuid, 'uuid4', return_value=SimpleNamespace(hex='0' * 32)):
        default = run_request(parse_args(['test', *common, '--', *command]))
        step_hashes = [hashlib.sha256(step['run'].encode()).hexdigest() for step in default['steps']]
        assert step_hashes == ['02a646a66876faa70df38dd26ebc608563de1661ee9dab025bf5ec90ffc1800f', 'e051d0b08172fc7be0466487ca6f89474ad4295034c037657e4feb903bfb58f6'], step_hashes
        args = parse_args(['test', '--full-history', *common, '--', *command])
        assert args.full_history is True
        body = run_request(args)
        assert len(body['steps']) == 2, 'preparation must share secured command and final scan'
        first = shlex.split(body['steps'][0]['run'])[4]
        assert '--refetch' in first and '--no-filter' in first and '--tags' in first
        assert 'rev-parse' in first and 'HEAD' in first and commit in first
        assert 'rev-list' in first and '--missing=print' in first and '--all' in first
        assert '.promisor' in first and 'partialclonefilter' in first
        assert 'run_with_full_history(' in first and repr(commit) in first
        assert first.index('if prepare_full_history(commit):') < first.index('run_and_record(command, mode, result_path)\n        return')
        assert body['steps'][-1] == default['steps'][-1], 'final scanner changed'
        args.full_history = False
        assert run_request(args) == default

    # Execute the generated wrapper with intercepted Git calls and fake secrets.
    # Preparation errors must skip Bazel while still reaching the final scanner.
    regressions = []
    for scenario in ('complete', 'shallow', 'old-git', 'fetch-failed', 'head-changed',
                     'missing', 'walk-failed', 'promisor-config', 'marker-vanished',
                     'marker-denied', 'git-timeout', 'git-unavailable'):
        with tempfile.TemporaryDirectory(prefix='bbrun-full-history-') as directory:
            root = Path(directory)
            pack = root / 'objects/pack'
            pack.mkdir(parents=True)
            marker = pack / ('pack-' + 'b' * 40 + '.promisor')
            marker.write_text('')
            upload = root / 'upload'
            upload.mkdir()
            calls = []
            fetched = [False]
            fake_secret = 'offline-full-history-secret'
            unlink = Path.unlink
            marker_attempts = []

            def unlink_marker(path, *positional, **keywords):
                if path == marker and scenario in ('marker-vanished', 'marker-denied'):
                    marker_attempts.append(path)
                    if scenario == 'marker-vanished':
                        unlink(path)
                        raise FileNotFoundError(fake_secret + ' https://hidden.invalid/')
                    raise PermissionError(fake_secret + ' https://hidden.invalid/')
                return unlink(path, *positional, **keywords)

            def git(argv, **kwargs):
                assert argv[0] == 'git'
                env = kwargs['env']
                assert 'BUILDBUDDY_API_KEY' not in env and 'GITHUB_TOKEN' not in env
                assert 'BBRUN_FINAL_SCAN' not in env
                assert fake_secret not in json.dumps(env)
                assert kwargs.get('capture_output') is True
                calls.append(argv)
                code, output = 0, ''
                if '--version' in argv:
                    output = 'git version 2.35.1' if scenario == 'old-git' else 'git version 2.43.0'
                elif 'fetch' in argv:
                    assert '--refetch' in argv and '--no-filter' in argv and '--tags' in argv
                    assert 'origin' in argv and '+refs/heads/*:refs/remotes/origin/*' in argv
                    assert ('--unshallow' in argv) == (scenario == 'shallow')
                    if scenario == 'git-timeout':
                        raise subprocess.TimeoutExpired(argv, 300, output=fake_secret)
                    if scenario == 'git-unavailable':
                        raise FileNotFoundError(fake_secret + ' https://hidden.invalid/')
                    fetched[0] = True
                    code = 1 if scenario == 'fetch-failed' else 0
                elif 'rev-parse' in argv:
                    if '--is-shallow-repository' in argv:
                        output = 'true' if scenario == 'shallow' and not fetched[0] else 'false'
                    elif '--git-path' in argv:
                        output = str(pack)
                    else:
                        output = 'b' * 40 if scenario == 'head-changed' else commit
                elif 'rev-list' in argv:
                    assert '--all' in argv and 'HEAD' in argv and '--missing=print' in argv
                    output = commit + '\n' + 'b' * 40 + ' historical-path\n'
                    if scenario == 'missing':
                        output += '?' + 'c' * 40 + '\n'
                    code = 1 if scenario == 'walk-failed' else 0
                elif 'config' in argv:
                    if '--get-regexp' in argv:
                        code = 0 if scenario == 'promisor-config' else 1
                        output = 'remote.other.promisor true' if code == 0 else ''
                else:
                    raise AssertionError('unexpected Git call')
                return subprocess.CompletedProcess(argv, code, output, fake_secret + ' https://hidden.invalid/')

            args.full_history = True
            with patch.object(uuid, 'uuid4', return_value=SimpleNamespace(hex='1' * 32)):
                body = run_request(args)
            environment = {'PATH': os.defpath, 'HOME': str(root),
                           'BUILDBUDDY_ARTIFACTS_DIRECTORY': str(upload),
                           'BUILDBUDDY_API_KEY': fake_secret, 'GITHUB_TOKEN': fake_secret}
            output, errors = io.StringIO(), io.StringIO()
            with patch.dict(os.environ, environment, clear=True), \
                    patch('subprocess.run', side_effect=git), \
                    patch.object(Path, 'unlink', new=unlink_marker), \
                    patch('subprocess.call', return_value=0) as execute, \
                    contextlib.redirect_stdout(output), contextlib.redirect_stderr(errors):
                source = shlex.split(body['steps'][0]['run'])[4].replace(
                    '/tmp/bbrun-' + '1' * 32 + '.status', str(root / 'result.status'))
                exec(compile(source, '<full-history-step>', 'exec'), {})
            text = output.getvalue() + errors.getvalue()
            assert fake_secret not in text and 'hidden.invalid' not in text
            assert 'full history:' in text
            state = root / 'result.status'
            if scenario == 'marker-vanished' and not execute.called:
                regressions.append('vanished promisor marker must be tolerated')
            if scenario in ('marker-vanished', 'marker-denied'):
                assert marker_attempts == [marker], 'marker race was not exercised'
            for call in calls:
                if call[:5] != ['git', '-c', 'gc.auto=0', '-c', 'maintenance.auto=false']:
                    regressions.append('Git auto-maintenance must be disabled: ' + scenario)
                    break
            if scenario in ('complete', 'shallow') or (scenario == 'marker-vanished' and execute.called):
                assert execute.call_args.args[0] == agent_command(command, args.repo)
                assert not marker.exists()
                assert state.read_text() == '0'
                summary = json.loads(next(line.split('full history: ', 1)[1]
                                          for line in text.splitlines() if line.startswith('full history: ')))
                assert summary == {'fetched': True, 'objects': 2, 'HEAD': commit}
            else:
                assert not execute.called, 'Bazel ran after checkout preparation failed'
                assert state.read_text() == '75'
                assert 'full history preparation failed:' in text
                typed_failure = {
                    'marker-denied': 'promisor-cleanup (PermissionError)',
                    'git-timeout': 'fetch (TimeoutExpired)',
                    'git-unavailable': 'fetch (FileNotFoundError)',
                    'fetch-failed': 'fetch (RuntimeError)',
                }.get(scenario)
                if typed_failure and 'full history preparation failed: ' + typed_failure not in text:
                    regressions.append('missing typed failure: ' + typed_failure)
            assert not list(upload.iterdir())
            state.unlink()
            staged = Path(str(state) + '.artifacts')
            if staged.exists():
                staged.rmdir()
    for interruption in (KeyboardInterrupt, SystemExit):
        with patch('subprocess.run', side_effect=interruption), \
                contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            try:
                prepare_full_history(commit)
            except interruption:
                pass
            else:
                regressions.append(interruption.__name__ + ' must propagate')
    # Real Git fixture: a shallow, blob-filtered checkout must gain historical
    # blobs, other branches and tags without changing its detached HEAD.
    with tempfile.TemporaryDirectory(prefix='bbrun-history-git-') as directory:
        root = Path(directory)
        source, remote, checkout = (root / name for name in ('source', 'remote.git', 'checkout'))
        environment = {'PATH': os.defpath, 'HOME': str(root), 'LANG': 'C.UTF-8'}

        def local_git(*arguments, cwd=root):
            result = subprocess.run(['git', *map(str, arguments)], cwd=cwd, env=environment,
                                    capture_output=True, text=True, timeout=30)
            assert result.returncode == 0, 'local Git fixture failed'
            return result.stdout.strip()

        local_git('init', '--initial-branch=main', source)
        local_git('init', '--bare', remote)
        local_git('config', 'uploadpack.allowFilter', 'true', cwd=remote)
        (source / 'historical.txt').write_text('old blob\n' * 100)
        local_git('add', '.', cwd=source)
        local_git('-c', 'user.name=Offline', '-c', 'user.email=offline@example.invalid',
                  'commit', '-m', 'old', cwd=source)
        historical = local_git('rev-parse', 'HEAD', cwd=source)
        local_git('tag', 'historical', cwd=source)
        local_git('branch', 'other', cwd=source)
        (source / 'historical.txt').write_text('new blob\n')
        local_git('add', '.', cwd=source)
        local_git('-c', 'user.name=Offline', '-c', 'user.email=offline@example.invalid',
                  'commit', '-m', 'new', cwd=source)
        requested = local_git('rev-parse', 'HEAD', cwd=source)
        local_git('push', remote, '--all', cwd=source)
        local_git('push', remote, '--tags', cwd=source)
        local_git('symbolic-ref', 'HEAD', 'refs/heads/main', cwd=remote)
        local_git('clone', '--filter=blob:none', '--depth=1', '--no-checkout', remote.as_uri(), checkout)
        local_git('checkout', '--detach', requested, cwd=checkout)
        assert local_git('rev-parse', '--is-shallow-repository', cwd=checkout) == 'true'
        assert list((checkout / '.git/objects/pack').glob('*.promisor'))
        vanished_markers = []
        unlink = Path.unlink

        def concurrent_unlink(path, *positional, **keywords):
            if path.resolve().parent == checkout / '.git/objects/pack' and path.suffix == '.promisor':
                vanished_markers.append(path)
                unlink(path)
            return unlink(path, *positional, **keywords)

        previous = Path.cwd()
        try:
            os.chdir(checkout)
            with patch.dict(os.environ, environment, clear=True), \
                    patch.object(Path, 'unlink', new=concurrent_unlink), \
                    contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                if not prepare_full_history(requested):
                    regressions.append('real shallow partial-clone marker race must be tolerated')
        finally:
            os.chdir(previous)
        assert vanished_markers, 'real clone marker race was not exercised'
        assert not regressions, '; '.join(regressions)
        assert local_git('rev-parse', 'HEAD', cwd=checkout) == requested
        assert local_git('rev-parse', '--is-shallow-repository', cwd=checkout) == 'false'
        assert local_git('show', historical + ':historical.txt', cwd=checkout) == ('old blob\n' * 100).strip()
        assert local_git('rev-parse', 'refs/remotes/origin/other', cwd=checkout) == historical
        assert local_git('rev-parse', 'refs/tags/historical', cwd=checkout) == historical
        assert not list((checkout / '.git/objects/pack').glob('*.promisor'))
        assert '?' not in local_git('rev-list', '--objects', '--missing=print', '--all', 'HEAD', cwd=checkout)
        local_git('fsck', '--full', cwd=checkout)
    print('full history selftest: PASS (opt-in secured preparation; default golden bytes; '
          'complete fetch; shallow clone; pinned HEAD; missing objects; promisor cleanup; '
          'old Git and fetch failure fail closed; scrubbed Git children; safe summary; final scan retained)')


def action_size_selftest():
    import contextlib
    import io
    from types import SimpleNamespace
    from unittest.mock import patch

    common = ['--repo', 'https://github.com/RobTand/prismaquant', '--commit', 'a' * 40,
              '--branch', 'main', '--detach']
    command = ['bazel', 'test', '--config=remote', '--jobs=16', '//tests:test_core']
    fake_key = 'offline-action-size-sentinel'
    with patch.object(uuid, 'uuid4', return_value=SimpleNamespace(hex='0' * 32)):
        normal = run_request(parse_args(['test', *common, '--', *command]))
    # Compare the exact HTTP bytes, including JSON spacing and escaping.
    with patch('urllib.request.urlopen', return_value=io.BytesIO(b'{}')) as transport:
        BuildBuddy(fake_key).call('Run', normal)
    assert transport.call_count == 1
    assert transport.call_args.args[0].data == json.dumps(normal).encode()

    for mode, argv in (
            ('test', [*command, '--target_pattern_file=ci/pq-targets.txt']),
            ('query', ['bazel', 'query', '//tests:all']),
            ('step', ['true'])):
        with patch.object(Path, 'read_text', return_value=fake_key), \
                patch(__name__ + '.wait_for_gate_window'), \
                patch('urllib.request.urlopen', return_value=io.BytesIO(
                    b'{"invocationId":"offline-small-action"}')) as transport, \
                contextlib.redirect_stdout(io.StringIO()):
            assert main([mode, *common, '--', *argv]) == 0
        assert transport.call_count == 1, mode
        if mode == 'test':
            assert b'--target_pattern_file=ci/pq-targets.txt' in transport.call_args.args[0].data

    # The CLI must refuse before even creating an HTTP request. Never read a
    # real key, wait for a real gate, or allow a failed guard to reach the wire.
    for mode, argv in (
            ('step', ['echo', 'x' * 106823]),
            ('test', [*command, '//tests:' + 'x' * 106823]),
            ('query', ['bazel', 'query', 'x' * 106823])):
        errors = io.StringIO()
        with patch.object(Path, 'read_text', return_value=fake_key), \
                patch(__name__ + '.wait_for_gate_window'), \
                patch('urllib.request.Request') as request, \
                patch('urllib.request.urlopen', return_value=io.BytesIO(
                    b'{"invocationId":"offline-oversized-action"}')) as transport, \
                contextlib.redirect_stdout(io.StringIO()), \
                contextlib.redirect_stderr(errors):
            code = main([mode, *common, '--', *argv])
        assert not request.called and not transport.called, 'oversized action reached HTTP transport'
        assert code == 2, (mode, code)
        message = errors.getvalue()
        assert re.search(r'\d+ bytes', message) and '122880' in message, message
        assert 'pass long target lists with --target_pattern_file' in message
        assert 'x' * 100 not in message and fake_key not in message

    # Count every step and the other request fields, including UTF-8 text.
    for body in ({**normal, 'steps': [{'run': 'x' * 50000}] * 2},
                 {**normal, 'env': {'LARGE': '\u754c' * 40000}},
                 {**normal, 'branch': 'x' * 100000}):
        with patch('urllib.request.urlopen') as transport:
            try:
                BuildBuddy(fake_key).call('Run', body)
            except RemoteError as error:
                assert 'pass long target lists with --target_pattern_file' in str(error)
            else:
                raise AssertionError('oversized combined action accepted')
        assert not transport.called
    print('action size selftest: PASS (oversized test/query/step refused with exit 2 before HTTP; '
          'combined steps, env and other fields counted; normal HTTP bytes unchanged; '
          '--target_pattern_file passes)')


def selftest():
    import contextlib
    import io
    import tempfile
    from unittest.mock import patch

    action_size_selftest()
    full_history_selftest()
    routing_selftest()
    gate_window_selftest()
    artifact_drop_selftest()
    security_selftest()
    environment_selftest()
    sentinel = 'offline-shared-client-key'
    fake_tokens = {'BUILDBUDDY_API_KEY': sentinel, 'GITHUB_TOKEN': 'offline-github-value',
                   'UV_API_KEY': 'offline-uv-value', 'PYTHON_AUTH_TOKEN': 'offline-python-value'}
    requests, queued = [], []
    running = {'invocationStatus': 'PARTIAL_INVOCATION_STATUS'}
    complete = {'invocationStatus': 'COMPLETE_INVOCATION_STATUS', 'success': True, 'bazelExitCode': 'OK'}
    now = [0.0]

    def opened(request, timeout):
        method = request.full_url.rsplit('/', 1)[1]
        payload = json.loads(request.data)
        assert request.get_header('X-buildbuddy-api-key') == sentinel
        for value in fake_tokens.values():
            assert value.encode() not in request.data
        if method == 'Run':
            for name in fake_tokens:
                assert name not in payload['env']
                assert name not in payload['env']
        requests.append((method, payload, timeout))
        expected_method, response = queued.pop(0)
        assert method == expected_method, method
        if isinstance(response, Exception):
            raise response
        return io.BytesIO(response if isinstance(response, bytes) else json.dumps(response).encode())

    def run(argv, replies, expected_code):
        queued.extend(replies)
        before = len(requests)
        output, errors = io.StringIO(), io.StringIO()
        with patch.object(sys, 'argv', ['bbrun.py', *argv]), contextlib.redirect_stdout(output), \
                contextlib.redirect_stderr(errors), patch(__name__ + '.wait_for_gate_window') as gate_wait:
            code = main()
        submitted = any(method == 'Run' for method, _, _ in requests[before:])
        assert gate_wait.call_count == int(submitted), 'gate wait must apply only to submissions'
        assert code == expected_code, (code, errors.getvalue())
        assert not queued, 'an expected API call was not made'
        for value in fake_tokens.values():
            assert value not in json.dumps(argv)
            assert value not in output.getvalue()
            assert value not in errors.getvalue()
        return output.getvalue(), requests[before:]

    def terminal_replies(record=complete):
        return [('GetInvocation', {'invocation': [record]}),
                ('GetLog', {'log': {'contents': 'first chunk\n'}, 'nextPageToken': '0001'}),
                ('GetLog', {'log': {'contents': 'query result //example:target\n'}, 'next_page_token': '0002'}),
                ('GetLog', {'log': {'contents': ''}})]

    with tempfile.TemporaryDirectory(prefix='bbrun-selftest-') as folder:
        home = Path(folder)
        keyfile = home / '.config/buildbuddy/api_key'
        keyfile.parent.mkdir(parents=True)
        keyfile.write_text(sentinel)
        with patch.object(Path, 'home', return_value=home), patch.dict(os.environ, fake_tokens, clear=True):
            with patch('urllib.request.urlopen', side_effect=opened), patch('subprocess.Popen', side_effect=AssertionError('local child started')):
                common = ['--repo', 'https://github.com/RobTand/prismabuild', '--commit', 'a' * 40,
                          '--branch', 'prismabuild-2667', '--timeout', '20']
                commands = {
                    'test': ['bazel', 'test', '//tests:test_core', '--config=remote', '--jobs=16'],
                    'query': ['bazel', 'query', 'attr(tags, runner, tests(//...))', '--output=label'],
                    'step': ['python3', '-m', 'pytest', 'tests/test example.py'],
                }
                for mode, command in commands.items():
                    output, calls = run([mode, *common, '--', *command],
                                        [('Run', {'invocationId': 'offline-run'})] + terminal_replies(), 0)
                    assert 'first chunk\n' in output
                    assert 'query result //example:target\n' in output
                    pages = [body['page_token'] for method, body, _ in calls if method == 'GetLog']
                    assert pages == ['0000', '0001', '0002']
                    if mode == 'query':
                        assert "['bazel', 'query'," in shlex.split(calls[0][1]['steps'][0]['run'])[4]
                    if mode == 'step':
                        assert shlex.split(calls[0][1]['steps'][0]['run'])[:4] == ['python3', '-I', '-S', '-c']
                # Preserve the pre-mode CLI and enforce the original load limit.
                legacy = parse_args([*common, '--', *commands['test']])
                assert legacy.mode == 'test' and run_request(legacy)['wait_until'] == 'STARTED'
                for command in (['bazel', 'test', '//tests:test_core', '--jobs=40'],
                                ['bazel', 'test', '--jobs=16', '--config=remote']):
                    legacy.command = command
                    try:
                        run_request(legacy)
                    except RemoteError:
                        pass
                    else:
                        raise AssertionError('unsafe job configuration accepted')
                for mode in ('test', 'step'):
                    output, calls = run([mode, *common, '--detach', '--', *commands[mode]],
                                        [('Run', {'invocation_id': 'offline-detached'})], 0)
                    assert 'detached: offline-detached' in output
                    assert [method for method, _, _ in calls] == ['Run']
                    assert calls[0][1]['wait_until'] == 'QUEUED'
                with patch('time.monotonic', side_effect=lambda: now[0]), patch('time.sleep', side_effect=lambda seconds: now.__setitem__(0, now[0] + seconds)):
                    output, calls = run(['wait', 'offline-pending', '--max-wait', '1'],
                                        [('GetInvocation', {'invocation': [running]})], 2)
                    assert now[0] == 1 and calls[0][2] == 1
                    assert 'still running: offline-pending' in output
                    assert 'bbrun.py wait offline-pending' in output
                    # A later call resumes without submitting Run again.
                    output, calls = run(['wait', 'offline-pending'], terminal_replies(), 0)
                    assert calls[0][0] == 'GetInvocation'
                    assert all(method != 'Run' for method, _, _ in calls)
                    assert '"success": true' in output
                    failed = {**complete, 'success': False, 'bazelExitCode': 'Failed'}
                    run(['wait', 'offline-failed'], terminal_replies(failed), 1)
                    disconnected = {'invocationStatus': 'DISCONNECTED_INVOCATION_STATUS'}
                    run(['wait', 'offline-disconnected'], terminal_replies(disconnected), 1)
                    # Remote Run API and Bazel child invocations use different
                    # successful exit-code names, including when success is omitted.
                    for exit_code in ('OK', 'SUCCESS'):
                        record = {'command': 'remote run', 'invocationStatus': 'COMPLETE_INVOCATION_STATUS',
                                  'bazelExitCode': exit_code}
                        run(['wait', 'offline-exit-code'], terminal_replies(record), 0)
                    parent = {**complete, 'command': 'remote run',
                              'childInvocations': [{'invocationId': 'offline-child'}]}
                    for exit_code in ('OK', 'SUCCESS'):
                        child = {'command': 'test', 'invocationStatus': 'COMPLETE_INVOCATION_STATUS',
                                 'bazelExitCode': exit_code}
                        replies = [('GetInvocation', {'invocation': [parent]}),
                                   ('GetInvocation', {'invocation': [child]})] + terminal_replies()[1:]
                        output, calls = run(['wait', 'offline-parent'], replies, 0)
                        assert calls[0][1]['include_child_invocations'] is True
                        assert calls[1][1]['selector']['invocation_id'] == 'offline-child'
                        assert 'child test verdict' in output and 'offline-child' in output
                    child = {**failed, 'command': 'test'}
                    output, _ = run(['wait', 'offline-parent'],
                                    [('GetInvocation', {'invocation': [parent]}),
                                     ('GetInvocation', {'invocation': [child]})] + terminal_replies()[1:], 1)
                    assert 'child test verdict' in output and 'Failed' in output
                    child = {**running, 'command': 'test'}
                    output, _ = run(['wait', 'offline-parent'],
                                    [('GetInvocation', {'invocation': [parent]}),
                                     ('GetInvocation', {'invocation': [child]})], 2)
                    assert 'still running: offline-parent' in output and 'offline-child' in output
                    run(['wait', 'offline-parent'],
                        [('GetInvocation', {'invocation': [parent]}),
                         ('GetInvocation', {'invocation': []})], 2)
                    # A passed child cannot hide a failed wrapper or disconnect.
                    run(['wait', 'offline-parent'],
                        [('GetInvocation', {'invocation': [{**parent, 'success': False, 'bazelExitCode': 'Failed'}]}),
                         ('GetInvocation', {'invocation': [{**complete, 'command': 'test'}]})]
                        + terminal_replies()[1:], 1)
                    snake_parent = {**complete, 'command': 'remote run',
                                    'child_invocations': [{'invocation_id': 'offline-child'}]}
                    run(['wait', 'offline-parent'],
                        [('GetInvocation', {'invocation': [snake_parent]}),
                         ('GetInvocation', {'invocation': [{'command': 'test',
                           'invocationStatus': 'COMPLETE_INVOCATION_STATUS', 'bazel_exit_code': 'SUCCESS'}]})]
                        + terminal_replies()[1:], 0)
                    for record in ({**complete, 'bazelExitCode': 'FAILED'},
                                   {**complete, 'success': False},
                                   {'invocationStatus': 'COMPLETE_INVOCATION_STATUS'}):
                        run(['wait', 'offline-inconsistent'], terminal_replies(record), 1)
                    # A whole default wait consumes at most 600 fake seconds.
                    before = now[0]
                    run(['wait', 'offline-long'], [('GetInvocation', {'invocation': [running]})] * 60, 2)
                    assert now[0] - before == 600
                file_record = {'invocation': [{**complete, 'artifacts': [
                    {'name': 'result.bin', 'uri': 'bytestream://example/blobs/hash/3'}],
                    'buildToolLogs': [{'name': 'profile.gz', 'uri': 'bytestream://example/blobs/profile/4'}]}]}
                output, calls = run(['artifacts', 'offline-artifacts'], [('GetInvocation', file_record)], 0)
                assert 'result.bin' in output and 'profile.gz' in output
                assert calls[0][1]['include_artifacts'] and calls[0][1]['include_build_tool_logs']
                target = home / 'artifact.bin'
                output, calls = run(['artifacts', 'offline-artifacts', '--fetch', 'result.bin', '--out', str(target)],
                                    [('GetInvocation', file_record), ('GetFile', b'\x00\xff\x01')], 0)
                assert target.read_bytes() == b'\x00\xff\x01'
                assert calls[-1][1] == {'uri': 'bytestream://example/blobs/hash/3'}
                run(['artifacts', 'offline-artifacts', '--fetch', 'absent', '--out', str(target)],
                    [('GetInvocation', file_record)], 1)
                target.unlink()
                for payload in (sentinel.encode(), gzip.compress(sentinel.encode())):
                    run(['artifacts', 'offline-artifacts', '--fetch', 'result.bin', '--out', str(target)],
                        [('GetInvocation', file_record), ('GetFile', payload)], 1)
                    assert not target.exists()
                # Check the live-tail repeat case without appending it twice.
                client = BuildBuddy(sentinel)
                queued.append(('GetLog', {'log': {'contents': 'live-tail'}, 'nextChunkId': 'ignored', 'nextPageToken': '0000'}))
                assert log(client, 'offline-live') == 'live-tail'
                queued.extend([('GetLog', {'log': {'contents': 'a'}, 'nextPageToken': '0001'}),
                               ('GetLog', {'log': {'contents': 'b'}, 'nextPageToken': '0000'})])
                try:
                    log(client, 'offline-cycle')
                except RemoteError:
                    pass
                else:
                    raise AssertionError('cyclic log pagination accepted')
                assert not queued
                try:
                    client.call('Run', {'accidental': sentinel})
                except RemoteError:
                    pass
                else:
                    raise AssertionError('credential-bearing request body accepted')
    print('selftest: PASS (test, query, step, wait, artifacts; header-only key; no local children outside offline scan simulations; scrubbed step env with artifacts directory; bounded resume/detach; complete log paging; raw/gzip artifact safety; OK/SUCCESS and child test verdicts)')
    return 0


if __name__ == '__main__':
    sys.exit(main())
"######;
