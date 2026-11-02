"""Shared upload hook. The coordinator will pin PrismaQuant's checker here."""
import hashlib
from pathlib import Path
import subprocess
import sys

from child_env import child_env

# SECRET-LEAK-CHECKER HOOK: vendor check_no_secret_leak.py byte-identical here
# and replace None with its coordinator-supplied sha256 in the follow-up commit.
CHECKER = Path(__file__).with_name('check_no_secret_leak.py')
CHECKER_SHA256 = None


def main():
    if CHECKER_SHA256 is None:
        print('::warning::Shared secret-leak checker is awaiting its published SHA.')
        return 0
    if hashlib.sha256(CHECKER.read_bytes()).hexdigest() != CHECKER_SHA256:
        raise RuntimeError('Vendored secret-leak checker does not match its pinned SHA256')
    return subprocess.run([sys.executable, str(CHECKER), *sys.argv[1:]],
                          env=child_env(), check=False).returncode


if __name__ == '__main__':
    sys.exit(main())
