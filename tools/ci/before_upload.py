"""Verify and run the shared secret scanner before every artifact upload."""
import contextlib
import hashlib
import io
from pathlib import Path
import runpy
import sys

CHECKER = Path(__file__).with_name('check_no_secret_leak.py')
PIN = CHECKER.with_suffix('.sha256')


def main(argv=None):
    expected = PIN.read_text().split()[0]
    if hashlib.sha256(CHECKER.read_bytes()).hexdigest() != expected:
        raise RuntimeError('Vendored secret-leak checker does not match its pinned SHA256')
    # Only this trusted scanner reads the named secrets. Executing it in-process
    # avoids handing those credentials to any child environment.
    output = io.StringIO()
    with contextlib.redirect_stdout(output):
        status = runpy.run_path(str(CHECKER))['main'](argv)
    diagnostics = output.getvalue()
    print(diagnostics, end='')
    return 75 if 'not scanned' in diagnostics else status


if __name__ == '__main__':
    sys.exit(main())
