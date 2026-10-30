"""Run the demo, then prove the kernel's laws with Kani. Runs as one PrismaBuild action."""
import pathlib
import subprocess
import sys
import time

HERE = pathlib.Path(__file__).resolve().parent


def step(label, argv):
    start = time.monotonic()
    done = subprocess.run(argv, cwd=HERE, capture_output=True, text=True)
    print(f"== {label}: exit {done.returncode} in {time.monotonic() - start:.1f}s", flush=True)
    text = (done.stdout + done.stderr).strip()
    if text:
        print(text, flush=True)
    if done.returncode:
        sys.exit(done.returncode)


step("demo", ["cargo", "run", "--release", "--quiet"])
step("kani", ["cargo", "kani", "--output-format", "terse", "-j", "2"])
