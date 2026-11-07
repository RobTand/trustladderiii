"""Run a bounded command from the exact selected commit, outside PB's stamped tree."""
import re
import subprocess
import sys
import tempfile
from pathlib import Path


def main(argv):
    head, *command = argv
    if not re.fullmatch(r"[0-9a-f]{40}", head) or not command:
        raise ValueError("exact commit and command are required")
    source = Path(__file__).resolve().parents[2]
    with tempfile.TemporaryDirectory(prefix="tl3-exact-source-") as scratch:
        checkout = Path(scratch) / "source"
        subprocess.run(["git", "clone", "--quiet", "--shared", "--no-checkout",
                        str(source), str(checkout)], check=True, timeout=60)
        subprocess.run(["git", "checkout", "--quiet", "--detach", head],
                       cwd=checkout, check=True, timeout=60)
        actual = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=checkout, timeout=15)
        if actual.decode().strip() != head:
            raise RuntimeError("selected source identity mismatch")
        # The caller's PB deadline bounds the complete process group as well.
        return subprocess.run(command, cwd=checkout, timeout=1740).returncode


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
