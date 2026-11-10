"""Run kernel formatting, lint, build and tests inside an admitted action.

    run.py tests
"""
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


mode = sys.argv[1:]
if mode == ["tests"]:
    code = step("format", ["cargo", "fmt", "--all", "--", "--check"])
    if code:
        sys.exit(code)
    code = step("clippy", ["cargo", "clippy", "-p", "tl3-kernel", "--all-targets", "--locked", "--", "-D", "warnings"])
    if code:
        sys.exit(code)
    code = step("kernel build (no_std)", ["cargo", "build", "-p", "tl3-kernel", "--release"])
    if code:
        sys.exit(code)
    sys.exit(step("tests", ["cargo", "test", "-p", "tl3-kernel", "--release", "--no-fail-fast", "--", "--nocapture"]))
sys.exit(__doc__)
