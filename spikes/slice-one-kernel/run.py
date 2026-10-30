"""Run the kernel's tests, or Kani proofs, as one PrismaBuild action.

    run.py tests             run the tests
    run.py prove NAME...     prove only the named harnesses
    run.py prove             prove every harness

Each proof is small enough to be its own PrismaBuild action, so a campaign
can run them all in parallel. Output streams, so a killed run still shows how
far it got.
"""
import os
import re
import pathlib
import subprocess
import sys
import time

HERE = pathlib.Path(__file__).resolve().parent


def step(label, argv):
    print(f"== {label}: {' '.join(argv)}", flush=True)
    start = time.monotonic()
    code = subprocess.run(argv, cwd=HERE).returncode
    print(f"== {label}: exit {code} in {time.monotonic() - start:.1f}s", flush=True)
    return code


mode, names = (sys.argv[1], sys.argv[2:]) if len(sys.argv) > 1 else ("", [])
if mode == "tests":
    sys.exit(step("tests", ["cargo", "test", "--release", "--no-fail-fast", "--", "--nocapture"]))
if mode == "prove":
    kani = ["cargo", "kani", "--output-format", "terse", "-j", os.environ.get("KANI_JOBS", "1")]
    for name in names:
        kani += ["--harness", f"proofs::{name}", "--exact"]
    print(f"== kani: {' '.join(kani)}", flush=True)
    start = time.monotonic()
    done = subprocess.run(kani, cwd=HERE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    print(done.stdout, flush=True)
    print(f"== kani: exit {done.returncode} in {time.monotonic() - start:.1f}s", flush=True)
    if done.returncode:
        sys.exit(done.returncode)
    # A proof whose cover checks aren't all reachable proved nothing about
    # those cases, so it fails too.
    covers = re.findall(r"\*\* (\d+) of (\d+) cover properties satisfied", done.stdout)
    if not covers:
        sys.exit("== kani: no cover results reported")
    if any(int(hit) < int(total) for hit, total in covers):
        sys.exit("== kani: a cover check was not satisfied, so the proof may be vacuous")
    sys.exit(0)
sys.exit(__doc__)
