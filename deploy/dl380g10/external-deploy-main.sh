#!/usr/bin/env bash
# Standalone installed caller. No product checkout is needed to start it.
set -euo pipefail
exec python3 -I - "$@" <<'PY'
import fcntl
import os
from pathlib import Path
import re
import signal
import stat
import subprocess
import sys
import tempfile

UPSTREAM = 'https://github.com/RobTand/trustladderiii.git'
ROOT = Path('/home/rob/.local/share/tl3-deploy-sources')
os.umask(0o077)
environment = {k: v for k, v in os.environ.items() if not k.startswith('GIT_')}
environment.update(GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_GLOBAL='/dev/null',
                   GIT_TERMINAL_PROMPT='0')

def git(*args, cwd=None, timeout=30):
    with subprocess.Popen(['git', *args], cwd=cwd, env=environment,
                          stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                          start_new_session=True) as child:
        try:
            output, _ = child.communicate(timeout=timeout)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGKILL)
            child.communicate(timeout=5)
            raise RuntimeError('source Git deadline exceeded') from None
        if child.returncode:
            raise RuntimeError('source Git command refused; output suppressed')
        return output.strip()

def owned(path):
    for part in (path, *path.parents):
        info = part.lstat()
        if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
            raise RuntimeError('source path must contain only directories')
    info = path.stat()
    if info.st_uid != os.getuid() or info.st_mode & 0o022:
        raise RuntimeError('source directory must be owned and privately writable')

def verify(tree, sha):
    owned(tree)
    if not (tree / '.git').is_dir() or (tree / '.git').is_symlink():
        raise RuntimeError('source must be an independent clone')
    owned(tree / '.git')
    if git('rev-parse', '--show-toplevel', cwd=tree).decode() != str(tree):
        raise RuntimeError('source repository boundary mismatch')
    if git('rev-parse', 'HEAD', cwd=tree).decode() != sha:
        raise RuntimeError('source HEAD mismatch; existing trees are never reset')
    if git('status', '--porcelain=v1', '--untracked-files=all', '--ignored', cwd=tree):
        raise RuntimeError('source tree is dirty; existing trees are never cleaned')
    if git('rev-parse', '--is-shallow-repository', cwd=tree) != b'false':
        raise RuntimeError('qualification requires complete Git history')
    if git('config', '--get', 'remote.origin.url', cwd=tree).decode() != UPSTREAM:
        raise RuntimeError('source upstream mismatch')
    git('rev-list', '--objects', '--missing=error', sha, cwd=tree, timeout=120)

try:
    if len(sys.argv) != 2 or not re.fullmatch('[0-9a-f]{40}', sys.argv[1]):
        raise RuntimeError('one full exact qualified release SHA is required')
    sha = sys.argv[1]
    owned(ROOT.parent)
    ROOT.mkdir(mode=0o700, exist_ok=True)
    owned(ROOT)
    # The lock covers publication and exec handoff; inherited descriptors close
    # on exec. Concurrent invocations cannot publish over an existing tree.
    fd = os.open(ROOT / '.lock', os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    if (not stat.S_ISREG(os.fstat(fd).st_mode) or os.fstat(fd).st_uid != os.getuid()
            or os.fstat(fd).st_mode & 0o077):
        raise RuntimeError('unsafe source lock')
    fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    tree = ROOT / sha
    if not os.path.lexists(tree):
        with tempfile.TemporaryDirectory(prefix='.prepare-', dir=ROOT) as scratch:
            staged = Path(scratch) / 'source'
            git('clone', '--quiet', '--no-checkout', '--no-local', UPSTREAM,
                str(staged), timeout=300)
            git('fetch', '--quiet', 'origin', sha, cwd=staged, timeout=300)
            git('checkout', '--quiet', '--detach', sha, cwd=staged)
            verify(staged, sha)
            staged.rename(tree)
    verify(tree, sha)
    canonical = tree / 'deploy/dl380g10/deploy-main.sh'
    if (not canonical.is_file() or canonical.is_symlink()
            or any(p.is_symlink() for p in canonical.parents)):
        raise RuntimeError('exact source lacks the canonical caller')
    os.chdir(tree)
    os.execve('/bin/bash', ['bash', str(canonical), sha], environment)
except (OSError, RuntimeError) as error:
    print(f'exact-source caller refused: {error}', file=sys.stderr)
    sys.exit(1)
PY
