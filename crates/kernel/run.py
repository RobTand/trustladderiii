"""Run the kernel's tests, or Kani proofs, as one PrismaBuild action.

    run.py manifest [NAME...]  print a PrismaBuild campaign manifest
    run.py tests             build the kernel, then run its tests
    run.py prove NAME...     prove only the named harnesses
    run.py prove             prove every harness

Each proof is small enough to be its own PrismaBuild action, so a campaign
can run them all in parallel. Output streams, so a killed run still shows how
far it got.
"""
import os
import json
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
if mode == "manifest":
    env = {
        "PATH": "/home/rob/.cargo/bin:/usr/local/bin:/usr/bin:/bin",
        "RUSTUP_HOME": "/home/rob/.rustup", "CARGO_HOME": "/home/rob/.cargo",
        "KANI_HOME": "/home/rob/.kani", "CARGO_BUILD_JOBS": "1",
        "KANI_JOBS": "1", "OMP_NUM_THREADS": "1", "RAYON_NUM_THREADS": "1", "RUST_TEST_THREADS": "1",
    }
    source = (HERE / "src/proofs.rs").read_text()
    proofs = re.findall(r"^proof!\(\s*(\w+),", source, re.M)
    replies = set(re.findall(r"^proof!\(\s*(\w+),\s*check_apply\(\s*reply\(", source, re.M))
    selected = names or ["tests", *proofs]
    if set(selected) - {"tests", *proofs}:
        sys.exit("unknown proof name")
    rows = []
    for name in selected:
        if name in {"apply_new_request", "apply_new_adoption", "progress_from_queued", "apply_reviewer_out_of_room"}:
            memory = 44
        elif name.startswith("progress_from_"):
            memory = 24
        elif name in replies:
            memory = 44
        else:
            memory = 10
        rows.append({"cwd": str(HERE), "argv": ["/usr/bin/python3", "run.py", *(["tests"] if name == "tests" else ["prove", name])],
                     "cpus": 1, "demand": {"mem_gb": 4 if name == "tests" else memory},
                     "anywhere": True, "env": env, "priority": -10, "timeout_s": 3600})
    print(json.dumps(rows, indent=2))
    sys.exit(0)
if mode == "tests":
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
if mode == "prove":
    # KANI_OUTPUT=regular shows CBMC's progress, which tells where a run that
    # ran out of memory got to.
    output = os.environ.get("KANI_OUTPUT", "terse")
    kani = ["cargo", "kani", "-p", "tl3-kernel", "--output-format", output, "-j", os.environ.get("KANI_JOBS", "1")]
    for name in names:
        kani += ["--harness", f"proofs::{name}", "--exact"]
    print(f"== kani: {' '.join(kani)}", flush=True)
    start = time.monotonic()
    proc = subprocess.Popen(kani, cwd=HERE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    lines = []
    for line in proc.stdout:
        print(line, end="", flush=True)
        lines.append(line)
    code = proc.wait()
    print(f"== kani: exit {code} in {time.monotonic() - start:.1f}s", flush=True)
    if code:
        sys.exit(code)
    # A proof whose cover checks aren't all reachable proved nothing about
    # those cases, so it fails too.
    covers = re.findall(r"\*\* (\d+) of (\d+) cover properties satisfied", "".join(lines))
    if not covers:
        sys.exit("== kani: no cover results reported")
    if any(int(hit) < int(total) for hit, total in covers):
        sys.exit("== kani: a cover check was not satisfied, so the proof may be vacuous")
    sys.exit(0)
sys.exit(__doc__)
