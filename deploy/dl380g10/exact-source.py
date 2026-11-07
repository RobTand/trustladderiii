"""Run a bounded command from exact qualified source, outside PB's stamped tree."""
import os
import re
import signal
import subprocess
import sys
import tempfile
from pathlib import Path


def main(argv):
    head, *command = argv
    if not re.fullmatch(r"[0-9a-f]{40}", head) or not command:
        raise ValueError("exact commit and command are required")
    source = Path(__file__).resolve().parents[2]
    # Git's inherited repository overrides must not redirect identity checks.
    environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    with tempfile.TemporaryDirectory(prefix=".tl3-exact-source-", dir=source.parent) as scratch:
        checkout = Path(scratch) / "source"
        subprocess.run(["git", "clone", "--quiet", "--shared", "--no-checkout",
                        str(source), str(checkout)], env=environment, check=True, timeout=60)
        subprocess.run(["git", "checkout", "--quiet", "--detach", head],
                       cwd=checkout, env=environment, check=True, timeout=60)
        def git(*args):
            return subprocess.check_output(["git", *args], cwd=checkout, env=environment, timeout=15).strip()
        if git("rev-parse", "HEAD").decode() != head or git("status", "--porcelain"):
            raise RuntimeError("selected source identity mismatch")
        if git("rev-parse", "--is-shallow-repository") != b"false":
            raise RuntimeError("qualification requires complete source history before the outage")
        git("rev-parse", f"{head}^{{tree}}")
        subprocess.run(["git", "merge-base", "--is-ancestor", f"{head}^", head],
                       cwd=checkout, env=environment, check=True, timeout=15)
        with subprocess.Popen(command, cwd=checkout, env=environment, start_new_session=True) as child:
            try:
                return child.wait(timeout=7100)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait(timeout=5)
                raise RuntimeError("exact-source action exceeded its bounded deadline") from None


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
